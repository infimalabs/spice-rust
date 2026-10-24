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

mod board_view;
mod graph_view;
mod groups;
mod origin;
mod planning;
mod preparation;
mod read_scope;
mod report;
mod room;
mod task_list;
mod task_listing;
mod taste_review;
mod team_history;

// The order in which one claim's preconditions refuse is documented as a
// contract on `ClaimGrant`, and a caller sees only the first refusal, so it
// is pinned apart from the scenarios that drive a claim that succeeds.
mod claim_grant;
mod creation_context;

fn at<'a>(answer: &'a BoardAnswer, task: &TaskHandle) -> &'a BoardTask {
    task_heads(answer, task).expect("fixture task is visible")
}

/// The workflow status the newest `task::reviewed` fact for `task` recorded.
fn latest_review_status(bus: &Bus, task: &TaskHandle) -> body::TaskCompletionStatus {
    let born = task.born().expect("fixture handle carries a birth stamp");
    bus.fact_snapshot()
        .expect("review facts read")
        .facts()
        .iter()
        .rev()
        .find_map(|envelope| {
            envelope
                .fact
                .read::<body::TaskReviewed>()
                .filter(|reviewed| reviewed.task == born)
        })
        .expect("the task was reviewed")
        .status
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
        launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
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
                source: identity,
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
                fresh_session: None,
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

fn seed_creation_transcript(bus: &Bus, lane: CanonicalUuid) -> body::TaskCreationContext {
    seed_creation_stream(bus, lane, body::TASK_CREATION_CONTEXT_STREAM)
}

fn seed_creation_stream(bus: &Bus, lane: CanonicalUuid, stream: &str) -> body::TaskCreationContext {
    let vendor: wire::Name = "omp".parse().expect("fixture vendor");
    let source = uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed));
    let next = body::TranscriptPosition {
        member: 1,
        offset: 128,
    };
    let head = wire::TranscriptSourceHead {
        lane,
        vendor: vendor.clone(),
        stream: stream.to_owned(),
        source,
        next,
    };
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("lane:{lane}").parse().expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("current fixture instant"),
        Producer::ResidentRecognition,
    );
    let ingested = write
        .fact(
            body::JournalIngested {
                lane,
                vendor,
                stream: stream.to_owned(),
                source,
                supersedes: None,
                from: body::TranscriptPosition {
                    member: 1,
                    offset: 0,
                },
                to: next,
                usage: Vec::new(),
                non_usage_records: 1,
                residue_records: 0,
            },
            FactCause::Root,
        )
        .expect("fixture ingestion fact builds");
    write.transcript_source_head(ingested, move |_| head.clone());
    assert!(matches!(
        bus.commit(write).expect("fixture transcript commits"),
        CommitOutcome::Committed(_)
    ));
    body::TaskCreationContext::Transcript {
        session: String::new(),
        boundary: wire::TranscriptSourceHead {
            lane,
            vendor: "omp".parse().expect("fixture vendor"),
            stream: stream.to_owned(),
            source,
            next,
        },
        creator_team: None,
    }
}

fn authorize_lane_creation(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
) -> body::TaskCreationContext {
    let identity = uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed));
    let group = u32::try_from(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed))
        .expect("fixture group fits u32");
    declare_session(bus, epoch, seat, identity, group);
    let mut context = seed_creation_transcript(bus, seat.lane);
    let body::TaskCreationContext::Transcript {
        session,
        creator_team,
        ..
    } = &mut context
    else {
        unreachable!("fixture transcript helper returns transcript authority")
    };
    *session = identity.to_string();
    *creator_team = Some(seat.team);
    context
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
            preparation: None,
            intent: None,
            title: title.to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.identity".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    task
}

