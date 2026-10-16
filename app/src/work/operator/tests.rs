use super::*;
use crate::work::tests::{board, claim, stamp, task};
use wire::EnvironmentRequirement;
use wire::head::ClaimDisposition;

fn upgrade() -> wire::EnvironmentRequirements {
    wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("requirement"),
    )
}

fn queued(queue: &OperatorQueue<'_>) -> Vec<usize> {
    queue.waits().iter().map(OperatorWait::task).collect()
}

#[test]
fn only_execution_walls_wait_in_the_operator_danger_queue() {
    let mut claimed_design = task(1, "queue.design");
    claimed_design.claim = Some(claim(&claimed_design, ClaimDisposition::Active));
    let mut deferred_design = task(2, "queue.design");
    deferred_design.workflow.body.deferral =
        Some("2026-09-24T06:00:00.000Z".parse().expect("deferral"));
    let mut held = task(3, "queue.held");
    held.statement.body.requirements = upgrade();
    let mut first_dependent = task(4, "queue.held");
    first_dependent.statement.body.dependencies = vec![stamp(3)];
    let mut second_dependent = task(5, "queue.held");
    second_dependent.statement.body.dependencies = vec![stamp(4), stamp(3)];
    let mut deferred_upgrade = task(6, "queue.aside");
    deferred_upgrade.statement.body.requirements = upgrade();
    deferred_upgrade.workflow.body.deferral =
        Some("2026-09-24T06:00:00.000Z".parse().expect("deferral"));
    let mut behind_deferral = task(7, "queue.aside");
    behind_deferral.statement.body.dependencies = vec![stamp(6)];
    let ready = task(8, "queue.ready");
    let blocked_gate = task(9, "queue.blocked");

    let mut board = board(vec![
        claimed_design,
        deferred_design,
        held,
        first_dependent,
        second_dependent,
        deferred_upgrade,
        behind_deferral,
        ready,
        blocked_gate,
    ]);
    board.awaiting_selection.extend([stamp(1), stamp(2)]);
    board
        .blocked
        .extend([stamp(4), stamp(5), stamp(7), stamp(9)]);
    let work = WorkProjection::from_board(&board);
    let queue = OperatorQueue::new(&work);

    assert_eq!(queued(&queue), [2, 5, 8]);
    let waits = queue.waits();
    assert!(waits[0].environment());
    assert_eq!(
        waits[0].holds(),
        [3, 4],
        "a held task reached directly and through another wall is one wait"
    );
    assert!(
        !waits[1].environment(),
        "set-aside work waits on nothing for its own sake"
    );
    assert_eq!(waits[1].wall(), Some(DependencyWall::Deferred(&stamp(6))));
    assert_eq!(waits[1].holds(), [6]);
    assert_eq!(waits[2].wall(), Some(DependencyWall::Gate(&stamp(9))),);
    assert!(
        waits[2].holds().is_empty(),
        "a resident-classified blocked gate appears even without a projected dependency wall"
    );
    assert!(
        !queued(&queue).contains(&0) && !queued(&queue).contains(&1),
        "selection stays visible through Work without entering the red danger queue"
    );
}

#[test]
fn next_steps_through_the_queue_in_order_and_wraps() {
    let mut first = task(1, "queue.next");
    first.statement.body.requirements = upgrade();
    let ready = task(2, "queue.next");
    let mut second = task(3, "queue.next");
    second.statement.body.requirements = upgrade();
    let selection = task(4, "queue.next");
    let mut board = board(vec![first, ready, second, selection]);
    board.awaiting_selection.insert(stamp(4));
    let work = WorkProjection::from_board(&board);
    let queue = OperatorQueue::new(&work);

    assert_eq!(queue.next_after(None), Some(0));
    assert_eq!(queue.next_after(Some(0)), Some(2));
    assert_eq!(
        queue.next_after(Some(2)),
        Some(0),
        "the last wraps to the first"
    );
    assert_eq!(
        queue.next_after(Some(1)),
        Some(0),
        "an open task that waits on nothing starts from the first"
    );

    let empty = crate::work::tests::board(Vec::new());
    let work = WorkProjection::from_board(&empty);
    assert_eq!(OperatorQueue::new(&work).next_after(None), None);
}
