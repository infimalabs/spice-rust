//! Real journal authority for integration tests that intentionally enter the full gate.

use std::fs;
use std::path::Path;

use journal::{FactCause, Journal, JournalWrite};
use spice::repository::Repository;
use wire::body::{ClaimWon, SourceBoundary, TaskAdded, UpSeated};
use wire::{
    Actor, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Instant, LaneResidency,
    LeaseHead, Producer, ResidencyHead, ResidentState, TaskHead, TaskStamp, WorkflowHead,
};

pub const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163d1";
const OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce9131163d2";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163d3";

#[derive(Clone, Copy)]
struct VerifyAuthority {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    now: Instant,
    expiry: Instant,
}

pub fn install(root: &Path) {
    let repository = Repository::discover(root).expect("verify fixture repository resolves");
    let state = repository
        .admit_state()
        .expect("verify fixture state paths admit");
    if state.journal().is_file() {
        return;
    }
    fs::create_dir_all(
        state
            .journal()
            .parent()
            .expect("verify fixture journal has a parent"),
    )
    .expect("create verify fixture state directory");
    let mut journal = Journal::open(state.journal()).expect("create verify fixture journal");
    let now = now();
    let authority = VerifyAuthority {
        lane: uuid(LANE),
        occupant: uuid(OCCUPANT),
        epoch: uuid(EPOCH),
        now,
        expiry: after(now, 600),
    };

    install_residency(&mut journal, &repository, authority);
    let task = install_task(&mut journal, authority.now);
    install_claim(&mut journal, task, authority);
}

fn install_residency(journal: &mut Journal, repository: &Repository, authority: VerifyAuthority) {
    let VerifyAuthority {
        lane,
        occupant,
        epoch,
        now,
        ..
    } = authority;
    let mut residency = JournalWrite::new(
        Actor::Spice,
        "resident:verify-fixture"
            .parse()
            .expect("verify fixture resident origin"),
        now,
        Producer::CommandDoor,
    );
    let seated = residency
        .fact(UpSeated { epoch }, FactCause::Root)
        .expect("verify fixture seating fact builds");
    let worktree = repository.worktree_root().display().to_string();
    residency.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![LaneResidency {
            lane,
            team: lane,
            name: None,
            worktree,
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("verify fixture vendor"),
            model: None,
            effort: None,
            occupant: Some(occupant),
            lineage: vec![occupant],
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    journal
        .apply(residency)
        .expect("verify fixture residency commits");
}

fn install_task(journal: &mut Journal, now: Instant) -> TaskStamp {
    let task = TaskStamp::for_micros(1).expect("verify fixture task stamp");
    let flow = ["todo", "verify", "review"]
        .map(|phase| phase.parse().expect("verify fixture phase"))
        .to_vec();
    let verify: wire::Phase = "verify".parse().expect("verify fixture current phase");
    let mut addition = JournalWrite::new(
        Actor::Operator,
        "task:verify-fixture"
            .parse()
            .expect("verify fixture task origin"),
        now,
        Producer::CommandDoor,
    );
    let added = addition
        .fact(
            TaskAdded {
                task: task.clone(),
                title: "Exercise full gate".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "task.verify".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                flow: flow.clone(),
                phase: verify.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("verify fixture task fact builds");
    let statement_task = task.clone();
    addition.task_head(added, move |revision| TaskHead {
        born: statement_task,
        queue_origin: revision,
        title: "Exercise full gate".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "task.verify".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:verify-fixture"
            .parse()
            .expect("verify fixture provenance"),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision,
    });
    let workflow_task = task.clone();
    addition.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow,
        phase: verify,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    journal
        .apply(addition)
        .expect("verify fixture task commits");
    task
}

fn install_claim(journal: &mut Journal, task: TaskStamp, authority: VerifyAuthority) {
    let VerifyAuthority {
        lane,
        occupant,
        epoch,
        now,
        expiry,
    } = authority;
    let actor = Actor::Lane(lane);
    let cell = ClaimCell {
        project: "task.verify".to_owned(),
        phase: "verify".parse().expect("verify fixture claim phase"),
    };
    let mut claim = JournalWrite::new(
        actor.clone(),
        "task:verify-fixture"
            .parse()
            .expect("verify fixture claim origin"),
        now,
        Producer::CommandDoor,
    );
    let won = claim
        .fact(
            ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(cell.clone()),
                until: expiry,
                git: None,
                recovery: None,
                source: SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("verify fixture claim fact builds");
    let claim_task = task.clone();
    claim.claim_head(claim_task, won, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    claim.lease_head(task, won, move |_| LeaseHead {
        start: now,
        renewal: now,
        maximum: expiry,
        expiry,
        epoch,
    });
    journal.apply(claim).expect("verify fixture claim commits");
}

pub fn selects_full_gate(arguments: &[&str]) -> bool {
    let mut words = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match *argument {
            "--json" => {}
            "--at" => {
                let _ = arguments.next();
            }
            word => words.push(word),
        }
    }
    words == ["gate"]
}

fn now() -> Instant {
    Instant::new(jiff::Timestamp::now()).expect("verify fixture clock is canonical")
}

fn after(at: Instant, seconds: i64) -> Instant {
    let timestamp = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .expect("verify fixture lease fits the clock");
    Instant::new(timestamp).expect("verify fixture expiry is canonical")
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("verify fixture UUID")
}
