use std::collections::BTreeMap;

use board::{Entry, ProjectPolicy};
use bus::{Bus, CommitOutcome};
use journal::{FactCause, Journal, JournalWrite, PendingFact};
use lane::hold::Holds;
use steer::Ledger;
use wire::{
    Actor, CanonicalUuid, Instant, LaneResidency, Origin, Phase, Producer, ResidencyHead,
    ResidentState, TaskHandle, TaskHead, TaskStamp, WorkflowHead, body,
};

use super::materialization::Projection;
use super::{Available, Backoff, Resolution, Target, Trigger, project_targets, resolve};

mod deferred_exit;
mod exit_disposal;
mod reaped_seat;
mod trust_ice;
mod vendor_failure;

const PROJECT: &str = "lifecycle.work";
const TASK_MOMENT: &str = "1kKRRsDP";
const NEWER_TASK_MOMENT: &str = "1kKRRsDT";
const INVALID_TASK_MOMENT: &str = "1kKRRsDQ";

#[derive(Debug, PartialEq, Eq)]
struct SemanticState {
    residency: ResidencyHead,
    entries: Vec<Entry>,
    ledger: Ledger,
    ready_at: BTreeMap<TaskStamp, Instant>,
    holds: Holds,
    resolution: Resolution,
}

#[test]
fn late_refusal_rolls_back_the_complete_projection_and_retry_is_idempotent() {
    let (bus, projects, mut projection) = caught_up_fixture(0);
    let before = projection.clone();
    committed(
        bus.commit(invalid_late_suffix())
            .expect("late-invalid suffix commits to the durable fixture"),
    );
    let snapshot = bus.task_snapshot().expect("late-invalid snapshot reads");

    let first = projection
        .advance(&snapshot, &projects)
        .expect_err("unknown project refuses after the earlier steering fact");
    assert!(format!("{first:?}").contains("lifecycle::task-head-invalid"));
    assert_eq!(
        projection, before,
        "cursor, steering, entries, dependencies, subscriptions, fact instants, and ready times all roll back"
    );

    let retry = projection
        .advance(&snapshot, &projects)
        .expect_err("retrying the same suffix still refuses");
    assert!(format!("{retry:?}").contains("lifecycle::task-head-invalid"));
    assert_eq!(
        projection, before,
        "retry cannot duplicate the planned lane subscription or steering directive"
    );
    assert!(projection.ledger().directives().is_empty());

    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn one_fact_advancement_is_suffix_bounded_and_restart_equivalent_at_different_scales() {
    let small = one_fact_scenario(1);
    let large = one_fact_scenario(2_048);

    assert_eq!(small, large);
}

#[test]
fn org_policy_drives_every_member_while_terminal_seat_life_remains_an_override() {
    for (policy, expected) in [
        (wire::intent::TeamLife::Steer, Resolution::Idle),
        (
            wire::intent::TeamLife::Drive,
            Resolution::Wait {
                trigger: Trigger::AvailableWork,
                lane: lane(),
                deadline: "2030-04-12T18:47:01.271Z"
                    .parse()
                    .expect("starvation deadline"),
            },
        ),
        (
            wire::intent::TeamLife::Drain,
            Resolution::Attempt {
                trigger: Trigger::DrainQueue,
                lane: lane(),
            },
        ),
    ] {
        let (bus, _, projection) = policy_fixture(policy, false);
        let snapshot = bus.task_snapshot().expect("policy snapshot reads");
        let targets = project_targets(
            &snapshot,
            &projection,
            instant(50),
            &Holds::new(3),
            &board::NextConfig::default(),
        )
        .expect("team policy projects lifecycle targets");
        assert_eq!(
            resolve(&targets, &Backoff::default(), instant(50)),
            expected
        );
        bus.shutdown().expect("policy fixture bus stops");
    }

    let (bus, _, projection) = policy_fixture(wire::intent::TeamLife::Drain, true);
    let snapshot = bus
        .task_snapshot()
        .expect("terminal override snapshot reads");
    let targets = project_targets(
        &snapshot,
        &projection,
        instant(50),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("terminal override projects");
    assert!(
        targets.is_empty(),
        "terminal seat life suppresses team startup"
    );
    bus.shutdown().expect("terminal override bus stops");
}

/// A vacant lane seated alone under its own identity is not on a team.
///
/// Its configured policy stays durable, but only a team draws work under it:
/// the lane holds at Idle under every policy, exactly as a steered team does,
/// however much ready work the board carries.
#[test]
fn a_vacant_lane_without_a_team_projects_no_startup_under_any_policy() {
    for policy in [
        wire::intent::TeamLife::Steer,
        wire::intent::TeamLife::Drive,
        wire::intent::TeamLife::Drain,
    ] {
        let (bus, _, projection) = seated_policy_fixture(policy, false, lane());
        let snapshot = bus.task_snapshot().expect("unteamed snapshot reads");
        let targets = project_targets(
            &snapshot,
            &projection,
            instant(50),
            &Holds::new(3),
            &board::NextConfig::default(),
        )
        .expect("an unteamed lane projects lifecycle targets");
        assert_eq!(
            resolve(&targets, &Backoff::default(), instant(50)),
            Resolution::Idle,
            "{policy:?} starts no agent on a lane without a team",
        );
        bus.shutdown().expect("unteamed fixture bus stops");
    }
}

#[test]
fn drive_burst_uses_the_selected_winners_ready_instant() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    committed(
        bus.commit(ready_task_write(NEWER_TASK_MOMENT, instant(7)))
            .expect("newer ready candidate commits"),
    );
    let snapshot = bus.task_snapshot().expect("drive burst snapshot reads");
    projection
        .advance(&snapshot, &projects)
        .expect("drive burst projection advances");

    let targets = project_targets(
        &snapshot,
        &projection,
        instant(8),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("drive burst targets project");
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].available,
        Available::At(instant(4)),
        "the selected older winner settles from its own READY transition rather than the newer nonwinner"
    );
    assert!(matches!(
        resolve(&targets, &Backoff::default(), instant(8)),
        Resolution::Attempt {
            trigger: Trigger::AvailableWork,
            ..
        }
    ));
    bus.shutdown().expect("drive burst fixture bus stops");
}

