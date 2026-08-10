//! Durable board command tests, including lane lifecycle task persistence.

use super::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use wire::{Accepted, CanonicalUuid, ResidencyHead, ResidentState};

mod subscription;

// Shared board and journal construction stays separate from the scenarios
// so both source files remain inside the structural review bound.
mod fixture;
use fixture::*;

// Renewal and restart assertions are one bounded reclaim scenario seam.
mod reclaim;
use reclaim::*;

// Each allocator re-read refusal is pinned by a scenario that moves only
// what that one comparison reads, so severing them apart is observable.
mod allocation_guard;
use allocation_guard::*;

mod gate_findings;
mod origin;

// The order in which one claim's preconditions refuse is documented as a
// contract on `ClaimGrant`, and a caller sees only the first refusal, so it
// is pinned apart from the scenarios that drive a claim that succeeds.
mod claim_grant;

// Automatic capacity uses the same allocator guard and mint before launch.
mod automatic_lifecycle;

fn at<'a>(answer: &'a BoardAnswer, task: &TaskHandle) -> &'a BoardTask {
    task_heads(answer, task).expect("fixture task is visible")
}

fn residency(answer: &BoardAnswer, seat: &Seat) -> Witness {
    residency_witness(answer, seat.lane, seat.occupant)
}

fn declare_session(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    identity: CanonicalUuid,
    group: u32,
) {
    let before = board(bus, epoch, Actor::Operator);
    let mut residency = before.residency.body;
    let row = residency
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane");
    let session = wire::LaneSession {
        lane: row.lane,
        occupant: row.occupant.expect("fixture occupant"),
        worktree: row.worktree.clone(),
        vendor: row.vendor.clone(),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: identity.to_string(),
        locus: wire::TapeLocus {
            member: 0,
            offset: u64::from(group),
            length: 1,
        },
    };
    row.session = Some(session.clone());
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let now = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, now, Producer::ResidentRecognition);
    let declared = write
        .fact(
            wire::body::LaneSessionDeclared {
                lane: session.lane,
                occupant: session.occupant,
                worktree: session.worktree,
                vendor: session.vendor,
                launch: session.launch,
                epoch: session.epoch,
                stream: session.stream,
                identity: session.identity,
                member: session.locus.member,
                offset: session.locus.offset,
                length: session.locus.length,
            },
            FactCause::Existing(before.residency.position),
        )
        .expect("session declaration fact builds");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane: seat.lane,
                occupant: seat.occupant,
                epoch,
                group,
            },
            FactCause::Pending(declared),
        )
        .expect("lane-started fact builds");
    write.residency_head(started, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("session declaration commits"),
        CommitOutcome::Committed(_)
    ));
}

fn claim_expected(answer: &BoardAnswer, task: &TaskHandle, seat: &Seat) -> Vec<Witness> {
    let heads = at(answer, task);
    vec![
        statement_witness(heads),
        workflow_witness(heads),
        claim_witness(heads),
        Witness::Eligibility {
            rule: "next".parse().expect("fixture allocator rule"),
        },
        residency(answer, seat),
    ]
}

fn clean(root: &Path, bus: Bus) {
    bus.verify().expect("fixture authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn add_born(bus: &Bus, epoch: CanonicalUuid, born: wire::TaskStamp, title: &str) -> TaskHandle {
    let task = TaskHandle::for_project("board.identity", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &TaskAdd {
            title: title.to_owned(),
            project: Some("board.identity".to_owned()),
            description: None,
            acceptance: Vec::new(),
            origin: Some("fixture:identity".parse().expect("fixture origin")),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
        },
    ));
    task
}

fn creation_fact_origin(bus: &Bus, task: &TaskHandle) -> wire::Origin {
    let born = task
        .born()
        .expect("fixture handle carries an inception stamp");
    bus.fact_snapshot()
        .expect("creation facts read")
        .facts()
        .iter()
        .find(|fact| {
            fact.fact.kind() == wire::FactKind::TaskAdded
                && fact
                    .fact
                    .body()
                    .get("task")
                    .and_then(serde_json::Value::as_str)
                    == Some(born.as_str())
        })
        .unwrap_or_else(|| panic!("task {task} has one creation fact"))
        .origin
        .clone()
}

