use super::*;

pub(super) fn deferred_task(project: &str, stamp: &str) -> TaskHandle {
    TaskHandle::for_project(project, &stamp.parse().expect("fixture task stamp"))
}

pub(super) fn seed_timer_task(
    bus: &Bus,
    task: &TaskHandle,
    project: &str,
    filing: wire::TaskFiling,
    deferral: Option<Instant>,
    dependencies: Vec<wire::TaskStamp>,
) {
    let stamp = task.born().expect("fixture task has a stamp");
    let phase = "todo".parse::<Phase>().expect("fixture phase");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let origin = format!("task:{stamp}")
        .parse::<Origin>()
        .expect("fixture task origin");
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let added = write
        .fact(
            body::TaskAdded {
                preparation: None,
                intent: None,
                task: stamp.clone(),
                title: format!("Timer fixture {stamp}"),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: project.to_owned(),
                filing,
                tags: Vec::new(),
                priority: None,
                taste: None,
                depends: dependencies.clone(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("fixture task fact builds");
    let workflow_fact = match deferral {
        Some(until) => write
            .fact(
                body::TaskDeferred {
                    task: stamp.clone(),
                    until,
                },
                FactCause::Pending(added),
            )
            .expect("fixture deferral fact builds"),
        None => added,
    };
    let statement_stamp = stamp.clone();
    let statement_phase = phase.clone();
    let statement_project = project.to_owned();
    let statement_filing = filing;
    write.task_head(added, move |position| TaskHead {
        preparation: None,
        authorization: None,
        evidence: Vec::new(),
        intent: None,
        born: statement_stamp,
        queue_origin: position,
        title: "Timer deferral fixture".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: statement_project,
        filing: statement_filing,
        tags: Vec::new(),
        priority: None,
        taste: None,
        dependencies,
        provenance: origin,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision: position,
    });
    write.workflow_head(stamp, workflow_fact, move |_| WorkflowHead {
        flow: vec![statement_phase.clone()],
        phase: statement_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral,
        completion: None,
        generation: 0,
    });
    assert!(matches!(
        bus.commit(write).expect("fixture task commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

pub(super) fn timer_task_heads(bus: &Bus, task: &TaskHandle) -> TaskHeads {
    let stamp = task.born().expect("fixture task has a stamp");
    bus.task_snapshot()
        .expect("fixture task snapshot reads")
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == stamp)
        .expect("fixture task heads exist")
        .clone()
}

pub(super) fn task_woken_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture task facts read")
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::TaskWoken)
        .count()
}