#[test]
fn drain_ready_reentry_restarts_the_first_members_timer_deadline() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drain, false);
    committed(
        bus.commit(deferral_write(true, instant(5)))
            .expect("task deferral commits"),
    );
    projection
        .advance(
            &bus.task_snapshot().expect("deferred snapshot reads"),
            &projects,
        )
        .expect("deferred projection advances");
    committed(
        bus.commit(deferral_write(false, instant(7)))
            .expect("task wake commits"),
    );
    let snapshot = bus.task_snapshot().expect("woken snapshot reads");
    projection
        .advance(&snapshot, &projects)
        .expect("woken projection advances");

    let targets = project_targets(
        &snapshot,
        &projection,
        instant(8),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("woken Drain target projects");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].drain_queue, Available::At(instant(10)));
    assert_eq!(
        resolve(&targets, &Backoff::default(), instant(8)),
        Resolution::Wait {
            trigger: Trigger::DrainQueue,
            lane: lane(),
            deadline: instant(10),
        }
    );
    assert_eq!(
        bus.task_snapshot()
            .expect("post-resolution snapshot reads")
            .cursor(),
        snapshot.cursor(),
        "deadline projection writes no claim or lifecycle fact"
    );
    assert_eq!(
        resolve(&targets, &Backoff::default(), instant(10)),
        Resolution::Attempt {
            trigger: Trigger::DrainQueue,
            lane: lane(),
        }
    );
    bus.shutdown().expect("Drain re-entry fixture bus stops");
}

#[test]
fn synthesized_guidance_stays_pending_without_resurrecting_a_vacant_lane() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Steer, false);
    committed(
        bus.commit(reminder_write())
            .expect("taste reminder commits"),
    );
    let snapshot = bus.task_snapshot().expect("reminder snapshot reads");
    projection
        .advance(&snapshot, &projects)
        .expect("reminder projection advances");
    assert_eq!(projection.ledger().pending(lane()).len(), 1);
    assert_eq!(
        projected_resolution(&snapshot, &projection),
        Resolution::Idle,
        "resident-authored guidance remains answerable without becoming wake authority"
    );

    committed(
        bus.commit(operator_direction_write())
            .expect("operator direction commits"),
    );
    let snapshot = bus
        .task_snapshot()
        .expect("operator direction snapshot reads");
    projection
        .advance(&snapshot, &projects)
        .expect("operator direction projection advances");
    assert_eq!(
        projected_resolution(&snapshot, &projection),
        Resolution::Attempt {
            trigger: Trigger::PendingInbox,
            lane: lane(),
        },
        "genuine operator steering resurrects the vacant lane"
    );
    bus.shutdown().expect("guidance fixture bus stops");
}

