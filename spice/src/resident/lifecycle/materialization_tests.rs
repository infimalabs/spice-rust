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
use super::{Available, Backoff, Resolution, Trigger, project_targets, resolve};

const PROJECT: &str = "lifecycle.work";
const TASK_MOMENT: &str = "1kKRRsDP";
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
fn team_policy_drives_every_member_while_terminal_seat_life_remains_an_override() {
    for (policy, expected) in [
        (wire::intent::LineageLife::Steer, Resolution::Idle),
        (
            wire::intent::LineageLife::Drive,
            Resolution::Wait {
                trigger: Trigger::AvailableWork,
                lane: lane(),
                deadline: "2030-04-12T18:47:01.271Z"
                    .parse()
                    .expect("starvation deadline"),
            },
        ),
        (
            wire::intent::LineageLife::Drain,
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

    let (bus, _, projection) = policy_fixture(wire::intent::LineageLife::Drain, true);
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

#[test]
fn drain_team_startup_projects_every_current_member() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::LineageLife::Drain, false);
    add_vacant_team_members(&bus, &[lane_at(2), lane_at(3)]);
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
            .all(|target| target.drain_queue == Available::Ready)
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
    let residency = incremental
        .project_residency(
            snapshot
                .residency()
                .expect("fixture residency head exists")
                .body(),
        )
        .expect("automatic subscription projects");
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
    policy: wire::intent::LineageLife,
    terminal: bool,
) -> (Bus, ProjectPolicy, Projection) {
    let mut journal = Journal::in_memory().expect("policy fixture journal opens");
    journal
        .apply(seed_write_with_policy(policy, terminal))
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
    seed_write_with_policy(wire::intent::LineageLife::Drain, false)
}

fn seed_write_with_policy(policy: wire::intent::LineageLife, terminal: bool) -> JournalWrite {
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
                project: PROJECT.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
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
            lineage_id: lane(),
            name: None,
            worktree: "/fixture".to_owned(),
            life: if terminal { "drain" } else { "drive" }.to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: None,
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "vacant".to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: lane(),
            policy,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
    });
    write
}

fn add_vacant_team_members(bus: &Bus, lanes: &[CanonicalUuid]) {
    let observed = bus
        .residency_head()
        .expect("expanded team residency reads")
        .expect("expanded team has residency");
    let mut next = observed.body().clone();
    for member in lanes {
        next.lanes.push(LaneResidency {
            lane: *member,
            lineage_id: lane(),
            name: None,
            worktree: format!("/fixture/{member}"),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
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
                    lineage_id: lane(),
                    capability: *member,
                    name: None,
                    worktree: format!("/fixture/{member}"),
                    life: "drive".to_owned(),
                    vendor: "claude".parse().expect("fixture vendor"),
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