#[test]
fn native_task_handles_use_the_predecessor_stamp_alphabet() {
    const PREDECESSOR_ALPHABET: &[u8; 52] = b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";

    for (value, expected) in PREDECESSOR_ALPHABET.iter().copied().enumerate() {
        let handle = mint_handle_at(Some("task.identity"), "ignored", value as i64);
        let (_, stamp) = handle
            .as_str()
            .split_once('-')
            .expect("native handle carries a stem and stamp");
        assert_eq!(stamp.len(), steer::Key::WIDTH);
        assert!(
            stamp
                .bytes()
                .all(|byte| PREDECESSOR_ALPHABET.contains(&byte)),
            "{stamp:?} is outside the predecessor task alphabet"
        );
        assert_eq!(stamp.as_bytes()[steer::Key::WIDTH - 1], expected);
    }
}

/// Shape CASE 147 / INVARIANTS ID-02: lexical identity order is inception order.
#[test]
fn plain_string_stamp_and_handle_order_is_inception_order() {
    let millis = [2_705_i64, 51, 2_704, 52, 53];
    let mut stamps = millis
        .into_iter()
        .map(|millis| wire::TaskStamp::for_millis(millis).expect("fixture inception"))
        .collect::<Vec<_>>();
    stamps.sort();
    let expected = [51_i64, 52, 53, 2_704, 2_705]
        .into_iter()
        .map(|millis| wire::TaskStamp::for_millis(millis).expect("fixture inception"))
        .collect::<Vec<_>>();
    assert_eq!(stamps, expected);

    let mut handles = millis
        .into_iter()
        .map(|millis| {
            TaskHandle::for_project(
                "task.identity",
                &wire::TaskStamp::for_millis(millis).expect("fixture inception"),
            )
        })
        .collect::<Vec<_>>();
    handles.sort();
    assert_eq!(
        handles
            .iter()
            .map(|handle| handle.born().expect("sorted handle stamp"))
            .collect::<Vec<_>>(),
        expected
    );
}

/// Shape CASE 148 / INVARIANTS ID-03: every creation route advances collisions.
#[test]
fn same_millisecond_adds_followups_and_documents_receive_ordered_distinct_stamps() {
    let alphabet = (0..52_i64)
        .map(|millis| {
            wire::TaskStamp::for_millis(millis)
                .expect("alphabet fixture inception")
                .as_str()
                .as_bytes()[wire::TaskStamp::WIDTH - 1]
        })
        .collect::<Vec<_>>();
    assert!(alphabet.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        alphabet.iter().all(|byte| !b"AEIOUaeiou".contains(byte)),
        "task stamp alphabet must remain vowel-free"
    );

    let (root, bus, epoch, _seats) = fixture("identity-collision", 0);
    let candidate_millis = 90_000_i64;
    let first_stamp =
        wire::TaskStamp::for_millis(candidate_millis).expect("first fixture inception");
    add_born(
        &bus,
        epoch,
        first_stamp.clone(),
        "First same-millisecond task",
    );

    let after_first = board(&bus, epoch, Actor::Operator);
    let second_stamp = allocate_inception(&after_first, candidate_millis);
    assert_eq!(
        second_stamp,
        wire::TaskStamp::for_millis(candidate_millis + 1).expect("second fixture inception")
    );
    add_born(
        &bus,
        epoch,
        second_stamp.clone(),
        "Second same-millisecond task",
    );

    let after_second = board(&bus, epoch, Actor::Operator);
    let followups = allocate_inceptions(&after_second, candidate_millis, 3);
    let expected = (2..5)
        .map(|offset| {
            wire::TaskStamp::for_millis(candidate_millis + offset)
                .expect("advanced fixture inception")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        followups, expected,
        "review follow-up planning advances +1 ms"
    );

    let preview = board::Preview::classify("# Goal\n\n- First node\n- Second node\n")
        .expect("document collision fixture classifies");
    let document = document_handles(preview.document(), &after_second, candidate_millis);
    assert_eq!(
        document
            .iter()
            .map(|handle| handle.born().expect("document handle stamp"))
            .collect::<Vec<_>>(),
        expected,
        "authored-document creation advances over the same occupied instants"
    );
    clean(&root, bus);
}

#[test]
fn a_nonfinal_review_phase_prints_the_transition_it_can_accept() {
    let (root, bus, epoch, seats) = fixture("nonfinal-review-next-action", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-review01",
        "Advance through a nonfinal review phase",
        Some("review,verify"),
    );
    let available = board(&bus, epoch, lane.clone());
    let answer = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        claim_expected(&available, &task, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
        },
    ));
    let CommandAnswer::Claim(claim) = answer else {
        panic!("claim win returned another answer shape");
    };
    assert_eq!(
        claim.next.as_deref(),
        Some("spice task done"),
        "review is printable only when task review can settle the final phase",
    );
    clean(&root, bus);
}

