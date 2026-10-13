use wire::head::ClaimDisposition;

use super::{DesignStanding, RoomProjection};
use crate::work::tests::{board, claim, stamp, task};

#[test]
fn a_design_light_puts_settlement_then_owed_selection_then_review_then_claim() {
    let design = stamp(1);
    let standing = |tasks| RoomProjection::default().design_standing(&design, Some(&board(tasks)));
    let anchor = || task(1, "rooms.design");
    let output = |authorized: bool| {
        let mut output = task(2, "rooms.output");
        output.statement.body.preparation = Some(wire::TaskPreparation {
            parent: design.clone(),
            controller: design.clone(),
            construction: output.statement.body.born.clone(),
        });
        if authorized {
            output.statement.body.engagement = Some(wire::Seq::new(7).expect("fixture seq"));
        }
        output
    };
    let claimed = || {
        let mut anchor = anchor();
        anchor.claim = Some(claim(&anchor, ClaimDisposition::Active));
        anchor
    };
    let reviewing = || {
        let mut anchor = claimed();
        anchor.workflow.body.phase = "review".parse().expect("fixture phase");
        anchor
    };
    let settled = || {
        let mut anchor = reviewing();
        anchor.workflow.body.completion = Some(
            "2026-09-23T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        anchor
    };

    assert_eq!(
        RoomProjection::default().design_standing(&design, None),
        DesignStanding::Unavailable,
        "no board invents no standing",
    );
    assert_eq!(standing(vec![]), DesignStanding::Unavailable);
    assert_eq!(standing(vec![anchor()]), DesignStanding::Open);
    let mut freed = anchor();
    freed.claim = Some(claim(&freed, ClaimDisposition::Freed));
    assert_eq!(standing(vec![freed]), DesignStanding::Open);
    assert_eq!(standing(vec![claimed()]), DesignStanding::Claimed);
    assert_eq!(standing(vec![reviewing()]), DesignStanding::Review);
    assert_eq!(
        standing(vec![reviewing(), output(false)]),
        DesignStanding::Unselected,
        "an output owing the operator's selection outranks review",
    );
    assert_eq!(
        standing(vec![claimed(), output(true)]),
        DesignStanding::Claimed,
        "an engaged output owes no selection",
    );
    assert_eq!(
        standing(vec![settled(), output(false)]),
        DesignStanding::Settled,
        "settlement outranks everything",
    );
}
