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
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &task, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
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

#[test]
fn lane_task_origin_thread_comes_from_its_durable_creation_session() {
    let (root, bus, epoch, seats) = fixture("task-origin-session", 1);
    let first_identity = uuid(9_101);
    declare_session(&bus, epoch, &seats[0], first_identity, 101);
    let first = add(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        "CLI-origin001",
        "Retain the creating session",
        Some("todo"),
    );
    let second_identity = uuid(9_102);
    declare_session(&bus, epoch, &seats[0], second_identity, 102);
    let second = add(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        "CLI-origin002",
        "Retain the successor session",
        Some("todo"),
    );
    let answer = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let first_actor = first_identity.get().simple().to_string();
    let second_actor = second_identity.get().simple().to_string();
    assert_eq!(answer.actor_route.origin_thread, second_actor);
    assert_eq!(at(&answer, &first).origin_thread, first_actor);
    assert_eq!(at(&answer, &second).origin_thread, second_actor);
    clean(&root, bus);
}

fn task_list_fixture(
    bus: &Bus,
    epoch: CanonicalUuid,
) -> (BoardAnswer, TaskHandle, TaskHandle, TaskHandle) {
    let first = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0001",
        "First task route",
        "task.cli",
        Some("todo"),
    );
    let descendant = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0002",
        "Descendant task route",
        "task.cli.deep",
        Some("todo"),
    );
    let waiting = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0003",
        "Deferred serve route",
        "serve.ops",
        Some("todo"),
    );
    let completed = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0004",
        "Completed board route",
        "board.test",
        Some("todo"),
    );
    let mut answer = board(bus, epoch, Actor::Operator);
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == first)
        .expect("first row")
        .statement
        .body
        .tags
        .push("priority:medium".to_owned());
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == waiting)
        .expect("waiting row")
        .workflow
        .body
        .deferral = Some(fixture_instant("fixture deferral"));
    let completed_row = answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == completed)
        .expect("completed row");
    completed_row.workflow.body.status = "completed".to_owned();
    completed_row.workflow.body.completion = Some(fixture_instant("fixture completion"));
    (answer, first, descendant, waiting)
}

fn assert_task_list_queries(
    answer: &BoardAnswer,
    first: &TaskHandle,
    descendant: &TaskHandle,
    waiting: &TaskHandle,
) {
    let default = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    assert!(default.starts_with(concat!(
        "scope actor-route filter ",
        "( project:agent.operator.task or origin_thread.is:operator or ",
        "( project:board or project:rim or project:serve or project:task ) )\n"
    )));
    assert!(default.contains(&format!("{first} [todo] P:M task.cli First task route")));
    assert!(default.contains(&format!(
        "{descendant} [todo] task.cli.deep Descendant task route"
    )));
    assert!(!default.contains(waiting.as_str()), "{default}");

    let project = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: Some("task.cli".to_owned()),
            limit: None,
            status: Some(TaskListStatus::Pending),
        },
    );
    assert!(project.starts_with("scope explicit-project project:task.cli\n"));
    assert!(project.contains(first.as_str()));
    assert!(project.contains(descendant.as_str()));
    assert!(!project.contains(waiting.as_str()));

    let waiting_only = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: Some(TaskListStatus::Waiting),
        },
    );
    assert!(waiting_only.contains(waiting.as_str()));
    assert!(!waiting_only.contains(first.as_str()));

    let completed_only = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: Some(TaskListStatus::Completed),
        },
    );
    assert!(completed_only.contains("board.test Completed board route"));
    assert!(!completed_only.contains(first.as_str()));

    let newest_global = task_list(
        answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(1),
            status: None,
        },
    );
    let completed = answer
        .tasks
        .iter()
        .find(|task| task.presentation_title() == "Completed board route")
        .expect("completed fixture row");
    assert_eq!(
        newest_global,
        format!(
            concat!(
                "scope global --all\n",
                "limited --limit 1 original 4 withheld 3\n",
                "{} [done] board.test Completed board route"
            ),
            completed.statement.body.handle
        )
    );
}

fn fixture_instant(context: &str) -> Instant {
    "2030-04-12T18:44:09.271Z"
        .parse()
        .unwrap_or_else(|error| panic!("{context}: {error}"))
}

fn fixture_instant_after(at: Instant, seconds: i64) -> Instant {
    Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .expect("fixture instant remains in range"),
    )
    .expect("fixture instant is canonical")
}

