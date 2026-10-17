//! Required question evidence gates positive completion at the board door.

use super::*;
use wire::rooms::{
    QuestionResultRef, RoomAuthorityStamp, RoomDestination, RoomPosted, RoomPublicationKind,
    RoomRoute,
};

/// The operator's ASK of `lane` in `room`, delivered as one live directive
/// keyed by its result.
fn ask(bus: &Bus, room: &TaskHandle, lane: CanonicalUuid) -> QuestionResultRef {
    let room = room.born().expect("fixture handle carries its birth");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let stamp = |offset: i64| {
        RoomAuthorityStamp::for_micros(1_790_000_000_000_000 + offset).expect("fixture stamp")
    };
    let result = QuestionResultRef::new(stamp(1));
    let directive = uuid(90_001);
    let sent = body::SteerSent {
        from: None,
        room: Some(room.clone()),
        directive,
        fanout: None,
        key: result.to_string(),
        lane,
        text: "which one?".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: fixture_instant_after(at.clone(), 1_500),
    };
    let origin: wire::Origin = format!("task:{room}").parse().expect("fixture origin");
    let mut write = JournalWrite::new(
        Actor::Operator,
        origin.clone(),
        at.clone(),
        Producer::CommandDoor,
    );
    let created = write
        .fact(sent.clone(), FactCause::Root)
        .expect("direction builds");
    let published = at.clone();
    write.steering_head(created, move |created| wire::SteeringHead {
        from: None,
        room: sent.room,
        directive: sent.directive,
        fanout: None,
        key: sent.key,
        lane: sent.lane,
        text: sent.text,
        control: None,
        attachments: Vec::new(),
        published,
        expires: sent.expires,
        stage: body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: wire::Disposition::Live,
        origin,
        created,
        minted_micros: None,
    });
    write
        .fact(
            RoomPosted {
                publication: stamp(0),
                text: "which one?".to_owned(),
                operation: RoomPublicationKind::Ask {
                    result: result.clone(),
                    deadline: fixture_instant_after(at.clone(), 1_800),
                },
                routes: vec![RoomRoute {
                    destination: RoomDestination::Lane { lane },
                    context: Some(room),
                    direction: None,
                }],
                directed: vec![directive],
                expires: fixture_instant_after(at, 1_200),
            },
            FactCause::Root,
        )
        .expect("ASK builds");
    assert!(matches!(
        bus.commit(write).expect("ASK reaches writer"),
        CommitOutcome::Committed(_)
    ));
    result
}

fn change(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &TaskHandle,
    evidence: wire::TaskEvidenceEdit,
) -> Outcome<CommandAnswer> {
    let view = board(bus, epoch, actor.clone());
    issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&view, task))],
        &TaskEdit {
            intent: None,
            evidence: Some(evidence),
            task: task.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
            flow: None,
        },
    )
}

fn done(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
        .expect("completion witnesses");
    issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: task.clone(),
            validation: "checked".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    )
}

fn refused(outcome: &Outcome<CommandAnswer>, reason: &str) {
    assert!(
        matches!(outcome, Outcome::Refusal(refusal) if refusal.reason() == reason),
        "expected refusal {reason}, got {outcome:?}"
    );
}

#[test]
fn completion_waits_for_required_evidence_until_answered_or_waived_by_the_operator() {
    let (root, bus, epoch, seats) = fixture("evidence-completion", 2);
    let lane = Actor::Lane(seats[0].lane);
    let task = todo_task(&bus, epoch, "EVID-task0001", "Consume one answer");
    let result = ask(&bus, &task, seats[1].lane);
    assert_committed(change(
        &bus,
        epoch,
        lane.clone(),
        &task,
        wire::TaskEvidenceEdit::Require {
            result: result.clone(),
        },
    ));
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &task).statement.body.evidence, [result.clone()]);

    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let pending = done(&bus, epoch, &seats[0], &task);
    refused(&pending, "task::evidence-unsatisfied");
    let lane_waiver = change(
        &bus,
        epoch,
        lane.clone(),
        &task,
        wire::TaskEvidenceEdit::Waive {
            result: result.clone(),
        },
    );
    refused(&lane_waiver, "task::evidence-authority");
    assert_committed(change(
        &bus,
        epoch,
        Actor::Operator,
        &task,
        wire::TaskEvidenceEdit::Waive { result },
    ));
    reclaim_held(&bus, epoch, &lane, &seats[0], &task);
    assert_committed(done(&bus, epoch, &seats[0], &task));
    clean(&root, bus);
}
