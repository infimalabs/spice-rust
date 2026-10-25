use super::*;
use wire::intent::{REPORT_READ_VERSION, ReportRead};

#[test]
fn report_admits_filed_facts_and_refuses_a_different_generation_pin() {
    let (_root, bus, epoch, seats) = fixture("report-filed-scope", 1);
    let actor = Actor::Lane(seats[0].lane);
    let filed = add(
        &bus,
        epoch,
        Actor::Operator,
        "report-filed",
        "Visible report work",
        Some("todo"),
    );
    let draft = add_draft_row(
        &bus,
        epoch,
        Actor::Operator,
        "report-draft",
        "Private friction evidence",
    );
    let at: Instant = "2026-09-30T12:00:00.000Z".parse().expect("fixture instant");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "fixture:report-lease-scope"
            .parse()
            .expect("fixture origin"),
        at,
        Producer::BusTimer,
    );
    for task in [&filed, &draft] {
        write
            .fact(
                body::ClaimLapsed {
                    task: task.born().expect("fixture identity"),
                    occupant: seats[0].occupant,
                    expiry: at,
                },
                FactCause::Root,
            )
            .expect("lease fact builds");
    }
    assert!(matches!(
        bus.commit(write).expect("lease fixture commits"),
        CommitOutcome::Committed(_)
    ));
    let upper = board(&bus, epoch, actor.clone()).cursor;
    let mut request = ReportRead {
        version: REPORT_READ_VERSION,
        after: None,
        upper,
        kinds: vec![wire::FactKind::TaskAdded, wire::FactKind::ClaimLapsed],
        pin: None,
        command_at: None,
    };
    let answer = match issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::ReportRead,
        Vec::new(),
        &request,
    ) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Report(answer),
            ..
        }) => answer,
        outcome => panic!("report read failed: {outcome:?}"),
    };
    assert_eq!(
        answer
            .rows
            .iter()
            .filter_map(wire::StreamFact::task)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([filed.born().expect("filed identity")]),
    );
    assert_eq!(
        answer
            .rows
            .iter()
            .map(wire::StreamFact::kind)
            .collect::<Vec<_>>(),
        [wire::FactKind::TaskAdded, wire::FactKind::ClaimLapsed],
    );
    let mut pin = answer.pin;
    pin.epoch = uuid(999_999);
    request.pin = Some(pin);
    assert!(matches!(
        issue(
            &bus,
            epoch,
            actor,
            CommandKind::ReportRead,
            Vec::new(),
            &request
        ),
        Outcome::Refusal(_)
    ));
}

#[test]
fn report_pages_carry_seat_fields_but_never_a_lane_socket_capability() {
    let (_root, bus, epoch, seats) = fixture("report-seat-credential", 1);
    let seat = &seats[0];
    let capability = uuid(777_777);
    let mut write = JournalWrite::new(
        Actor::Operator,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        current_instant().expect("clock"),
        Producer::ResidentRecognition,
    );
    write
        .fact(
            body::LaneSeated {
                lane: seat.lane,
                team: seat.team,
                capability,
                name: Some("Ava".to_owned()),
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "omp".parse().expect("fixture vendor"),
                model: None,
                effort: None,
            },
            FactCause::Root,
        )
        .expect("seat fact builds");
    assert!(matches!(
        bus.commit(write).expect("seat fixture commits"),
        CommitOutcome::Committed(_)
    ));
    let actor = Actor::Lane(seat.lane);
    let request = ReportRead {
        version: REPORT_READ_VERSION,
        after: None,
        upper: board(&bus, epoch, actor.clone()).cursor,
        kinds: vec![wire::FactKind::LaneSeated],
        pin: None,
        command_at: None,
    };
    let answer = match issue(
        &bus,
        epoch,
        actor,
        CommandKind::ReportRead,
        Vec::new(),
        &request,
    ) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Report(answer),
            ..
        }) => answer,
        outcome => panic!("report read failed: {outcome:?}"),
    };
    let encoded = serde_json::to_string(&answer).expect("report answer encodes");
    assert!(!encoded.contains(&capability.to_string()), "{encoded}");
    assert_eq!(
        answer
            .rows
            .iter()
            .filter_map(wire::StreamFact::lane_seated)
            .collect::<Vec<_>>(),
        [wire::StreamLaneSeated {
            lane: seat.lane,
            team: seat.team,
            name: Some("Ava".to_owned()),
            worktree: "/fixture".to_owned(),
            life: "drive".to_owned(),
            vendor: "omp".parse().expect("fixture vendor"),
            model: None,
            effort: None,
        }],
    );
}