fn projected_resolution(snapshot: &bus::TaskSnapshot, projection: &Projection) -> Resolution {
    let now = instant(50);
    let targets = project_targets(
        snapshot,
        projection,
        now,
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("guidance lifecycle targets project");
    resolve(&targets, &Backoff::default(), now)
}

fn reminder_write() -> JournalWrite {
    let at = instant(10);
    let origin = format!("resident:{}", epoch())
        .parse::<Origin>()
        .expect("fixture resident origin");
    let reminder = body::TasteReminded {
        directive: uuid(10),
        key: "1kKRRsDR".to_owned(),
        lane: lane(),
        taste: "polling".to_owned(),
        content: wire::Digest::compute(wire::Domain::TasteProse, b"poll until complete"),
        occupant: uuid(9),
        compaction: 1,
        locus: body::TranscriptLocus {
            source: Some(uuid(8)),
            member: 1,
            offset: 2,
            length: 3,
            event: 0,
            line: 1,
            column: 1,
        },
        text: "[taste:polling] Wait for the completion signal.".to_owned(),
        expires: instant(59),
    };
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        at,
        Producer::ResidentRecognition,
    );
    let reminded = write
        .fact(reminder.clone(), FactCause::Root)
        .expect("taste reminder fact builds");
    write.steering_head(reminded, move |position| {
        Ledger::reminded_head(&reminder, origin, at, position).expect("taste reminder head builds")
    });
    write
}

fn operator_direction_write() -> JournalWrite {
    let at = instant(11);
    let origin = format!("resident:{}", epoch())
        .parse::<Origin>()
        .expect("fixture resident origin");
    let direction = body::SteerSent {
        directive: uuid(11),
        fanout: Some(uuid(12)),
        key: "1kKRRsDS".to_owned(),
        lane: lane(),
        text: "Resume for this operator request.".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: instant(59),
    };
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let sent = write
        .fact(direction.clone(), FactCause::Root)
        .expect("operator direction fact builds");
    write.steering_head(sent, move |position| {
        Ledger::sent_head(&direction, origin, at, position).expect("operator direction head builds")
    });
    write
}

#[test]
fn drain_org_startup_projects_every_current_member() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drain, false);
    add_vacant_org_members(&bus, &[lane_at(2), lane_at(3)]);
    let snapshot = bus.task_snapshot().expect("expanded team snapshot reads");
    projection
        .advance(&snapshot, &projects)
        .expect("expanded team projection advances");

    let targets = project_targets(
        &snapshot,
        &projection,
        instant(50),
        &Holds::new(3),
        &board::NextConfig::default(),
    )
    .expect("expanded team targets project");
    assert_eq!(
        targets.iter().map(|target| target.lane).collect::<Vec<_>>(),
        vec![lane(), lane_at(2), lane_at(3)],
    );
    assert!(
        targets
            .iter()
            .all(|target| target.drain_queue == Available::At(instant(4)))
    );
    bus.shutdown().expect("expanded team bus stops");
}

