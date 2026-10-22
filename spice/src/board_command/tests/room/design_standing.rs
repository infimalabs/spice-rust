//! Working-group stance as the sole active/passive authority for an open
//! Design: who can claim it, and what becoming passive asks of its holder.

use super::super::preparation::add_controller;
use super::super::reclaim::free_active;
use super::*;
use wire::intent::WorkingGroupChangeStatus;
use wire::rooms::StopControl;

pub(super) fn design(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    add_controller(bus, epoch, name, wire::TaskIntent::Design, None)
}

pub(super) fn born(task: &TaskHandle) -> TaskStamp {
    task.born().expect("fixture task stamp")
}

/// Win `task` for `seat` under the named `rule`, as `spice task capture` and
/// `spice task steal` submit it; a recovery rule carries a well-formed
/// transaction so the claim reaches selection.
pub(in super::super) fn win_named(
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
            recovery: rule.as_str().starts_with("recover").then(|| "0".repeat(40)),
            rule,
        },
    )
}

pub(in super::super) fn refused(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub(super) fn offered(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) -> Option<TaskStamp> {
    match next_board(bus, epoch, Actor::Lane(seat.lane)).next {
        Some(NextWorkState::Candidate { task }) => Some(task.statement.body.born),
        _ => None,
    }
}

pub(in super::super) fn active_holder(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskHandle,
) -> Option<Actor> {
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
            sent.control
                .as_deref()
                .and_then(|control| control.parse::<StopControl>().ok())
                .is_some_and(|stop| stop.room == room)
        })
        .collect()
}

/// The one live stop request standing against `room`'s holder, if any.
fn live_stop(bus: &Bus, room: &TaskHandle) -> Option<wire::SteeringHead> {
    let room = born(room);
    let mut stops = bus
        .read(move |read| read.live_room_stops(&room, 2))
        .expect("steering index reads");
    assert!(stops.len() <= 1, "one live stop at most: {stops:?}");
    stops.pop().map(|head| head.body().clone())
}

pub(super) fn move_out(
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

fn set_stance(
    bus: &Bus,
    epoch: CanonicalUuid,
    stance: wire::rooms::WorkingGroupStance,
) -> (Outcome<CommandAnswer>, Vec<CanonicalUuid>) {
    let partition = bus
        .read(|read| read.group_partition())
        .expect("partition reads")
        .expect("fixture group stands")
        .into_body();
    let group = partition.groups[0].group.clone();
    let request = wire::rooms::WorkingGroupStanceSet { group, stance };
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupStance,
        vec![Witness::Group {
            revision: partition.revision,
        }],
        &request,
    );
    let wire::intent::Intent::WorkingGroupStance(request) = command.body() else {
        panic!("fixture command carries a working-group stance");
    };
    crate::board_command::set_working_group_stance(
        bus,
        &command,
        request,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
    )
}

/// Commit the fixture group's Passive stance alone, as a stance command that
/// ended before shelving its rooms leaves it.
fn stand_passive_unshelved(bus: &Bus) {
    let from = bus
        .read(|read| read.group_partition())
        .expect("partition reads")
        .expect("fixture group stands")
        .into_body();
    let mut partition = from.clone();
    partition.revision += 1;
    assert!(partition.set_stance(
        &from.groups[0].group,
        wire::rooms::WorkingGroupStance::Passive
    ));
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(
        Actor::Operator,
        wire::rooms::working_group_origin(),
        at,
        Producer::CommandDoor,
    );
    write
        .fact(
            wire::rooms::WorkingGroupsArranged { from, partition },
            FactCause::Root,
        )
        .expect("stance fact builds");
    assert!(matches!(
        bus.commit(write).expect("stance write reaches the journal"),
        CommitOutcome::Committed(_)
    ));
}

fn partition_revision(bus: &Bus) -> u64 {
    bus.read(|read| read.group_partition())
        .expect("partition reads")
        .expect("fixture group stands")
        .body()
        .revision
}

fn group_change(outcome: Outcome<CommandAnswer>) -> wire::intent::WorkingGroupChange {
    match assert_committed(outcome) {
        CommandAnswer::WorkingGroupChange(change) => change,
        other => panic!("expected a working-group change, got {other:?}"),
    }
}

