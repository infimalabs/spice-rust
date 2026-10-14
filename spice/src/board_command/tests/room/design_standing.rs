//! Working-group standing as the sole active/passive authority for an open
//! Design: who can claim it, and what leaving every group asks of its holder.

use super::super::preparation::add_controller;
use super::*;

fn design(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    add_controller(bus, epoch, name, wire::TaskIntent::Design, None)
}

fn born(task: &TaskHandle) -> TaskStamp {
    task.born().expect("fixture task stamp")
}

/// Win `task` for `seat` under the named `rule`, as `spice task capture` and
/// `spice task steal` submit it.
fn win_named(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
    rule: &str,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let rule: Name = rule.parse().expect("named claim rule");
    issue(
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
    )
}

fn refused(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn offered(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) -> Option<TaskStamp> {
    match next_board(bus, epoch, Actor::Lane(seat.lane)).next {
        Some(NextWorkState::Candidate { task }) => Some(task.statement.body.born),
        _ => None,
    }
}

fn active_holder(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Option<Actor> {
    at(&board(bus, epoch, Actor::Operator), task)
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
        .map(|claim| claim.body.actor.clone())
}

/// Every direction ever sent asking a holder of `room` to stop.
fn stop_directions(bus: &Bus, room: &TaskHandle) -> Vec<wire::body::SteerSent> {
    let room = born(room);
    bus.fact_snapshot()
        .expect("steering facts read")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::body::SteerSent>())
        .filter(|sent| {
            matches!(
                sent.control.as_deref().map(str::parse),
                Some(Ok(CoordinationControl::Stop { room: ref stopped, .. })) if *stopped == room
            )
        })
        .collect()
}

fn move_out(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskHandle,
) -> (Outcome<CommandAnswer>, Option<CanonicalUuid>) {
    let revision = bus
        .read(|read| read.group_partition())
        .expect("partition reads")
        .map_or(0, |head| head.body().revision);
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupMove,
        vec![
            Witness::Group { revision },
            Witness::Existence { task: born(task) },
        ],
        &wire::rooms::WorkingGroupMove {
            design: born(task),
            to: None,
        },
    );
    let wire::intent::Intent::WorkingGroupMove(request) = command.body() else {
        panic!("fixture command carries a working-group move");
    };
    crate::board_command::move_working_group(
        bus,
        &command,
        request,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
    )
}

#[test]
fn only_a_grouped_design_is_offered_or_claimed() {
    let (root, bus, epoch, seats) = fixture("design-standing-claims", 1);
    let task = design(&bus, epoch, "Passive design");

    assert_eq!(
        offered(&bus, epoch, &seats[0]),
        None,
        "a passive Design is never offered"
    );
    assert_eq!(
        refused(&win_named(&bus, epoch, &seats[0], &task, "capture")),
        "claim::design-passive"
    );
    assert_eq!(active_holder(&bus, epoch, &task), None);

    group_design(&bus, epoch, &task);
    assert_eq!(offered(&bus, epoch, &seats[0]), Some(born(&task)));
    let lane = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &lane, &task);
    assert_eq!(active_holder(&bus, epoch, &task), Some(lane));
    clean(&root, bus);
}

#[test]
fn a_held_design_leaving_its_group_asks_its_holder_to_park_and_returns_unchanged() {
    let (root, bus, epoch, seats) = fixture("design-standing-park", 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Held design");
    group_design(&bus, epoch, &task);
    let holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &holder, &task);
    let before = at(&board(&bus, epoch, Actor::Operator), &task)
        .statement
        .clone();

    let (moved, notified) = move_out(&bus, epoch, &task);
    assert_committed(moved);
    assert_eq!(
        notified,
        Some(seats[0].lane),
        "the seated holder is woken at once"
    );
    // The live claim stays authoritative until its holder parks it.
    assert_eq!(active_holder(&bus, epoch, &task), Some(holder.clone()));
    let stop = coordination_status(&bus, &task)
        .stop
        .expect("the room records the park request");
    assert_eq!(coordination_lane(&bus, stop.target), Some(seats[0].lane));
    let [sent] = stop_directions(&bus, &task)
        .try_into()
        .expect("one park request");
    assert_eq!(sent.key, stop.key);
    assert_eq!(sent.lane, seats[0].lane);
    assert!(
        sent.text.contains(&format!("spice task free {task}")),
        "the request names ordinary parking: {}",
        sent.text
    );
    assert_eq!(
        refused(&win_named(&bus, epoch, &seats[1], &task, "steal")),
        "claim::design-passive",
        "no lane steals a passive Design from its holder"
    );

    // Re-entry and a second exit while the request stands send nothing new.
    group_design(&bus, epoch, &task);
    assert_committed(move_out(&bus, epoch, &task).0);
    assert_eq!(stop_directions(&bus, &task).len(), 1);

    free_active(&bus, epoch, &holder, &seats[0], &task);
    assert_eq!(coordination_status(&bus, &task).stop, None);
    assert_eq!(
        offered(&bus, epoch, &seats[0]),
        None,
        "a parked passive Design is not reoffered"
    );
    assert_eq!(offered(&bus, epoch, &seats[1]), None);

    group_design(&bus, epoch, &task);
    assert_eq!(offered(&bus, epoch, &seats[1]), Some(born(&task)));
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &task).statement,
        before,
        "standing changes no statement, preparation, or authorization"
    );
    clean(&root, bus);
}

