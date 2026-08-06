use app::mosaic::{Candidate, Lifecycle};
use app::mosaic_plane::arrange;

fn candidates(span: usize, rows: i32) -> Vec<Candidate> {
    vec![Candidate { span, rows }]
}

/// This is the production dependency witness, not another engine-unit test.
/// `mosaic_plane::arrange` is the same function each browser lane component
/// calls. Removing its last call into `app/src/mosaic.rs` either fails to build
/// or makes these engine-decided placements and lifecycle states go red.
#[test]
fn production_lane_plane_is_decided_by_the_mosaic_engine() {
    let plane = arrange(&[
        candidates(3, 2),
        candidates(3, 4),
        candidates(6, 3),
        candidates(12, 2),
    ]);

    let placements = plane
        .cards
        .iter()
        .map(|card| (card.track, card.span, card.base, card.rows, card.lifecycle))
        .collect::<Vec<_>>();
    assert_eq!(
        placements,
        vec![
            (0, 3, 0, 2, Lifecycle::Liquid),
            (3, 3, 0, 4, Lifecycle::Liquid),
            (6, 6, 0, 3, Lifecycle::Liquid),
            (0, 12, 4, 2, Lifecycle::Liquid),
        ]
    );
    assert_eq!((plane.max_row, plane.min_base, plane.rows()), (6, 0, 6));
}

#[test]
fn sibling_lane_planes_never_share_a_row_floor() {
    let busy = arrange(&[candidates(12, 5), candidates(12, 4), candidates(12, 3)]);
    let quiet = arrange(&[candidates(12, 2)]);

    assert_eq!(busy.max_row, 12);
    assert_eq!(quiet.cards[0].base, 0);
    assert_eq!(quiet.max_row, 2);
}