fn one_fact_scenario(unrelated_history: usize) -> SemanticState {
    let (bus, projects, mut incremental) = caught_up_fixture(unrelated_history);
    let before = bus.task_snapshot().expect("admitted snapshot reads");
    let admitted = incremental.cursor();
    let mut holds = replay_holds(before.facts().iter());
    committed(bus.commit(hold_write()).expect("one-fact tail commits"));
    let snapshot = bus.task_snapshot().expect("advanced snapshot reads");
    assert_eq!(
        snapshot.facts().after(admitted).count(),
        1,
        "a caught-up lifecycle projection touches only the committed suffix"
    );
    for envelope in snapshot.facts().after(admitted) {
        absorb_hold(&mut holds, envelope);
    }
    assert_eq!(holds, replay_holds(snapshot.facts().iter()));

    incremental
        .advance(&snapshot, &projects)
        .expect("incremental projection advances");
    let mut restarted = Projection::new();
    restarted
        .advance(&snapshot, &projects)
        .expect("restart projection rebuilds");
    assert_eq!(
        incremental, restarted,
        "one-fact incremental state is byte-for-byte equivalent to restart state"
    );

    let now = instant(50);
    let targets = project_targets(
        &snapshot,
        &incremental,
        now,
        &holds,
        &board::NextConfig::default(),
    )
    .expect("wake targets project from incremental state");
    let resolution = resolve(&targets, &Backoff::default(), now);
    let residency = snapshot
        .residency()
        .expect("fixture residency head exists")
        .body()
        .clone();
    let state = SemanticState {
        residency,
        entries: incremental.entries().to_vec(),
        ledger: incremental.ledger().clone(),
        ready_at: incremental.ready_at().clone(),
        holds,
        resolution,
    };

    bus.shutdown().expect("fixture bus stops");
    state
}

fn caught_up_fixture(unrelated_history: usize) -> (Bus, ProjectPolicy, Projection) {
    let mut journal = Journal::in_memory().expect("fixture journal opens");
    journal.apply(seed_write()).expect("fixture task seeds");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let projects = projects();
    let mut projection = Projection::new();
    projection
        .advance(
            &bus.task_snapshot().expect("seed snapshot reads"),
            &projects,
        )
        .expect("seed projection builds");

    if unrelated_history > 0 {
        committed(
            bus.commit(unrelated_write("retained-history", unrelated_history))
                .expect("unrelated history commits"),
        );
        projection
            .advance(
                &bus.task_snapshot().expect("history snapshot reads"),
                &projects,
            )
            .expect("unrelated history advances");
    }
    (bus, projects, projection)
}

fn policy_fixture(
    policy: wire::intent::TeamLife,
    terminal: bool,
) -> (Bus, ProjectPolicy, Projection) {
    seated_policy_fixture(policy, terminal, team())
}

/// The policy fixture with its one lane seated under `team`.
///
/// Seating the lane under its own identity leaves it without a team, which is
/// how every lane starts before an operator places it.
fn seated_policy_fixture(
    policy: wire::intent::TeamLife,
    terminal: bool,
    team: CanonicalUuid,
) -> (Bus, ProjectPolicy, Projection) {
    let mut journal = Journal::in_memory().expect("policy fixture journal opens");
    journal
        .apply(seed_write_seated(policy, terminal, team))
        .expect("policy fixture seeds");
    let bus = Bus::start(journal).expect("policy fixture bus starts");
    let projects = projects();
    let mut projection = Projection::new();
    projection
        .advance(
            &bus.task_snapshot().expect("policy fixture snapshot reads"),
            &projects,
        )
        .expect("policy fixture projection builds");
    (bus, projects, projection)
}

fn seed_write() -> JournalWrite {
    seed_write_with_policy(wire::intent::TeamLife::Drain, false)
}

fn seed_write_with_policy(policy: wire::intent::TeamLife, terminal: bool) -> JournalWrite {
    seed_write_seated(policy, terminal, team())
}

