use std::path::Path;

use journal::{FactCause, Journal, JournalWrite};
use wire::body::{ClaimWon, SourceBoundary, TaskAdded};
use wire::{
    Actor, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Instant, LeaseHead, Producer,
    TaskHandle, TaskHead, TaskStamp, WorkflowHead,
};

const PROJECT: &str = "gate.routing";

pub(super) fn seed_todo_claim(path: &Path, lane: CanonicalUuid) -> TaskHandle {
    let mut journal = Journal::open(path).expect("open routing journal");
    let task = TaskStamp::for_micros(1).expect("routing task stamp");
    let todo: wire::Phase = "todo".parse().expect("routing todo phase");
    let at = instant("2000-01-01T00:00:00.000Z");
    add_task(&mut journal, &task, &todo, at);
    add_claim(&mut journal, &task, todo, lane, at);
    TaskHandle::for_project(PROJECT, &task)
}

fn add_task(journal: &mut Journal, task: &TaskStamp, todo: &wire::Phase, at: Instant) {
    let origin: wire::Origin = "task:routing-fixture".parse().expect("routing origin");
    let flow = ["todo", "review"]
        .map(|phase| phase.parse().expect("routing workflow phase"))
        .to_vec();
    let mut add = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let added = add
        .fact(
            TaskAdded {
                task: task.clone(),
                title: "Exercise routing admission".to_owned(),
                description: None,
                acceptance: vec!["routing is operational".to_owned()],
                requirements: wire::EnvironmentRequirements::default(),
                project: PROJECT.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: todo.clone(),
                revision: 1,
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
            },
            FactCause::Root,
        )
        .expect("build routing task fact");
    let statement_task = task.clone();
    add.task_head(added, move |revision| TaskHead {
        born: statement_task,
        queue_origin: revision,
        title: "Exercise routing admission".to_owned(),
        description: None,
        acceptance: vec!["routing is operational".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        project: PROJECT.to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let workflow_task = task.clone();
    let phase = todo.clone();
    add.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow,
        phase,
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
    journal.apply(add).expect("commit routing task");
}

fn add_claim(
    journal: &mut Journal,
    task: &TaskStamp,
    todo: wire::Phase,
    lane: CanonicalUuid,
    at: Instant,
) {
    let occupant = uuid(2);
    let epoch = uuid(3);
    let actor = Actor::Lane(lane);
    let cell = ClaimCell {
        project: PROJECT.to_owned(),
        phase: todo,
    };
    let until = instant("2099-01-01T01:00:00.000Z");
    let mut claim = JournalWrite::new(
        actor.clone(),
        "task:routing-fixture".parse().expect("routing origin"),
        at,
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
                until,
                git: None,
                recovery: None,
                source: SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("build routing claim fact");
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
        acknowledged: 0,
    });
    let lease_task = task.clone();
    claim.lease_head(lease_task, won, move |_| LeaseHead {
        start: at,
        renewal: at,
        expiry: until,
        maximum: until,
        epoch,
    });
    journal.apply(claim).expect("commit routing claim");
}

pub(super) fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("routing UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("routing instant")
}
