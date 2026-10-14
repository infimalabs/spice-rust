use wire::head::ClaimDisposition;
use wire::intent::BoardTask;

use super::{DesignStanding, RoomProjection, prepared_open_count};
use crate::work::tests::{board, claim, stamp, task};

fn design(grouped: bool) -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design.grouped = grouped;
    design
}

fn claimed(mut design: BoardTask) -> BoardTask {
    design.claim = Some(claim(&design, ClaimDisposition::Active));
    design
}

fn concluded(mut design: BoardTask, review: &str, reason: Option<&str>) -> BoardTask {
    design.workflow.body.status = "completed".to_owned();
    design.workflow.body.review = review.to_owned();
    design.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    design.collapse_reason = reason.map(str::to_owned);
    design
}

fn output(authorized: bool) -> BoardTask {
    let mut output = task(2, "rooms.output");
    output.statement.body.preparation = Some(wire::TaskPreparation {
        parent: stamp(1),
        controller: stamp(1),
        construction: output.statement.body.born.clone(),
    });
    if authorized {
        output.statement.body.engagement = Some(wire::Seq::new(7).expect("fixture seq"));
    }
    output
}

fn light(tasks: Vec<BoardTask>) -> DesignStanding {
    RoomProjection::default().design_standing(&stamp(1), Some(&board(tasks)))
}

#[test]
fn group_membership_alone_decides_an_open_designs_activity() {
    assert_eq!(
        RoomProjection::default().design_standing(&stamp(1), None),
        DesignStanding::Unavailable,
        "no board invents no standing",
    );
    assert_eq!(light(vec![]), DesignStanding::Unavailable);
    assert_eq!(
        light(vec![task(1, "rooms.plain")]),
        DesignStanding::Unavailable,
        "a task without Design intent has no Design standing",
    );

    assert_eq!(
        design(false).design_standing(),
        Some(wire::intent::DesignStanding::Passive)
    );
    assert_eq!(light(vec![design(false)]), DesignStanding::Passive);
    assert_eq!(
        light(vec![claimed(design(false)), output(false)]),
        DesignStanding::Passive,
        "a holder parking a Design that left its group owes the operator nothing new",
    );

    assert_eq!(
        design(true).design_standing(),
        Some(wire::intent::DesignStanding::Active)
    );
    assert_eq!(light(vec![design(true)]), DesignStanding::Active);
    assert_eq!(light(vec![claimed(design(true))]), DesignStanding::Claimed);
    assert_eq!(
        light(vec![claimed(design(true)), output(false)]),
        DesignStanding::Unselected,
        "an output owing the operator's selection outranks a claim",
    );
    assert_eq!(
        light(vec![claimed(design(true)), output(true)]),
        DesignStanding::Claimed,
        "an engaged output owes no selection",
    );

    let mut retained = design(true);
    retained.workflow.body.flow = vec![
        "design".parse().expect("fixture phase"),
        "review".parse().expect("fixture phase"),
    ];
    retained.workflow.body.phase = "review".parse().expect("fixture phase");
    assert_eq!(
        light(vec![retained]),
        DesignStanding::Active,
        "a retained pending Design parked in review reads open by its group",
    );
}

#[test]
fn terminal_standing_outranks_retained_group_membership() {
    let done = concluded(design(true), "unassigned", None);
    assert_eq!(
        done.design_standing(),
        Some(wire::intent::DesignStanding::Done)
    );
    assert_eq!(light(vec![done, output(false)]), DesignStanding::Done);

    let collapsed = concluded(claimed(design(true)), "closed", Some("premise failed"));
    assert_eq!(
        collapsed.design_standing(),
        Some(wire::intent::DesignStanding::Collapsed {
            reason: Some("premise failed".to_owned()),
        }),
    );
    assert_eq!(light(vec![collapsed]), DesignStanding::Collapsed);
    assert_eq!(
        concluded(design(false), "closed", None).design_standing(),
        Some(wire::intent::DesignStanding::Collapsed { reason: None }),
        "a retained negative conclusion that recorded no reason invents none",
    );

    let mut deleted = design(true);
    deleted.workflow.body.status = "deleted".to_owned();
    assert_eq!(
        deleted.design_standing(),
        Some(wire::intent::DesignStanding::Deleted)
    );
    assert_eq!(light(vec![deleted]), DesignStanding::Deleted);
}

fn prepared(number: i64, parent: i64, controller: i64, construction: i64) -> BoardTask {
    let mut task = task(number, "rooms.output");
    task.statement.body.preparation = Some(wire::TaskPreparation {
        parent: stamp(parent),
        controller: stamp(controller),
        construction: stamp(construction),
    });
    task
}

#[test]
fn prepared_open_count_admits_each_direct_role_once_and_excludes_retained_rows() {
    let design = stamp(1);
    let mut board = board(vec![
        prepared(2, 1, 20, 21),
        prepared(3, 30, 1, 31),
        prepared(4, 40, 41, 1),
        prepared(5, 1, 1, 1),
        prepared(6, 60, 61, 62),
    ]);
    let mut terminal = prepared(7, 1, 70, 71);
    terminal.workflow.body.status = "completed".to_owned();
    terminal.workflow.body.completion = Some(
        "2026-10-14T00:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    board.room_anchors.push(terminal);

    assert_eq!(prepared_open_count(&board, &design), 4);
}