fn creation_fact_origin(bus: &Bus, task: &TaskHandle) -> wire::Origin {
    let born = task.born().expect("fixture handle carries a birth stamp");
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
        holder.lines().any(|line| line
            == format!(
                "claim {held} phase:{} rolling until {}",
                at(&claimed, &held).workflow.body.phase,
                lease.body.expiry
            )),
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
    let (alpha, beta, _) = seed_edit_tasks(&bus, epoch);
    exercise_deferral_staleness(&bus, epoch, &alpha);
    add_dependency(&bus, epoch, &alpha, &beta);
    let graph = assert_graph_handout_and_ledger(&bus, epoch, &alpha);
    assert_render_form_parity(&graph);
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
            &allocation_subscription(&available.residency.body, &lane)
                .expect("fixture lane subscription"),
            available.allocation_band,
            available.recent_cell.as_ref(),
            &available.environment(),
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
            &allocation_subscription(&available.residency.body, &lane)
                .expect("fixture lane subscription"),
            available.allocation_band,
            available.recent_cell.as_ref(),
            &available.environment(),
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
    for recover in ["recover", RECOVER_STEAL_RULE] {
        let recover: Name = recover.parse().expect("recovery rule");
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
                matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == reason),
                "invalid {recover:?} identity did not refuse as {reason}: {refused:?}",
            );
        }
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
fn recovery_foreign_claim_then_free_is_explicitly_restorable() {
    let (root, bus, epoch, seats) = fixture("recover-foreign-free", 2);
    let recovering = Actor::Lane(seats[0].lane);
    let foreign = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECOVER-foreign",
        "Foreign claim moved the recovery head",
        Some("todo"),
    );
    let available = board(&bus, epoch, recovering.clone());
    let transaction = "c".repeat(40);
    let sealed = recovery_preflight(&available, &task)
        .expect("ready task admits recovery preflight")
        .seal(GitBoundary {
            head: "synchronized-head".to_owned(),
            integrated: "synchronized-head".to_owned(),
            upstream: Some("origin/main".to_owned()),
            upstream_head: Some("synchronized-head".to_owned()),
            local_commits: 0,
        });

    claim_task(&bus, epoch, &seats[1], &foreign, &task);
    let held = board(&bus, epoch, foreign);
    let heads = at(&held, &task);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Lane(seats[1].lane),
        CommandKind::ClaimFree,
        vec![
            claim_witness(heads),
            Witness::Lease {
                task: heads.statement.body.born.clone(),
                head: heads.lease.as_ref().expect("foreign lease").position,
            },
            residency(&held, &seats[1]),
        ],
        &TaskRef { task: task.clone() },
    ));

    let moved = board(&bus, epoch, recovering);
    assert!(matches!(
        recovery_board_state(&moved, &task, &transaction, &sealed),
        RecoveryBoardState::Restorable(_)
    ));
    assert_eq!(
        at(&moved, &task)
            .claim
            .as_ref()
            .expect("foreign claim head remains")
            .body
            .disposition,
        ClaimDisposition::Freed,
    );
    clean(&root, bus);
}

#[test]
fn claims_warn_on_shared_statement_terms_not_shared_project_stem() {
    let (root, bus, epoch, seats) = fixture("claim-overlap", 3);
    let first = add_project_with_acceptance(
        &bus,
        epoch,
        "MOSAIC-1kKQTw1V",
        "App lacks the mosaic lane interface",
        "board.alpha",
        Some("todo"),
        &["The watchband mosaic uses per-lane cards and stays stable"],
    );
    let overlapping = add_project_with_acceptance(
        &bus,
        epoch,
        "FIDELIT-1kKRSfyV",
        "Rebuild the lane watchband and message mosaic",
        "rim.beta",
        Some("todo"),
        &["The mosaic watchband keeps focused-agent state visible"],
    );
    let same_stem = add_project_with_acceptance(
        &bus,
        epoch,
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
        overlap_answer.warnings.as_ref(),
        &[wire::intent::ClaimWarning {
            task: first,
            lane: seats[0].lane,
            surface: vec![
                "lane".to_owned(),
                "mosaic".to_owned(),
                "watchband".to_owned()
            ],
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
    *answer
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
    let stolen = assert_committed(issue(
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
    let CommandAnswer::Claim(stolen) = stolen else {
        panic!("steal returned another answer shape: {stolen:?}");
    };
    assert!(!stolen.nothing_stolen, "a held claim was displaced");

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
fn stealing_takes_an_assigned_review_after_the_previous_holder_freed_it() {
    let (root, bus, epoch, seats) = fixture("steal-review", 2);
    let author = Actor::Lane(seats[0].lane);
    let reviewer = Actor::Operator;
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "STEAL-r01",
        "Freed holder review",
        Some("todo,review"),
    );
    advance_task_to_review(&bus, epoch, &seats[0], &author, &task);

    let operator_board = board(&bus, epoch, Actor::Operator);
    let operator_heads = at(&operator_board, &task);
    let capture: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::ClaimWin,
        vec![
            statement_witness(operator_heads),
            workflow_witness(operator_heads),
            claim_witness(operator_heads),
            Witness::Eligibility {
                rule: capture.clone(),
            },
            residency(&operator_board, &seats[0]),
        ],
        &ClaimWin {
            rule: capture,
            recovery: None,
        },
    ));
    let assigned = board(&bus, epoch, Actor::Operator);
    let assigned_heads = at(&assigned, &task);
    assert_eq!(
        assigned_heads.workflow.body.status.reviewer(),
        Some(&Actor::Operator)
    );
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::ClaimFree,
        vec![
            claim_witness(assigned_heads),
            Witness::Lease {
                task: assigned_heads.statement.body.born.clone(),
                head: assigned_heads.lease.as_ref().expect("lease").position,
            },
            residency(&assigned, &seats[0]),
        ],
        &TaskRef { task: task.clone() },
    ));

    let steal_board = board(&bus, epoch, reviewer.clone());
    let steal_heads = at(&steal_board, &task);
    assert_eq!(steal_heads.workflow.body.status.reviewer(), None);
    assert_eq!(
        steal_heads.claim.as_ref().expect("claim").body.disposition,
        ClaimDisposition::Freed
    );
    let steal: Name = "steal".parse().expect("steal rule");
    let stolen = assert_committed(issue(
        &bus,
        epoch,
        reviewer.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(steal_heads),
            workflow_witness(steal_heads),
            claim_witness(steal_heads),
            Witness::Eligibility {
                rule: steal.clone(),
            },
            residency(&steal_board, &seats[1]),
        ],
        &ClaimWin {
            rule: steal,
            recovery: None,
        },
    ));
    let CommandAnswer::Claim(stolen_answer) = stolen else {
        panic!("steal returned another answer shape: {stolen:?}");
    };
    assert!(
        stolen_answer.nothing_stolen,
        "a freed claim has no holder to displace"
    );

    let after = board(&bus, epoch, reviewer.clone());
    let stolen = at(&after, &task);
    assert_eq!(stolen.workflow.body.status.reviewer(), Some(&reviewer));
    let claim = stolen.claim.as_ref().expect("claim");
    assert_eq!(claim.body.actor, reviewer);
    assert_eq!(claim.body.disposition, ClaimDisposition::Active);
    assert_review_assignment_fact(&bus, &after, &task, &reviewer);
    clean(&root, bus);
}

