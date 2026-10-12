use super::*;

/// Win `task` for `seat` through the allocator, exactly as `task next` does.
fn take_next(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    current: &BoardAnswer,
    task: &TaskHandle,
) -> CommandAnswer {
    assert_committed(issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::ClaimWin,
        claim_expected(current, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ))
}

/// The clean verdict both reviewers try to record on `task`.
fn clean_verdict(task: &TaskHandle) -> TaskReview {
    TaskReview {
        task: task.clone(),
        finding: "CLEAN".to_owned(),
        note: None,
        then: Vec::new(),
        followup: Vec::new(),
    }
}

/// The lapse released the review inside the same fact that ended the claim.
fn assert_released_by_lapse(released: &BoardAnswer, task: &TaskHandle) {
    let heads = at(released, task);
    let claim = heads.claim.as_ref().expect("claim head");
    assert_eq!(claim.body.disposition, ClaimDisposition::Lapsed);
    assert_eq!(heads.workflow.body.assignment, None);
    assert_eq!(heads.workflow.body.review, "unassigned");
    assert_eq!(heads.workflow.body.phase.as_str(), "review");
    assert_eq!(
        heads.workflow.position, claim.position,
        "the release and the lapse are one fact",
    );
}

/// The review is pinned to `successor` by a fresh assignment fact of its own.
fn assert_retaken_by(bus: &Bus, retaken: &BoardAnswer, task: &TaskHandle, successor: &Actor) {
    let heads = at(retaken, task);
    assert_eq!(heads.workflow.body.assignment, Some(successor.clone()));
    assert_eq!(heads.workflow.body.review, "assigned");
    assert_eq!(
        heads.claim.as_ref().expect("claim head").body.actor,
        *successor
    );
    assert_review_assignment_fact(bus, retaken, task, successor);
}

/// A review whose reviewer lapsed is handed to the next lane, verdict and all.
///
/// The live reproduction (ALLOCAT-dSTl1nT36) was a review row whose reviewer
/// left with the claim unfreed: the lapse ended the possession but the
/// assignment still routed the row to that one lane, so `task next` on every
/// other lane reported the board empty. Both verdict doors are checked after the
/// takeover, because the assignment is what the review command admits on.
#[test]
fn a_lapsed_review_is_reallocated_and_its_verdict_moves_with_the_assignment() {
    let (root, bus, epoch, seats) = fixture("lapsed-review", 3);
    let author = Actor::Lane(seats[0].lane);
    let departed = Actor::Lane(seats[1].lane);
    let successor = Actor::Lane(seats[2].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "LAPSE-review001",
        "Review the work whose reviewer left",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &author, &task);
    let reviewable = board(&bus, epoch, departed.clone());
    take_next(&bus, epoch, &seats[1], &reviewable, &task);
    let assigned = board(&bus, epoch, departed.clone());
    assert_eq!(
        at(&assigned, &task).workflow.body.assignment,
        Some(departed.clone())
    );
    let stale_verdict = progress_witnesses(
        &assigned,
        at(&assigned, &task),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("the assigned review has exact witnesses");

    lapse_claim(&bus, epoch, &seats[1], &task);
    let released = board(&bus, epoch, successor.clone());
    assert_released_by_lapse(&released, &task);

    let CommandAnswer::Claim(takeover) = take_next(&bus, epoch, &seats[2], &released, &task) else {
        panic!("task next did not return a claim answer");
    };
    assert_eq!(takeover.task, task);
    let retaken = board(&bus, epoch, successor.clone());
    assert_retaken_by(&bus, &retaken, &task, &successor);

    let refused = issue(
        &bus,
        epoch,
        departed,
        CommandKind::TaskReview,
        stale_verdict,
        &clean_verdict(&task),
    );
    assert!(
        matches!(
            refused,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "review::assignment-refused"
        ),
        "the departed reviewer recorded a verdict on a review it no longer holds: {refused:?}",
    );
    let expected = progress_witnesses(
        &retaken,
        at(&retaken, &task),
        seats[2].lane,
        seats[2].occupant,
        true,
    )
    .expect("the retaken review has exact witnesses");
    assert_committed(issue(
        &bus,
        epoch,
        successor,
        CommandKind::TaskReview,
        expected,
        &clean_verdict(&task),
    ));
    let settled = board(&bus, epoch, Actor::Operator);
    let heads = at(&settled, &task);
    assert_eq!(heads.workflow.body.status, "completed");
    assert_eq!(heads.workflow.body.review, "clean");
    clean(&root, bus);
}

/// The allocator reaches a review pinned by a lapsed reviewer without repair.
///
/// This is the live journal shape before any lapse under the new law ran:
/// the claim head lapsed while the workflow head still names the reviewer.
/// The takeover mints the successor assignment over that stale pin.
#[test]
fn a_review_still_pinned_by_a_lapsed_reviewer_is_taken_over_without_repair() {
    let (root, bus, epoch, seats) = fixture("lapsed-review-pinned", 3);
    let author = Actor::Lane(seats[0].lane);
    let departed = Actor::Lane(seats[1].lane);
    let successor = Actor::Lane(seats[2].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "LAPSE-review002",
        "Review left pinned by an earlier lapse",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &author, &task);
    let reviewable = board(&bus, epoch, departed.clone());
    take_next(&bus, epoch, &seats[1], &reviewable, &task);
    lapse_claim(&bus, epoch, &seats[1], &task);
    pin_review_to(&bus, epoch, &task, &departed);

    let pinned = board(&bus, epoch, successor.clone());
    let heads = at(&pinned, &task);
    assert_eq!(heads.workflow.body.assignment, Some(departed.clone()));
    assert_eq!(
        heads.claim.as_ref().expect("claim head").body.disposition,
        ClaimDisposition::Lapsed
    );
    let entries = actor_entries(&pinned).expect("pinned board projects to entries");
    let entry = entries
        .iter()
        .find(|entry| entry.handle() == &task)
        .expect("the pinned review is on the board");
    assert!(
        board::eligible_entry(
            &board::resolved_tasks(&entries),
            entry,
            &successor,
            &pinned.environment()
        ),
        "a stale pin from a lapsed reviewer no longer hides the review",
    );

    take_next(&bus, epoch, &seats[2], &pinned, &task);
    let retaken = board(&bus, epoch, successor.clone());
    assert_retaken_by(&bus, &retaken, &task, &successor);
    clean(&root, bus);
}

/// Re-pin the review the way the live journal has it: a workflow head naming
/// a reviewer whose claim has already lapsed. No current writer leaves this
/// shape behind any more, so the fixture writes it directly.
fn pin_review_to(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle, reviewer: &Actor) {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, task);
    let mut workflow = heads.workflow.body.clone();
    workflow.assignment = Some(reviewer.clone());
    workflow.review = "assigned".to_owned();
    let born = heads.statement.body.born.clone();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let now = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, now, Producer::CommandDoor);
    let assigned = write
        .fact(
            body::TaskAssigned {
                task: born.clone(),
                reviewer: reviewer.clone(),
                phase: workflow.phase.clone(),
            },
            FactCause::Root,
        )
        .expect("stale pin fact builds");
    write.workflow_head(born, assigned, move |_| workflow);
    match bus.commit(write).expect("stale pin reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("stale pin did not commit: {outcome:?}"),
    }
}