#[test]
fn passive_stance_makes_every_held_design_passive_and_shelves_its_room() {
    let (root, bus, epoch, seats) = fixture("design-standing-passive", 3);
    prove_seat_addresses(&bus, epoch, &seats);
    let first = design(&bus, epoch, "First held by stance");
    let second = design(&bus, epoch, "Second held by stance");
    group_design(&bus, epoch, &first);
    group_design(&bus, epoch, &second);
    let first_holder = Actor::Lane(seats[0].lane);
    let second_holder = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[0], &first_holder, &first);
    claim_task(&bus, epoch, &seats[1], &second_holder, &second);

    let (changed, notified) = set_stance(&bus, epoch, wire::rooms::WorkingGroupStance::Passive);
    let change = group_change(changed);
    assert_eq!(change.status, WorkingGroupChangeStatus::Stanced);
    assert!(change.owed.is_empty());
    assert_eq!(notified, [seats[0].lane, seats[1].lane]);
    for (task, holder) in [(&first, first_holder), (&second, second_holder)] {
        assert_eq!(active_holder(&bus, epoch, task), Some(holder));
        assert!(live_stop(&bus, task).is_some());
        assert_eq!(
            refused(&win_named(&bus, epoch, &seats[2], task, "steal")),
            "claim::design-passive"
        );
    }
    assert_eq!(offered(&bus, epoch, &seats[2]), None);
    clean(&root, bus);
}

#[test]
fn a_repeated_passive_stance_shelves_what_its_change_left_owed() {
    let (root, bus, epoch, seats) = fixture("design-standing-repair", 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let first = design(&bus, epoch, "First left unshelved");
    let second = design(&bus, epoch, "Second left unshelved");
    group_design(&bus, epoch, &first);
    group_design(&bus, epoch, &second);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &first);
    claim_task(&bus, epoch, &seats[1], &Actor::Lane(seats[1].lane), &second);
    stand_passive_unshelved(&bus);
    let revision = partition_revision(&bus);
    assert!(live_stop(&bus, &first).is_none());

    let (repaired, notified) = set_stance(&bus, epoch, wire::rooms::WorkingGroupStance::Passive);
    let change = group_change(repaired);
    assert_eq!(change.status, WorkingGroupChangeStatus::Shelved);
    assert_eq!(change.revision, revision);
    assert!(change.owed.is_empty());
    assert_eq!(notified, [seats[0].lane, seats[1].lane]);
    assert_eq!(
        partition_revision(&bus),
        revision,
        "repair restates no partition"
    );
    for task in [&first, &second] {
        assert!(live_stop(&bus, task).is_some());
        assert_eq!(stop_directions(&bus, task).len(), 1);
    }

    let (again, notified) = set_stance(&bus, epoch, wire::rooms::WorkingGroupStance::Passive);
    assert_eq!(refused(&again), "group::stance-unchanged");
    assert!(notified.is_empty());
    for task in [&first, &second] {
        assert_eq!(
            stop_directions(&bus, task).len(),
            1,
            "nothing owed stops twice"
        );
    }
    clean(&root, bus);
}

#[test]
fn a_room_freed_before_its_shelving_settles_quiescent() {
    let (root, bus, epoch, seats) = fixture("design-standing-freed", 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let freed = design(&bus, epoch, "Freed before shelving");
    let held = design(&bus, epoch, "Held through shelving");
    group_design(&bus, epoch, &freed);
    group_design(&bus, epoch, &held);
    let freed_holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &freed_holder, &freed);
    claim_task(&bus, epoch, &seats[1], &Actor::Lane(seats[1].lane), &held);
    stand_passive_unshelved(&bus);
    free_active(&bus, epoch, &freed_holder, &seats[0], &freed);

    let (repaired, notified) = set_stance(&bus, epoch, wire::rooms::WorkingGroupStance::Passive);
    let change = group_change(repaired);
    assert_eq!(change.status, WorkingGroupChangeStatus::Shelved);
    assert!(change.owed.is_empty(), "a freed room owes nothing");
    assert_eq!(notified, [seats[1].lane]);
    assert!(stop_directions(&bus, &freed).is_empty());
    assert_eq!(stop_directions(&bus, &held).len(), 1);
    clean(&root, bus);
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
    assert_committed(win_named(&bus, epoch, &seats[0], &task, "capture"));
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
    let stop = live_stop(&bus, &task).expect("the room records the park request");
    assert_eq!(stop.lane, seats[0].lane);
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
    assert_eq!(active_holder(&bus, epoch, &task), None);
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
fn a_vacant_holder_is_asked_to_park_through_its_lane() {
    let (root, bus, epoch, seats) = fixture("design-standing-vacant", 1);
    prove_seat_addresses(&bus, epoch, &seats);
    let task = design(&bus, epoch, "Vacant holder design");
    group_design(&bus, epoch, &task);
    let holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &holder, &task);
    vacate(&bus, epoch, &seats[0]);

    assert_committed(move_out(&bus, epoch, &task).0);
    let stop = live_stop(&bus, &task).expect("a vacant holder still receives the park request");
    assert_eq!(stop.lane, seats[0].lane);
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