#[test]
fn task_status_counts_one_snapshot_and_task_add_retains_authored_context() {
    let (root, bus, epoch, _) = fixture("task-bootstrap", 1);
    let born = wire::TaskStamp::for_millis(8_001).expect("fixture task stamp");
    let task = TaskHandle::for_project("task.cli", &born);
    let origin: wire::Origin = "ack:1kLGp2NJ".parse().expect("fixture origin");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            title: "Carry native bootstrap context".to_owned(),
            project: Some("task.cli".to_owned()),
            description: Some("The agent needs the complete handoff".to_owned()),
            acceptance: vec![
                "Status reports board counts".to_owned(),
                "Creation provenance survives".to_owned(),
            ],
            origin: Some(origin.clone()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
        },
    ));
    add(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-review001",
        "Review native bootstrap",
        Some("review"),
    );

    let answer = board(&bus, epoch, Actor::Operator);
    let statement = &at(&answer, &task).statement.body;
    assert_eq!(
        statement.description.as_deref(),
        Some("The agent needs the complete handoff")
    );
    assert_eq!(
        statement.acceptance,
        [
            "Status reports board counts",
            "Creation provenance survives"
        ]
    );
    assert_eq!(statement.provenance, origin);
    assert_eq!(creation_fact_origin(&bus, &task), origin);
    assert_eq!(at(&answer, &task).origin_thread, "operator");

    let status = task_status(&answer).expect("validated board yields status");
    for expected in [
        "claim -",
        "actor operator",
        concat!(
            "filter ( project:agent.operator.task or origin_thread.is:operator or ",
            "( project:board or project:rim or project:serve or project:task ) )"
        ),
        "public task project depth 2..3 dotted segments",
        "active 0",
        "ready 1",
        "review 1",
        "blocked 0",
        "waiting 0",
        "stale 0",
        "oops 0",
    ] {
        assert!(status.lines().any(|line| line == expected), "{status}");
    }
    clean(&root, bus);
}

#[test]
fn task_list_preserves_scope_status_project_and_newest_limit_semantics() {
    let (root, bus, epoch, _) = fixture("task-list", 0);
    let (answer, first, descendant, waiting) = task_list_fixture(&bus, epoch);
    assert_task_list_queries(&answer, &first, &descendant, &waiting);
    clean(&root, bus);
}

#[test]
fn shortened_task_list_names_original_and_withheld_row_counts() {
    let (root, bus, epoch, _) = fixture("task-list-shortened", 0);
    let (answer, _, _, _) = task_list_fixture(&bus, epoch);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(2),
            status: None,
        },
    );

    assert!(
        listed
            .lines()
            .any(|line| line == "limited --limit 2 original 4 withheld 2"),
        "{listed}"
    );
    clean(&root, bus);
}

#[test]
fn complete_task_list_emits_no_withheld_notice() {
    let (root, bus, epoch, _) = fixture("task-list-complete", 0);
    let (answer, _, _, _) = task_list_fixture(&bus, epoch);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(4),
            status: None,
        },
    );

    assert!(!listed.contains("withheld"), "{listed}");
    clean(&root, bus);
}

