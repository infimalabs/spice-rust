//! Automated resumes under an operator's bounded rule.

use super::*;

/// Start a room coordination with `limit` automated resumes; answers its key.
fn start_ruled(
    bus: &Bus,
    epoch: CanonicalUuid,
    room: &TaskHandle,
    seat: &Seat,
    action: RoomCoordinateAction,
    limit: Option<u8>,
) -> String {
    let started = assert_committed(coordinate(
        bus,
        &ruled_coordinate_command(bus, epoch, room, seat, action, limit),
    ));
    let CommandAnswer::RoomCoordination(started) = started else {
        panic!("start returned another answer shape: {started:?}");
    };
    started.requests[0].key.clone()
}

/// Retire live request `key` as its lane would, acknowledging it or, with a
/// reason, declining it.
fn dispose(bus: &Bus, key: &str, offset: u64, decline: Option<&str>) {
    let lookup = key.to_owned();
    let head = bus
        .read(move |read| read.steering_key(&lookup))
        .expect("steering index reads")
        .expect("request exists");
    let lane = head.body().lane;
    let mut write = JournalWrite::new(
        Actor::Lane(lane),
        head.body().origin.clone(),
        Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
        Producer::AckScanner,
    );
    let locus = wire::body::TranscriptLocus {
        source: Some(uuid(16_300)),
        member: 1,
        offset,
        length: 7,
        event: 0,
        line: 1,
        column: 1,
    };
    let cause = FactCause::Existing(head.position());
    let directive = head.body().directive;
    let epoch = uuid(16_204);
    let (pending, disposition) = match decline {
        None => (
            write.fact(
                wire::body::SteerAcked {
                    directive,
                    key: key.to_owned(),
                    lane,
                    epoch,
                    locus,
                },
                cause,
            ),
            wire::Disposition::Acked,
        ),
        Some(reason) => (
            write.fact(
                wire::body::SteerDeclined {
                    directive,
                    key: key.to_owned(),
                    lane,
                    epoch,
                    locus,
                    reason: reason.to_owned(),
                },
                cause,
            ),
            wire::Disposition::Declined,
        ),
    };
    let pending = pending.expect("disposition builds");
    let mut disposed = head.body().clone();
    disposed.disposition = disposition;
    write.steering_head(pending, move |_| disposed);
    let outcome = bus.commit(write).expect("disposition commits");
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "{outcome:?}"
    );
}

/// Run one reconciliation pass and answer the lanes it resumed.
fn resume(bus: &Bus) -> Vec<CanonicalUuid> {
    room_resumes_by_rule(
        bus,
        &steer::Config::default(),
        &room_projects(),
        &board::NextConfig::default(),
    )
    .expect("ruled rooms read")
    .into_iter()
    .filter_map(|(_, resumed)| resumed.expect("automated resume does not fault"))
    .collect()
}

fn latest(bus: &Bus, room: &TaskHandle) -> RoomCoordinationStarted {
    let room = room.born().expect("fixture room has a birth stamp");
    bus.read(move |read| read.room_direction(&room))
        .expect("room direction reads")
        .expect("room has a coordination record")
        .body()
        .clone()
}

fn stamp(micros: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(micros).expect("fixture publication identity")
}

/// A two-seat room whose first seat is the target and second the other party.
fn ruled_room(name: &str) -> (FixtureRoot, Bus, CanonicalUuid, Vec<Seat>, TaskHandle) {
    let (root, bus, epoch, seats) = fixture(name, 2);
    prove_seat_addresses(&bus, epoch, &seats);
    let room = add(&bus, epoch, Actor::Operator, name, "Room controller", None);
    (root, bus, epoch, seats, room)
}