#[test]
fn a_vacant_holder_is_asked_to_park_under_its_claimed_occupant_address() {
    let (root, bus, epoch, seats) = fixture("design-standing-vacant", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Vacant holder design");
    group_design(&bus, epoch, &task);
    let holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &holder, &task);
    vacate(&bus, epoch, &seats[0]);

    assert_committed(move_out(&bus, epoch, &task).0);
    let stop = coordination_status(&bus, &task)
        .stop
        .expect("a vacant holder still receives the park request");
    let born_address = bus
        .task_snapshot()
        .expect("snapshot reads")
        .facts()
        .occupant_address(seats[0].lane, seats[0].occupant)
        .expect("the claimed occupant's birth is proved");
    assert_eq!(stop.target, born_address);
    let [sent] = stop_directions(&bus, &task)
        .try_into()
        .expect("one park request");
    assert_eq!(
        sent.lane, seats[0].lane,
        "the request queues for the lane's next seat"
    );
    assert_eq!(active_holder(&bus, epoch, &task), Some(holder));
    clean(&root, bus);
}

#[test]
fn leaving_every_group_withdraws_coordination_and_refuses_new_coordination() {
    let (root, bus, epoch, seats) = fixture("design-standing-coordination", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Coordinated design");
    assert_eq!(
        refused(&coordinate(
            &bus,
            &coordinate_command(&bus, epoch, &task, &seats[0], RoomCoordinateAction::Start),
        )),
        "room::design-passive"
    );

    group_design(&bus, epoch, &task);
    let started = assert_committed(coordinate(
        &bus,
        &coordinate_command(&bus, epoch, &task, &seats[0], RoomCoordinateAction::Start),
    ));
    let CommandAnswer::RoomCoordination(started) = started else {
        panic!("start returned another answer shape: {started:?}");
    };
    let key = started.requests[0].key.clone();

    let (moved, notified) = move_out(&bus, epoch, &task);
    assert_committed(moved);
    assert_eq!(notified, None, "an unheld Design has no holder to park");
    assert!(coordination_status(&bus, &task).requests.is_empty());
    let lookup = key.clone();
    let head = bus
        .read(move |read| read.steering_key(&lookup))
        .expect("steering index reads")
        .expect("the request exists");
    assert_ne!(head.body().disposition, wire::Disposition::Live);
    assert_eq!(
        refused(&coordinate(
            &bus,
            &coordinate_command(&bus, epoch, &task, &seats[0], RoomCoordinateAction::Resume),
        )),
        "room::design-passive"
    );
    assert_eq!(
        refused(&win_named(&bus, epoch, &seats[0], &task, "capture")),
        "claim::design-passive"
    );
    clean(&root, bus);
}

#[test]
fn an_offer_read_before_the_design_left_its_group_cannot_claim_it() {
    let (root, bus, epoch, seats) = fixture("design-standing-stale", 1);
    let task = design(&bus, epoch, "Stale offer design");
    group_design(&bus, epoch, &task);
    let lane = Actor::Lane(seats[0].lane);
    let available = board(&bus, epoch, lane.clone());
    let stale = claim_expected(&available, &task, &seats[0]);

    assert_committed(move_out(&bus, epoch, &task).0);
    let outcome = issue(
        &bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        stale,
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    );
    assert!(matches!(outcome, Outcome::Refusal(_)), "{outcome:?}");
    assert_eq!(active_holder(&bus, epoch, &task), None);
    clean(&root, bus);
}