#[test]
fn default_task_list_selects_each_complete_actor_route_branch_only() {
    let (root, bus, epoch, _) = fixture("task-list-actor-route", 0);
    let public = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-route0001",
        "Public route row",
        "task.cli",
        Some("todo"),
    );
    let mut answer = board(&bus, epoch, Actor::Operator);
    let template = at(&answer, &public).clone();
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == public)
        .expect("public route row")
        .origin_thread = "someone_else".to_owned();

    let mut actor_private = template.clone();
    actor_private.statement.body.born =
        wire::TaskStamp::for_millis(8_002).expect("fixture task stamp");
    actor_private.statement.body.project = answer.actor_route.project.clone();
    actor_private.statement.body.handle = TaskHandle::for_project(
        &actor_private.statement.body.project,
        &actor_private.statement.body.born,
    );
    actor_private.statement.body.provenance = "task:0000002z"
        .parse()
        .expect("actor-private fixture origin");
    actor_private.origin_thread = "someone_else".to_owned();
    actor_private.presentation_title = "Actor-private route row".to_owned();

    let mut origin_thread = template.clone();
    origin_thread.statement.body.born =
        wire::TaskStamp::for_millis(8_003).expect("fixture task stamp");
    origin_thread.statement.body.project = "external.route".to_owned();
    origin_thread.statement.body.handle = TaskHandle::for_project(
        &origin_thread.statement.body.project,
        &origin_thread.statement.body.born,
    );
    origin_thread.statement.body.provenance = "task:0000002B"
        .parse()
        .expect("origin-thread fixture provenance");
    origin_thread.origin_thread = answer.actor_route.origin_thread.clone();
    origin_thread.presentation_title = "Origin-thread route row".to_owned();

    let mut outside = template;
    outside.statement.body.born = wire::TaskStamp::for_millis(8_004).expect("fixture task stamp");
    outside.statement.body.project = "agent.someone_else.task".to_owned();
    outside.statement.body.handle = TaskHandle::for_project(
        &outside.statement.body.project,
        &outside.statement.body.born,
    );
    outside.statement.body.provenance =
        "task:0000002C".parse().expect("outside fixture provenance");
    outside.origin_thread = "someone_else".to_owned();
    outside.presentation_title = "Outside route row".to_owned();

    let included = [
        public.clone(),
        actor_private.statement.body.handle.clone(),
        origin_thread.statement.body.handle.clone(),
    ];
    let excluded = outside.statement.body.handle.clone();
    answer.tasks.extend([actor_private, origin_thread, outside]);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    for included in included {
        assert!(
            listed.contains(included.as_str()),
            "missing {included}: {listed}"
        );
    }
    assert!(!listed.contains(excluded.as_str()), "{listed}");
    assert!(listed.starts_with(concat!(
        "scope actor-route filter ",
        "( project:agent.operator.task or origin_thread.is:operator or ",
        "( project:board or project:rim or project:serve or project:task ) )\n"
    )));
    let status = task_status(&answer).expect("planted actor-route rows validate");
    assert!(status.lines().any(|line| line == "ready 3"), "{status}");
    assert!(
        status.lines().any(|line| {
            line == concat!(
                "filter ( project:agent.operator.task or origin_thread.is:operator or ",
                "( project:board or project:rim or project:serve or project:task ) )"
            )
        }),
        "{status}"
    );
    clean(&root, bus);
}

#[test]
fn actor_route_projects_the_current_lane_session_identity() {
    let (root, bus, epoch, seats) = fixture("actor-route-session", 1);
    let answer = board(&bus, epoch, Actor::Operator);
    let mut residency = answer.residency.body;
    let identity = uuid(9_001);
    let row = residency.lanes.first_mut().expect("fixture lane");
    let occupant = row.occupant.expect("fixture occupant");
    let legacy = actor_route(&Actor::Lane(seats[0].lane), &residency)
        .expect("a legacy occupied seat names its occupant route");
    assert_eq!(legacy.actor, occupant.get().simple().to_string());

    let row = residency.lanes.first_mut().expect("fixture lane");
    row.session = Some(wire::LaneSession {
        lane: row.lane,
        occupant,
        worktree: row.worktree.clone(),
        vendor: row.vendor.clone(),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: identity.to_string(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    });

    let route = actor_route(&Actor::Lane(seats[0].lane), &residency)
        .expect("current session identity names a route");
    let actor = identity.get().simple().to_string();
    assert_eq!(route.actor, actor);
    assert_eq!(route.project, format!("agent.{actor}.task"));
    assert_eq!(route.origin_thread, actor);

    residency.lanes[0]
        .session
        .as_mut()
        .expect("fixture session")
        .identity = "session-id-with-hyphens".to_owned();
    assert!(
        actor_route(&Actor::Lane(seats[0].lane), &residency).is_err(),
        "a current malformed session route must fail instead of falling back"
    );
    clean(&root, bus);
}

include!("tests/late.rs");

// Lane lifecycle, review, and document tests share this fixture harness.
// Their execution seam lives separately so each source stays reviewable.
mod resolution;
use resolution::*;

// Review diagrams join durable lifecycle facts, rather than current assignment
// heads, through the same command surface operators use.
mod review_graph;

// The closing boundaries the completion and review transactions record.
// Kept beside the resolution seam because they drive its helpers, and
// separate from it because each fact is pinned on its own.
mod closing_boundary;

// The hidden-visibility tests and the two helpers only they use. They share
// this module's fixture harness, so the seam is the law under test rather
// than the scaffolding.
mod hidden;
use hidden::*;

// Statement correction and its refusals are one bounded scenario seam, so
// this file stays inside the structural review bound as it grows.
mod edit;
use edit::*;

// Withdrawal, its refusals, and what a withdrawn row releases are one
// bounded scenario seam, kept beside the correction seam for the same
// structural reason.
mod delete;