#[test]
fn stealing_claims_a_task_nobody_holds_and_warns_nothing_was_stolen() {
    let (root, bus, epoch, seats) = fixture("steal-unheld", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "STEAL-free01",
        "Never claimed",
        Some("todo"),
    );

    let available = board(&bus, epoch, lane.clone());
    let heads = at(&available, &task);
    assert!(heads.claim.is_none(), "the task starts unclaimed");
    let steal: Name = "steal".parse().expect("steal rule");
    let won = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility {
                rule: steal.clone(),
            },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule: steal,
            recovery: None,
        },
    ));
    let CommandAnswer::Claim(won) = won else {
        panic!("steal returned another answer shape: {won:?}");
    };
    assert!(won.nothing_stolen, "nobody held the task");

    let after = board(&bus, epoch, lane.clone());
    let claim = at(&after, &task).claim.as_ref().expect("claim");
    assert_eq!(claim.body.actor, lane);
    assert_eq!(claim.body.disposition, ClaimDisposition::Active);
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
// A review whose reviewer lapsed must reach the next lane; the lapse releases
// the assignment and the allocator ignores one a lapse left behind.
mod lapsed_review;

// The closing boundaries the completion and review transactions record.
// Kept beside the resolution seam because they drive its helpers, and
// separate from it because each fact is pinned on its own.
mod closing_boundary;

// The hidden-visibility tests and the two helpers only they use. They share
// this module's fixture harness, so the seam is the law under test rather
// than the scaffolding.
mod hidden;
use hidden::*;

// Drafts are prospective work outside every execution rail until the same
// explicit file transition admits them as ordinary work.
mod draft_filing;

// Unexpected-behavior intake is ordinary Plan preparation under one typed,
// repository-owned Design; exact candidate decisions are its only release.
mod oops_design;

// Statement correction and its refusals are one bounded scenario seam, so
// this file stays inside the structural review bound as it grows.
mod edit;
use edit::*;
// The pitch every statement leads with: its mechanical refusals, the hold its
// prose findings raise, and the revision an authored change advances.
mod pitch;
// Whole-text replacement of a description or acceptance through the modify
// intent, admitted only over the digest of the text its writer read.
mod task_text;
// Declared Plan and Design intent: durable across phases and corrections,
// moved only by its own witnessed edit.
mod intent;

// Claimed dependency edits carry possession through one atomic statement-head
// replacement and keep the complete graph acyclic.
mod dependencies;
use dependencies::{dependency_witnesses, todo_task};

// The empty-drain readout a steered lane receives measures the parked board
// with the same split task status reports, so both are pinned on one board.
mod drain_readout;
mod task_creation_flow;
mod wake_claim;
// A completion that enters a continuing phase without publishing mints the
// next phase's claim for the completing seat inside the same write.
mod carried_claim;

// Withdrawal, its refusals, and what a withdrawn row releases are one
// bounded scenario seam, kept beside the correction seam for the same
// structural reason.
mod delete;

// Operator closure shares withdrawal's settlement seam but completes the row.
mod close;

// One selected task's append-only notes are a bounded read contract, separate
// from the board summary and its allocation scenarios.
mod detail;
mod relations;
mod timeline;
