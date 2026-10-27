use wire::head::ClaimDisposition;
use wire::intent::BoardTask;
use wire::rooms::DesignCycle;

use super::{DesignStanding, RoomProjection, associated, design_dependent_count};
use crate::work::tests::{board, claim, stamp, task};

fn design(active: bool) -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design.grouped = active;
    design.design_active = active;
    design
}

fn claimed(mut design: BoardTask) -> BoardTask {
    design.claim = Some(claim(&design, ClaimDisposition::Active));
    design
}

fn concluded(
    mut design: BoardTask,
    conclusion: fn(wire::Instant) -> wire::WorkflowStatus,
    reason: Option<&str>,
) -> BoardTask {
    let at = "2026-09-23T06:00:00.000Z"
        .parse()
        .expect("fixture completion");
    design.workflow.body.status = conclusion(at);
    design.collapse_reason = reason.map(str::to_owned);
    design
}

fn output() -> BoardTask {
    let mut output = task(2, "rooms.output");
    output.statement.body.preparation = Some(wire::TaskPreparation {
        parent: stamp(1),
        controller: stamp(1),
        construction: output.statement.body.born.clone(),
    });
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
        light(vec![claimed(design(false)), output()]),
        DesignStanding::Passive,
        "a holder parking a Design whose group paused owes the operator nothing new",
    );

    assert_eq!(
        design(true).design_standing(),
        Some(wire::intent::DesignStanding::Active)
    );
    assert_eq!(
        light(vec![design(true)]),
        DesignStanding::Unavailable,
        "an active Design the resident judged no cycle for invents none",
    );
    let judged = |design: BoardTask| {
        let mut view = board(vec![design]);
        view.design_cycles
            .insert(stamp(1), DesignCycle::AwaitingDecision);
        RoomProjection::default().design_standing(&stamp(1), Some(&view))
    };
    assert_eq!(
        judged(design(true)),
        DesignStanding::Active(DesignCycle::AwaitingDecision),
        "an active Design shows the resident's cycle",
    );
    assert_eq!(
        judged(claimed(design(true))),
        DesignStanding::Claimed,
        "a lane's live claim shows the pass it holds",
    );
    let mut ended = design(true);
    ended.claim = Some(claim(&ended, ClaimDisposition::Freed));
    assert_eq!(
        judged(ended.clone()),
        DesignStanding::Active(DesignCycle::AwaitingDecision),
        "an ended claim keeps its head but holds no pass",
    );
    assert_eq!(
        ended.design_availability(),
        wire::rooms::DesignAvailability::Unclaimed
    );

    let mut retained = design(true);
    retained.workflow.body.flow = vec![
        "design".parse().expect("fixture phase"),
        "review".parse().expect("fixture phase"),
    ];
    retained.workflow.body.phase = "review".parse().expect("fixture phase");
    assert_eq!(
        light(vec![retained]),
        DesignStanding::Unavailable,
        "Design intent outside the current design phase has no open standing",
    );
}

#[test]
fn terminal_standing_outranks_retained_group_membership() {
    let done = concluded(design(true), wire::WorkflowStatus::completed, None);
    assert_eq!(
        done.design_standing(),
        Some(wire::intent::DesignStanding::Done)
    );
    assert_eq!(light(vec![done, output()]), DesignStanding::Done);

    let collapsed = concluded(
        claimed(design(true)),
        wire::WorkflowStatus::closed,
        Some("premise failed"),
    );
    assert_eq!(
        collapsed.design_standing(),
        Some(wire::intent::DesignStanding::Collapsed {
            reason: Some("premise failed".to_owned()),
        }),
    );
    assert_eq!(light(vec![collapsed]), DesignStanding::Collapsed);
    assert_eq!(
        concluded(design(false), wire::WorkflowStatus::closed, None).design_standing(),
        Some(wire::intent::DesignStanding::Collapsed { reason: None }),
        "a retained negative conclusion that recorded no reason invents none",
    );

    let mut deleted = design(true);
    deleted.workflow.body.status = wire::WorkflowStatus::deleted(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    assert_eq!(
        deleted.design_standing(),
        Some(wire::intent::DesignStanding::Deleted)
    );
    assert_eq!(light(vec![deleted]), DesignStanding::Deleted);
}

#[test]
fn design_membership_tracks_each_current_dependency_edge() {
    let first = stamp(1);
    let second = stamp(9);
    let mut shared = task(2, "rooms.shared");
    shared.statement.body.dependencies = vec![first.clone(), second.clone()];
    let mut selected_by_first = task(3, "rooms.selected");
    selected_by_first.statement.body.dependencies = vec![second.clone()];
    selected_by_first.statement.body.preparation = Some(wire::TaskPreparation {
        parent: first.clone(),
        controller: first.clone(),
        construction: selected_by_first.statement.body.born.clone(),
    });
    let unrelated = task(4, "rooms.unrelated");
    let board = board(vec![shared.clone(), selected_by_first.clone(), unrelated]);

    assert_eq!(design_dependent_count(&board, &first), 1);
    assert_eq!(design_dependent_count(&board, &second), 2);
    assert!(associated(&shared.statement.body, &first));
    assert!(associated(&shared.statement.body, &second));
    assert!(
        !associated(&selected_by_first.statement.body, &first),
        "provenance does not keep a selected output in its former Design room",
    );
}
