use super::*;
use wire::intent::{REPORT_READ_VERSION, ReportRead};

#[test]
fn report_admits_public_facts_and_refuses_a_different_generation_pin() {
    let (_root, bus, epoch, seats) = fixture("report-public-scope", 1);
    let actor = Actor::Lane(seats[0].lane);
    let public = add(
        &bus,
        epoch,
        Actor::Operator,
        "report-public",
        "Visible report work",
        Some("todo"),
    );
    let hidden = hidden::add_oops(
        &bus,
        epoch,
        Actor::Operator,
        "report-hidden",
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
    for task in [&public, &hidden] {
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
            .filter_map(|fact| fact.fact.task_subject())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([&public.born().expect("public identity")]),
    );
    assert_eq!(
        answer
            .rows
            .iter()
            .map(|fact| fact.fact.kind())
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