fn seed_write_seated(
    policy: wire::intent::TeamLife,
    terminal: bool,
    team: CanonicalUuid,
) -> JournalWrite {
    let task = task(TASK_MOMENT);
    let phase = phase();
    let origin = format!("task:{}", handle(TASK_MOMENT))
        .parse::<Origin>()
        .expect("fixture task origin");
    let at = instant(1);
    let mut write = JournalWrite::new(
        Actor::Lane(lane()),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                title: "Project incremental lifecycle".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: PROJECT.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: lane_creation(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact builds");
    attach_task_heads(&mut write, added, task, origin, phase, Vec::new());
    let seated = write
        .fact(body::UpSeated { epoch: epoch() }, FactCause::Pending(added))
        .expect("resident fact builds");
    write.residency_head(seated, move |_| ResidencyHead {
        epoch: epoch(),
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![LaneResidency {
            lane: lane(),
            team,
            name: None,
            worktree: "/fixture".to_owned(),
            life: if terminal { "drain" } else { "drive" }.to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            model: None,
            effort: None,
            occupant: None,
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "vacant".to_owned(),
        }],
        subscriptions: vec![wire::TeamSubscription { team, policy }],
        renewals: Vec::new(),
    });
    write
}

fn ready_task_write(moment: &str, at: Instant) -> JournalWrite {
    let task = task(moment);
    let phase = phase();
    let origin = format!("task:{}", handle(moment))
        .parse::<Origin>()
        .expect("ready task origin");
    let mut write = JournalWrite::new(
        Actor::Lane(lane()),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                title: "Newer ready lifecycle candidate".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: PROJECT.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: lane_creation(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("ready task fact builds");
    attach_task_heads(&mut write, added, task, origin, phase, Vec::new());
    write
}

fn deferral_write(deferred: bool, at: Instant) -> JournalWrite {
    let task = task(TASK_MOMENT);
    let until = instant(20);
    let mut write = JournalWrite::new(
        Actor::Lane(lane()),
        format!("task:{}", handle(TASK_MOMENT))
            .parse()
            .expect("deferral task origin"),
        at,
        Producer::CommandDoor,
    );
    let changed = if deferred {
        write
            .fact(
                body::TaskDeferred {
                    task: task.clone(),
                    until,
                },
                FactCause::Root,
            )
            .expect("deferral fact builds")
    } else {
        write
            .fact(body::TaskWoken { task: task.clone() }, FactCause::Root)
            .expect("wake fact builds")
    };
    let phase = phase();
    write.workflow_head(task, changed, move |_| WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: deferred.then_some(until),
        completion: None,
        generation: 0,
    });
    write
}

fn add_vacant_org_members(bus: &Bus, lanes: &[CanonicalUuid]) {
    let observed = bus
        .residency_head()
        .expect("expanded team residency reads")
        .expect("expanded team has residency");
    let mut next = observed.body().clone();
    for member in lanes {
        next.lanes.push(LaneResidency {
            lane: *member,
            team: team(),
            name: None,
            worktree: format!("/fixture/{member}"),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            model: None,
            effort: None,
            occupant: None,
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "vacant".to_owned(),
        });
    }
    next.lanes.sort_by_key(|row| row.lane);
    let mut write = JournalWrite::new(
        Actor::Spice,
        "recovery:expanded-team"
            .parse()
            .expect("expanded team origin"),
        instant(2),
        Producer::ResidentRecognition,
    );
    let mut cause = FactCause::Root;
    let mut expanded = None;
    for member in lanes {
        let seated = write
            .fact(
                body::LaneSeated {
                    lane: *member,
                    team: team(),
                    capability: *member,
                    name: None,
                    worktree: format!("/fixture/{member}"),
                    life: "drive".to_owned(),
                    vendor: "claude".parse().expect("fixture vendor"),
                    model: None,
                    effort: None,
                },
                cause,
            )
            .expect("expanded team seat fact builds");
        cause = FactCause::Pending(seated);
        expanded = Some(seated);
    }
    write.residency_head(
        expanded.expect("expanded team has at least one added member"),
        move |_| next,
    );
    committed(bus.commit(write).expect("expanded team commits"));
}

fn invalid_late_suffix() -> JournalWrite {
    let at = instant(20);
    let origin = format!("resident:{}", epoch())
        .parse::<Origin>()
        .expect("fixture resident origin");
    let mut write = JournalWrite::new(
        Actor::Lane(lane()),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    let steering = body::SteerSent {
        directive: uuid(4),
        fanout: None,
        key: "1kJK6N3R".to_owned(),
        lane: lane(),
        text: "must not survive a later refusal".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: instant(59),
    };
    let sent = write
        .fact(steering.clone(), FactCause::Root)
        .expect("steering fact builds");
    let steering_origin = origin.clone();
    write.steering_head(sent, move |position| {
        Ledger::sent_head(&steering, steering_origin, at, position)
            .expect("fixture steering head builds")
    });
    let invalid_task = task(INVALID_TASK_MOMENT);
    let phase = phase();
    let added = write
        .fact(
            body::TaskAdded {
                task: invalid_task.clone(),
                title: "Invalid late project".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "outside.work".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Pending(sent),
        )
        .expect("late-invalid task fact builds");
    attach_task_heads_with_project(
        &mut write,
        added,
        invalid_task,
        origin,
        phase,
        Vec::new(),
        "outside.work",
    );
    write
}

fn attach_task_heads(
    write: &mut JournalWrite,
    added: PendingFact,
    task: TaskStamp,
    origin: Origin,
    phase: Phase,
    dependencies: Vec<TaskStamp>,
) {
    attach_task_heads_with_project(write, added, task, origin, phase, dependencies, PROJECT);
}

fn attach_task_heads_with_project(
    write: &mut JournalWrite,
    added: PendingFact,
    task: TaskStamp,
    origin: Origin,
    phase: Phase,
    dependencies: Vec<TaskStamp>,
    project: &'static str,
) {
    let statement_task = task.clone();
    write.task_head(added, move |position| TaskHead {
        born: statement_task,
        queue_origin: position,
        title: "Project incremental lifecycle".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: project.to_owned(),
        tags: Vec::new(),
        dependencies,
        provenance: origin,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision: position,
    });
    write.workflow_head(task, added, move |_| WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
}

fn unrelated_write(label: &str, count: usize) -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("recovery:{label}")
            .parse()
            .expect("fixture recovery origin"),
        instant(30),
        Producer::Judge,
    );
    for index in 0..count {
        write
            .fact(
                body::TasteCleared {
                    taste: format!("{label}-{index}"),
                    locus: "lifecycle-materialization-fixture".to_owned(),
                },
                FactCause::Root,
            )
            .expect("unrelated fact builds");
    }
    write
}

fn hold_write() -> JournalWrite {
    let mut write = JournalWrite::new(
        Actor::Spice,
        "recovery:equal-tail"
            .parse()
            .expect("fixture recovery origin"),
        instant(40),
        Producer::BusTimer,
    );
    write
        .fact(
            body::LaneHeld {
                lane: lane(),
                evidence: "equal scale-tail hold".to_owned(),
                until: instant(55),
            },
            FactCause::Root,
        )
        .expect("hold fact builds");
    write
}

fn replay_holds<'a>(facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>) -> Holds {
    let mut holds = Holds::new(3);
    for envelope in facts {
        absorb_hold(&mut holds, envelope);
    }
    holds
}

fn absorb_hold(holds: &mut Holds, envelope: &wire::Envelope<wire::AnyFact>) {
    if let Some(held) = envelope.fact.read::<body::LaneHeld>() {
        holds.absorb_held(&held);
    } else if let Some(resumed) = envelope.fact.read::<body::LaneResumed>() {
        holds.absorb_resumed(&resumed);
    }
}

fn committed(outcome: CommitOutcome) {
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "fixture commit refused: {outcome:?}"
    );
}

fn projects() -> ProjectPolicy {
    ProjectPolicy::new(["lifecycle".to_owned()], Vec::new(), 2, 4).expect("fixture project policy")
}

fn handle(moment: &str) -> TaskHandle {
    TaskHandle::for_project(PROJECT, &moment.parse().expect("fixture task moment"))
}

fn task(moment: &str) -> TaskStamp {
    handle(moment).born().expect("fixture task carries a stamp")
}

fn phase() -> Phase {
    "todo".parse().expect("fixture phase")
}

fn lane() -> CanonicalUuid {
    uuid(1)
}

/// The team the fixture lane is placed on.
///
/// Its identity differs from the lane's: a lane seated alone under its own
/// identity is not on a team and draws no work, so the fixture places its
/// seat the way an operator would.
fn team() -> CanonicalUuid {
    uuid(7)
}

fn lane_creation() -> body::TaskCreationContext {
    body::TaskCreationContext::Transcript {
        session: "fixture-session".to_owned(),
        boundary: wire::TranscriptSourceHead {
            lane: lane(),
            vendor: "claude".parse().expect("fixture vendor"),
            stream: body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
            source: uuid(90),
            next: body::TranscriptPosition {
                member: 1,
                offset: 1,
            },
        },
        creator_team: Some(team()),
    }
}

fn lane_at(last: u8) -> CanonicalUuid {
    uuid(last)
}

fn epoch() -> CanonicalUuid {
    uuid(2)
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131165{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn instant(second: u8) -> Instant {
    format!("2030-04-12T18:44:{second:02}.271Z")
        .parse()
        .expect("fixture instant")
}
