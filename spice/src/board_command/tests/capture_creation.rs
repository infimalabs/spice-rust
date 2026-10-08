use super::*;

use super::parked_worktree::{bind_lane, head_commit, issue_git, track_origin};
use crate::test_repository;

fn captured_add() -> TaskAdd {
    TaskAdd {
        title: "Adopt the committed implementation".to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some("ack:1kLGp2NJ".parse().expect("fixture steering origin")),
        flow: None,
        depends: Vec::new(),
        claim: true,
    }
}

fn capture_expected(answer: &BoardAnswer, seat: &Seat, task: TaskStamp) -> Vec<Witness> {
    vec![
        Witness::Absence { task },
        Witness::Eligibility {
            rule: CAPTURE_WORK_RULE
                .parse()
                .expect("capturework is a wire name"),
        },
        residency(answer, seat),
    ]
}

#[test]
fn committed_work_is_created_and_claimed_in_one_implementation_transaction() {
    let (root, bus, epoch, seats) = fixture("capture-create", 1);
    let seat = &seats[0];
    authorize_lane_creation(&bus, epoch, seat);
    let actor = Actor::Lane(seat.lane);
    let (lane, repository, opening) = bind_lane(&root, &bus, epoch, seat);
    track_origin(&lane, &root.join("origin.git"));
    fs::write(lane.join("captured.txt"), "captured implementation\n").expect("captured work file");
    test_repository::stage_all(&lane);
    let captured = test_repository::commit(&lane, "captured implementation");
    assert_ne!(captured, opening);

    let born = stamp("CAPTURE-create1");
    let handle = TaskHandle::for_project("board.test", &born);
    let before = board(&bus, epoch, actor.clone());
    let outcome = issue_git(
        &bus,
        &repository,
        epoch,
        actor.clone(),
        CommandKind::TaskAdd,
        capture_expected(&before, seat, born),
        &captured_add(),
    );
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(range),
        body: CommandAnswer::Task(answer),
    }) = outcome
    else {
        panic!("compound capture did not commit: {outcome:?}")
    };
    let claim = answer.claim.expect("compound capture returns its claim");
    assert_eq!(answer.task, handle);
    assert_eq!(claim.task, handle);
    assert_eq!(
        claim.phase.as_ref().map(|phase| phase.name.as_str()),
        Some("todo")
    );
    let committed = bus
        .fact_snapshot()
        .expect("capture facts remain readable")
        .facts()
        .iter()
        .filter(|fact| fact.seq >= range.first && fact.seq <= range.last)
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(committed.contains(&wire::FactKind::TaskAdded));
    assert!(committed.contains(&wire::FactKind::ClaimWon));

    let held = board(&bus, epoch, actor.clone());
    let heads = at(&held, &handle);
    assert_eq!(heads.workflow.body.phase.as_str(), "todo");
    assert_eq!(
        heads
            .workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["todo", "review"]
    );
    let boundary = &heads
        .git_context
        .as_ref()
        .expect("captured claim has a Git boundary")
        .boundary;
    assert_eq!(boundary.head, captured);
    assert_eq!(boundary.local_commits, 1);
    assert_eq!(head_commit(&lane), captured);

    let refused_born = stamp("CAPTURE-create2");
    let refused = TaskHandle::for_project("board.test", &refused_born);
    let facts_before = bus
        .fact_snapshot()
        .expect("capture facts remain readable")
        .facts()
        .len();
    let outcome = issue_git(
        &bus,
        &repository,
        epoch,
        actor.clone(),
        CommandKind::TaskAdd,
        capture_expected(&held, seat, refused_born),
        &captured_add(),
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "claim::active-exists"),
        "post-creation claim refusal must remain atomic: {outcome:?}",
    );
    assert_eq!(
        bus.fact_snapshot()
            .expect("capture facts remain readable")
            .facts()
            .len(),
        facts_before,
    );
    assert!(
        board(&bus, epoch, actor)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != refused),
        "a refused compound capture must not create its task",
    );
    clean(&root, bus);
}