/// A held task is named on the claim line and counted as active.
///
/// The snapshot above reports every claim field at rest, where a wrong
/// predicate and a right one agree: with nothing claimed, `claim -` and
/// `active 0` are what any filter produces. Bootstrap step three exists to
/// tell an agent which handle it already holds, so the reading that matters
/// is the one taken while a claim is live, and it has to distinguish the
/// principal asking from the other lanes holding rows of their own.
#[test]
fn task_status_names_the_claim_the_asking_principal_holds() {
    let (root, bus, epoch, seats) = fixture("task-bootstrap-claim", 2);
    let lane = Actor::Lane(seats[0].lane);
    let held = add(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-held00001",
        "Hold one native bootstrap row",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &held, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));

    let claimed = board(&bus, epoch, lane);
    let lease = at(&claimed, &held)
        .lease
        .as_ref()
        .expect("manual claim writes a lease");
    assert_eq!(
        lease.body.expiry,
        fixture_instant_after(lease.body.start, 60 * 60),
        "manual task claims keep the predecessor one-hour lease",
    );
    assert_eq!(
        lease.body.maximum,
        fixture_instant_after(lease.body.start, 2 * 60 * 60),
        "manual task claims keep a later renewal cap for task reclaim",
    );

    let holder = task_status(&claimed).expect("holder reads status");
    assert!(
        holder.lines().any(|line| line == format!("claim {held}")),
        "{holder}"
    );
    assert!(holder.lines().any(|line| line == "active 1"), "{holder}");

    // A peer's claim is still active work on the board, but it is not this
    // principal's, so the same row has to count without being named.
    let other =
        task_status(&board(&bus, epoch, Actor::Lane(seats[1].lane))).expect("peer reads status");
    assert!(other.lines().any(|line| line == "claim -"), "{other}");
    assert!(other.lines().any(|line| line == "active 1"), "{other}");
    clean(&root, bus);
}

#[test]
fn current_board_edits_are_guarded_deterministic_and_render_form_invariant() {
    let (root, bus, epoch, seats) = fixture("edits", 1);
    let initial = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        actor_seat(&initial).expect("sole operator seat"),
        (seats[0].lane, seats[0].occupant,)
    );
    let (alpha, beta, oops) = seed_edit_tasks(&bus, epoch);
    exercise_deferral_staleness(&bus, epoch, &alpha);
    add_dependency(&bus, epoch, &alpha, &beta);
    let graph = assert_graph_handout_and_ledger(&bus, epoch, &alpha);
    assert_render_form_parity(&graph);
    assert_oops_note_exists(&bus, &oops);
    clean(&root, bus);
}

#[test]
fn capture_claims_one_exact_eligible_non_allocator_task() {
    let (root, bus, epoch, seats) = fixture("capture", 1);
    let lane = Actor::Lane(seats[0].lane);
    let alpha = add(
        &bus,
        epoch,
        Actor::Operator,
        "CAP-alpha001",
        "Allocator winner",
        Some("todo"),
    );
    let beta = add(
        &bus,
        epoch,
        Actor::Operator,
        "CAP-beta0001",
        "Exact capture target",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_eq!(available.active_public_tasks().count(), 2);
    let entries = actor_entries(&available).expect("capture board entries are valid");
    assert_eq!(
        board::select(
            &entries,
            &lane,
            allocation_subscription(&available.residency.body, &lane)
                .expect("fixture lane subscription"),
            available.allocation_band,
            available.recent_cell.as_ref(),
        )
        .expect("allocator winner")
        .entry()
        .handle(),
        &alpha,
    );
    let heads = at(&available, &beta);
    let rule: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    ));

    let claimed = board(&bus, epoch, lane);
    assert!(at(&claimed, &alpha).claim.is_none());
    assert_eq!(
        at(&claimed, &beta)
            .claim
            .as_ref()
            .expect("capture claim")
            .body
            .disposition,
        ClaimDisposition::Active,
    );
    clean(&root, bus);
}

