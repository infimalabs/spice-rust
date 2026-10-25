use super::trust_ice::{held_claim_write, target};
use super::*;

#[test]
fn a_held_design_short_of_a_ready_pass_wakes_its_lane_only_for_operator_direction() {
    let (bus, mut projection) = policy_fixture(wire::intent::TeamLife::Drive, false);
    commit_and_advance(
        &bus,
        &mut projection,
        held_claim_write(&bus, instant(2), |statement, workflow| {
            statement.intent = Some(wire::TaskIntent::Design);
            workflow.flow = vec!["design".parse().expect("fixture phase")];
            workflow.phase = "design".parse().expect("fixture phase");
        }),
    );
    // An unsteeped, un-nudged Design is still gathering input for its first
    // pass, so its held claim earns no work-driven start.
    let parked = target(
        &bus.task_snapshot().expect("parked snapshot reads"),
        &projection,
    );
    assert_eq!(
        [parked.held_claim, parked.drain_queue, parked.available],
        [Available::None; 3]
    );
    assert_eq!(resolution(&bus, &projection), Resolution::Idle);

    commit_and_advance(&bus, &mut projection, operator_direction_write());
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

fn commit_and_advance(bus: &Bus, projection: &mut Projection, write: JournalWrite) {
    committed(bus.commit(write).expect("fixture write commits"));
    projection
        .advance(&bus.task_snapshot().expect("snapshot reads"))
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
    )
}
