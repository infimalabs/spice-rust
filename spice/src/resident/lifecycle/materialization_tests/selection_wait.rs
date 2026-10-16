use super::trust_ice::{held_claim_write, target};
use super::*;

const OUTPUT_MOMENT: &str = "1kKRRsDV";

#[test]
fn a_held_design_awaiting_selection_wakes_its_lane_only_for_operator_direction() {
    let (bus, projects, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    commit_and_advance(
        &bus,
        &projects,
        &mut projection,
        held_claim_write(&bus, instant(2), |statement, workflow| {
            statement.intent = Some(wire::TaskIntent::Design);
            workflow.flow = vec!["design".parse().expect("fixture phase")];
            workflow.phase = "design".parse().expect("fixture phase");
        }),
    );
    assert_eq!(
        resolution(&bus, &projection),
        Resolution::Attempt {
            trigger: Trigger::HeldClaim,
            lane: lane(),
        },
        "a held Design with no unselected output resumes its lane"
    );

    commit_and_advance(&bus, &projects, &mut projection, output_write(instant(3)));
    let parked = target(
        &bus.task_snapshot().expect("parked snapshot reads"),
        &projection,
    );
    assert_eq!(
        [parked.held_claim, parked.drain_queue, parked.available],
        [Available::None; 3]
    );
    assert_eq!(resolution(&bus, &projection), Resolution::Idle);

    commit_and_advance(&bus, &projects, &mut projection, operator_direction_write());
    assert_eq!(
        resolution(&bus, &projection),
        Resolution::Attempt {
            trigger: Trigger::PendingInbox,
            lane: lane(),
        },
        "operator direction still wakes the lane"
    );
    bus.shutdown().expect("selection wait fixture stops");
}

fn commit_and_advance(
    bus: &Bus,
    projects: &ProjectPolicy,
    projection: &mut Projection,
    write: JournalWrite,
) {
    committed(bus.commit(write).expect("fixture write commits"));
    projection
        .advance(&bus.task_snapshot().expect("snapshot reads"), projects)
        .expect("projection advances");
}

fn resolution(bus: &Bus, projection: &Projection) -> Resolution {
    resolve(
        &[target(
            &bus.task_snapshot().expect("snapshot reads"),
            projection,
        )],
        &Backoff::default(),
        instant(50),
        lane::WakeOrder::default(),
    )
}

/// A pending output the seeded Design constructed, with no selection.
fn output_write(at: Instant) -> JournalWrite {
    let output = task(OUTPUT_MOMENT);
    let design = task(TASK_MOMENT);
    let preparation = wire::TaskPreparation {
        parent: design.clone(),
        controller: design,
        construction: output.clone(),
    };
    let origin = format!("task:{}", handle(OUTPUT_MOMENT))
        .parse::<Origin>()
        .expect("output origin");
    let mut write = JournalWrite::new(Actor::Lane(lane()), origin, at, Producer::CommandDoor);
    let added = write
        .fact(
            body::TaskAdded {
                preparation: Some(preparation.clone()),
                intent: None,
                task: output.clone(),
                title: "Unselected output".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: PROJECT.to_owned(),
                filing: wire::TaskFiling::Filed,
                tags: Vec::new(),
                priority: None,
                taste: None,
                depends: Vec::new(),
                flow: vec![phase()],
                phase: phase(),
                creation: lane_creation(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("output fact builds");
    let born = output.clone();
    let provenance = wire::TaskProvenance::Task(output.clone());
    write.task_head(added, move |position| TaskHead {
        preparation: Some(preparation),
        authorization: None,
        evidence: Vec::new(),
        intent: None,
        born,
        queue_origin: position,
        title: "Unselected output".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: PROJECT.to_owned(),
        filing: wire::TaskFiling::Filed,
        tags: Vec::new(),
        priority: None,
        taste: None,
        dependencies: Vec::new(),
        provenance,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision: position,
    });
    write.workflow_head(output, added, move |_| WorkflowHead {
        flow: vec![phase()],
        phase: phase(),
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    write
}