#[test]
fn exact_recovery_names_one_ready_task_and_journals_its_transaction() {
    let (root, bus, epoch, seats) = fixture("recover-exact", 1);
    let lane = Actor::Lane(seats[0].lane);
    let alpha = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-alpha1",
        "Allocator winner",
        Some("todo"),
    );
    let beta = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-beta01",
        "Exact recovery target",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    let entries = actor_entries(&available).expect("recovery board entries are valid");
    assert_eq!(
        board::select(
            &entries,
            &lane,
            allocation_subscription(&available.residency.body, &lane)
                .expect("fixture lane subscription"),
            available.allocation_band,
            available.recent_cell.as_ref(),
        )
        .expect("allocator winner")
        .entry()
        .handle(),
        &alpha,
    );
    let heads = at(&available, &beta);
    let rule: Name = "recover".parse().expect("recovery rule");
    let transaction = "a".repeat(40);
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: Some(transaction.clone()),
        },
    ));

    let claimed = board(&bus, epoch, lane);
    assert!(at(&claimed, &alpha).claim.is_none());
    let claim = at(&claimed, &beta)
        .claim
        .as_ref()
        .expect("recovery claim is active");
    assert_eq!(claim.body.recovery.as_deref(), Some(transaction.as_str()));
    clean(&root, bus);
}

#[test]
fn recovery_identity_is_admitted_only_by_the_recover_rule() {
    let (root, bus, epoch, seats) = fixture("recover-rule", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-rule01",
        "Rule-bound recovery",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    let heads = at(&available, &task);
    let rule: Name = "next".parse().expect("next rule");
    let refused = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: Some("a".repeat(40)),
        },
    );
    assert!(
        matches!(
            refused,
            Outcome::Refusal(ref refusal) if refusal.reason() == "git::recovery-rule-mismatch"
        ),
        "ordinary next admitted a recovery identity: {refused:?}",
    );
    let recover: Name = "recover".parse().expect("recovery rule");
    for (identity, reason) in [
        (None, "git::recovery-transaction-missing"),
        (Some("a".repeat(39)), "git::recovery-transaction-invalid"),
    ] {
        let refused = issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility {
                    rule: recover.clone(),
                },
                residency(&available, &seats[0]),
            ],
            &ClaimWin {
                rule: recover.clone(),
                recovery: identity,
            },
        );
        assert!(
            matches!(refused, Outcome::Refusal(ref refusal) if refusal.reason() == reason),
            "invalid recovery identity did not refuse as {reason}: {refused:?}",
        );
    }
    assert!(
        at(&board(&bus, epoch, Actor::Lane(seats[0].lane)), &task)
            .claim
            .is_none()
    );
    clean(&root, bus);
}

#[test]
fn recovery_restart_classifies_only_its_exact_claim_as_postclaim() {
    let (root, bus, epoch, seats) = fixture("recover-restart", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-restart",
        "Restart-visible recovery",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    let transaction = "b".repeat(40);
    let boundary = GitBoundary {
        head: "synchronized-head".to_owned(),
        integrated: "synchronized-head".to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some("synchronized-head".to_owned()),
        local_commits: 0,
    };
    let sealed = recovery_preflight(&available, &task)
        .expect("ready task admits recovery preflight")
        .seal(boundary);
    assert!(matches!(
        recovery_board_state(&available, &task, &transaction, &sealed),
        RecoveryBoardState::PreClaim
    ));

    let heads = at(&available, &task);
    let rule: Name = "recover".parse().expect("recovery rule");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: Some(transaction.clone()),
        },
    ));
    let claimed = board(&bus, epoch, lane);
    assert!(matches!(
        recovery_board_state(&claimed, &task, &transaction, &sealed),
        RecoveryBoardState::PostClaim { .. }
    ));

    let mut impostor = claimed.clone();
    impostor
        .tasks
        .iter_mut()
        .find(|heads| heads.statement.body.handle == task)
        .expect("claimed task remains visible")
        .claim
        .as_mut()
        .expect("claimed task")
        .body
        .recovery = None;
    assert!(matches!(
        recovery_board_state(&impostor, &task, &transaction, &sealed),
        RecoveryBoardState::Ambiguous(_)
    ));
    clean(&root, bus);
}

#[test]
fn claims_warn_on_shared_acceptance_surface_not_shared_project_stem() {
    let (root, bus, epoch, seats) = fixture("claim-overlap", 3);
    let first = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "MOSAIC-1kKQTw1V",
        "Canvas lacks the mosaic lane interface",
        "board.alpha",
        Some("todo"),
        &["The watchband mosaic uses per-lane cards and stays stable"],
    );
    let overlapping = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "FIDELIT-1kKRSfyV",
        "Rebuild the lane watchband and message mosaic",
        "rim.beta",
        Some("todo"),
        &["The mosaic watchband keeps focused-agent state visible"],
    );
    let same_stem = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "MOSAIC-1kKQTw1W",
        "Audit unrelated board output",
        "board.gamma",
        Some("todo"),
        &["Export signed ledger receipts for audit and keep output stable"],
    );

    let first_answer = capture_for(&bus, epoch, &seats[0], &first);
    assert!(first_answer.warnings.is_empty());

    let overlap_answer = capture_for(&bus, epoch, &seats[1], &overlapping);
    assert_eq!(
        overlap_answer.warnings,
        [wire::intent::ClaimWarning {
            task: first,
            lane: seats[0].lane,
            surface: vec!["mosaic".to_owned(), "watchband".to_owned()],
        }]
    );

    let same_stem_answer = capture_for(&bus, epoch, &seats[2], &same_stem);
    assert!(same_stem_answer.warnings.is_empty());
    clean(&root, bus);
}