#[test]
fn a_rule_resumes_once_per_unanswered_input_until_its_budget_is_spent() {
    let (root, bus, epoch, seats, room) = ruled_room("room-rule-budget");
    let target = seats[0].lane;
    let other = seats[1].lane;
    let key = start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Start,
        Some(2),
    );
    let authorizing = latest(&bus, &room);
    publish_listener(&bus, &room, other, 1_760_000_000_000_001);
    assert!(resume(&bus).is_empty(), "a pending request is not resumed");
    dispose(&bus, &key, 40, None);
    assert!(
        resume(&bus).is_empty(),
        "input before the acknowledgment is not new input"
    );
    publish_listener(&bus, &room, target, 1_760_000_000_000_002);
    assert!(
        resume(&bus).is_empty(),
        "the target's own publication is not input"
    );

    publish_listener(&bus, &room, other, 1_760_000_000_000_003);
    publish_listener(&bus, &room, other, 1_760_000_000_000_004);
    assert_eq!(resume(&bus), vec![target]);
    let first = latest(&bus, &room);
    assert_eq!(first.rule, authorizing.rule);
    assert_eq!(first.target, authorizing.target);
    let automated = first
        .automated
        .clone()
        .expect("the resume records its rule");
    assert_eq!(automated.rule, authorizing.publication);
    assert_eq!(automated.step, 1);
    assert_eq!(automated.evidence, stamp(1_760_000_000_000_003));
    let status = coordination_status(&bus, &room);
    assert_eq!(status.requests.len(), 1, "the resume is a live request");
    assert_eq!(
        status.rule,
        Some(wire::intent::RoomResumeBudget { limit: 2, spent: 1 })
    );
    assert!(resume(&bus).is_empty(), "a live resume is not repeated");

    dispose(&bus, &first.key, 41, None);
    assert!(
        resume(&bus).is_empty(),
        "repeated agreement without input resumes nothing"
    );
    publish_listener(&bus, &room, other, 1_760_000_000_000_005);
    assert_eq!(resume(&bus), vec![target]);
    let second = latest(&bus, &room);
    let automated = second
        .automated
        .clone()
        .expect("the resume records its rule");
    assert_eq!(
        (automated.rule, automated.step),
        (authorizing.publication, 2)
    );

    dispose(&bus, &second.key, 42, None);
    publish_listener(&bus, &room, other, 1_760_000_000_000_006);
    assert!(resume(&bus).is_empty(), "a spent budget resumes nothing");
    assert_eq!(latest(&bus, &room), second);
    clean(&root, bus);
}

#[test]
fn a_decline_put_aside_claim_vacancy_or_unruled_resume_stops_the_rule() {
    let (root, bus, epoch, seats, room) = ruled_room("room-rule-stops");
    let other = seats[1].lane;
    let mut micros = 1_760_000_000_000_100;
    let mut input = |bus: &Bus| {
        micros += 1;
        publish_listener(bus, &room, other, micros);
    };

    let key = start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Start,
        Some(8),
    );
    dispose(&bus, &key, 50, Some("not my area"));
    input(&bus);
    assert!(resume(&bus).is_empty(), "a decline ends the rule");

    start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Resume,
        Some(8),
    );
    assert_committed(put_aside(&bus, &put_aside_command(&bus, epoch, &room)));
    input(&bus);
    assert!(
        resume(&bus).is_empty(),
        "putting the room aside ends the rule"
    );
    assert_eq!(coordination_status(&bus, &room).rule, None);

    let key = start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Resume,
        Some(8),
    );
    dispose(&bus, &key, 51, None);
    let manual = start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Resume,
        None,
    );
    dispose(&bus, &manual, 52, None);
    input(&bus);
    assert!(
        resume(&bus).is_empty(),
        "a resume without a rule ends the rule"
    );

    let key = start_ruled(
        &bus,
        epoch,
        &room,
        &seats[0],
        RoomCoordinateAction::Resume,
        Some(8),
    );
    dispose(&bus, &key, 53, None);
    let lane = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &lane, &room);
    input(&bus);
    assert!(resume(&bus).is_empty(), "a claimed anchor is not resumed");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        claim_free_witnesses(&bus, epoch, &lane, &seats[0], &room),
        &TaskRef { task: room.clone() },
    ));

    let before = latest(&bus, &room);
    vacate(&bus, epoch, &seats[0]);
    assert!(
        resume(&bus).is_empty(),
        "a target with no seated occupant is not resumed"
    );
    assert_eq!(latest(&bus, &room), before);
    clean(&root, bus);
}
