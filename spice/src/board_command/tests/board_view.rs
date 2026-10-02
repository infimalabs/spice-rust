//! The `board` leaf's posture and family pages over one large board.

use super::*;
use wire::output::Request;

const TASKS: usize = 300;

fn read(
    board: &BoardAnswer,
    family: Option<&str>,
    rows: Option<usize>,
    after: Option<&TaskHandle>,
) -> Result<BoardView, Outcome<CommandAnswer>> {
    let request = ViewRequest::new(
        family.map(str::to_owned),
        Request {
            rows,
            ..Request::default()
        },
        after.cloned(),
        None,
    )
    .expect("request admits");
    project_board(board, &request)
}

fn posture(board: &BoardAnswer, rows: Option<usize>) -> Box<Posture> {
    match read(board, None, rows, None) {
        Ok(BoardView::Posture(posture)) => posture,
        other => panic!("bare board is a posture: {other:?}"),
    }
}

/// Every handle a read and its continuations show, in order.
fn pages(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: &Actor,
    family: Option<&str>,
    rows: usize,
) -> Vec<TaskHandle> {
    let mut shown = Vec::new();
    let mut after = None::<TaskHandle>;
    loop {
        let board = board(bus, epoch, actor.clone());
        let bounded = match read(&board, family, Some(rows), after.as_ref()) {
            Ok(BoardView::Posture(posture)) => posture.work,
            Ok(BoardView::Family(page)) => page.tasks,
            other => panic!("a view is a posture or a family page: {other:?}"),
        };
        shown.extend(bounded.rows.iter().map(|row| row.handle.clone()));
        let Some(omission) = bounded.omission else {
            return shown;
        };
        let arguments = omission.continuation.arguments();
        assert_eq!(arguments[0], "board");
        assert_eq!(arguments[arguments.len() - 2], "--after");
        after = Some(
            arguments[arguments.len() - 1]
                .parse()
                .expect("the cursor is a handle"),
        );
    }
}

#[test]
fn a_large_board_keeps_the_claim_pages_every_task_once_and_clips_long_titles() {
    let (root, bus, epoch, seats) = fixture("board-view", 1);
    let lane = Actor::Lane(seats[0].lane);
    let long = "é".repeat(400);
    let handles = (0..TASKS)
        .map(|index| {
            let title = if index == TASKS / 2 {
                long.clone()
            } else {
                format!("Board view row {index}")
            };
            add(
                &bus,
                epoch,
                Actor::Operator,
                &format!("VIEW-{index:05}"),
                &title,
                Some("todo"),
            )
        })
        .collect::<Vec<_>>();
    let family = handles[0]
        .as_str()
        .split_once('-')
        .expect("a handle names its family")
        .0
        .to_owned();

    // The allocator's first offer is the row the lane claims.
    let before = posture(&board(&bus, epoch, lane.clone()), None);
    assert_eq!(before.counts.ready, TASKS);
    let held = before.work.rows[0].handle.clone();
    claim_task(&bus, epoch, &seats[0], &lane, &held);

    let current = board(&bus, epoch, lane.clone());
    let after = posture(&current, None);
    let claim = after.claim.as_ref().expect("the claim is shown");
    assert_eq!((&claim.handle, claim.standing), (&held, Standing::Claimed));
    assert_eq!(
        (after.counts.visible, after.counts.ready),
        (TASKS, TASKS - 1)
    );
    assert_eq!(after.work.rows.len(), 10, "the default page is ten rows");
    let omission = after.work.omission.as_ref().expect("most work is withheld");
    assert_eq!(
        (omission.population, omission.withheld),
        (TASKS - 1, TASKS - 11)
    );

    // A count-only read still answers the claim: it is never budgeted.
    let counted = posture(&current, Some(0));
    assert!(counted.work.rows.is_empty());
    assert_eq!(counted.claim.map(|row| row.handle), Some(held.clone()));

    // Continuations visit each ready row exactly once, starting with the
    // first page, and never repeat the claim.
    let work = pages(&bus, epoch, &lane, None, 97);
    let first = after.work.rows.iter().map(|row| &row.handle);
    assert!(work.iter().take(10).eq(first));
    let mut seen = work.clone();
    seen.sort();
    let mut expected = handles.clone();
    expected.retain(|handle| handle != &held);
    expected.sort();
    assert_eq!(seen, expected, "every ready row once, the claim never");

    // A family page leads with the reader's claim and pages the whole family.
    let members = pages(&bus, epoch, &lane, Some(&family), 120);
    assert_eq!(members[0], held);
    assert_eq!(members.len(), TASKS);

    // The long title is clipped on a character boundary with its cut counted.
    let Ok(BoardView::Family(page)) =
        read(&current, Some(&family), Some(1), Some(&handles[TASKS / 2]))
    else {
        panic!("a family read is a family page");
    };
    let title = &page.tasks.rows[0].title;
    assert_eq!(title.text, "é".repeat(60));
    assert_eq!(
        title.withheld_bytes.map(std::num::NonZero::get),
        Some(long.len() - 120)
    );

    // A cursor outside the order is refused rather than silently restarted.
    let refused = read(&current, None, None, Some(&held)).expect_err("a claim is no work row");
    assert!(
        matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == "board::cursor-unknown"),
        "{refused:?}"
    );
    clean(&root, bus);
}