fn capture_for(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> wire::intent::ClaimAnswer {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let rule: Name = "capture".parse().expect("capture rule");
    let answer = assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, seat),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    ));
    let CommandAnswer::Claim(answer) = answer else {
        panic!("claim win returned another answer shape: {answer:?}");
    };
    answer
}

#[test]
fn stealing_takes_a_live_claim_at_once_where_capture_is_refused() {
    let (root, bus, epoch, seats) = fixture("steal", 2);
    let holder = Actor::Lane(seats[0].lane);
    let rival = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "STEAL-live01",
        "Held under a live lease",
        Some("todo"),
    );

    let available = board(&bus, epoch, holder.clone());
    let heads = at(&available, &task);
    let capture: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        &bus,
        epoch,
        holder.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility {
                rule: capture.clone(),
            },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule: capture,
            recovery: None,
        },
    ));

    let held = board(&bus, epoch, rival.clone());
    let heads = at(&held, &task);
    let witnesses = |rule: &Name| {
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&held, &seats[1]),
        ]
    };

    // Same board, same rival, same witnesses: only the rule differs, so a
    // steal that behaved like a capture would be refused right here.
    let capture: Name = "capture".parse().expect("capture rule");
    let declined = issue(
        &bus,
        epoch,
        rival.clone(),
        CommandKind::ClaimWin,
        witnesses(&capture),
        &ClaimWin {
            rule: capture,
            recovery: None,
        },
    );
    assert!(
        matches!(
            declined,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "claim::capture-ineligible"
        ),
        "capture took a held claim: {declined:?}",
    );

    let steal: Name = "steal".parse().expect("steal rule");
    assert_committed(issue(
        &bus,
        epoch,
        rival.clone(),
        CommandKind::ClaimWin,
        witnesses(&steal),
        &ClaimWin {
            rule: steal,
            recovery: None,
        },
    ));

    // The lease the holder still had did not delay the transfer, and the
    // ClaimWon naming the new holder is what tells the old one.
    let after = board(&bus, epoch, rival.clone());
    let taken = at(&after, &task).claim.as_ref().expect("stolen claim");
    assert_eq!(taken.body.actor, rival);
    assert_eq!(taken.body.disposition, ClaimDisposition::Active);
    assert_ne!(taken.body.actor, holder);
    clean(&root, bus);
}

#[test]
fn reclaim_renews_only_the_lane_owned_claim_and_survives_restart() {
    let (root, bus, epoch, seats) = fixture("reclaim", 1);
    let lane = Actor::Lane(seats[0].lane);
    let held_task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-held1",
        "Held recovery work",
        Some("todo"),
    );
    let available_task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-next1",
        "Next allocator work",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &held_task, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let held = board(&bus, epoch, lane.clone());
    let before = at(&held, &held_task).clone();
    let claim = before.claim.as_ref().expect("active claim");
    let lease = before.lease.as_ref().expect("active lease");
    let expected = vec![
        Witness::Claim {
            task: before.statement.body.born.clone(),
            head: Some(claim.position),
        },
        Witness::Lease {
            task: before.statement.body.born.clone(),
            head: lease.position,
        },
        residency(&held, &seats[0]),
    ];
    let after = renew_and_assert(&bus, epoch, &lane, &held_task, &before, expected);
    assert_reclaim_survives_restart(
        &root,
        bus,
        epoch,
        &lane,
        &held_task,
        &available_task,
        &after,
    );
}

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

// Parked `.oops` rows have one deliberate exit: exact capture may hold one
// for triage, and atomic wake promotion moves it into public work.
mod oops_promotion;

// Statement correction and its refusals are one bounded scenario seam, so
// this file stays inside the structural review bound as it grows.
mod edit;
use edit::*;

// Withdrawal, its refusals, and what a withdrawn row releases are one
// bounded scenario seam, kept beside the correction seam for the same
// structural reason.
mod delete;
