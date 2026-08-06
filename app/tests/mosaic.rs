use std::cell::Cell;

use app::mosaic::{
    Candidate, Card, FREEZE_DEPTH, Geometry, Lifecycle, Reservation, TRACK_COUNT, burial_depth,
    css_row_start, extent, full_replay, pack, pane_is_scrolled, replay_liquid,
    resolve_frozen_resize, ripple_rows, scroll_compensation, span_candidates,
};

fn candidates(span: usize, rows: i32) -> Vec<Candidate> {
    vec![Candidate { span, rows }]
}

fn card(index: usize, track: usize, span: usize, base: i32, rows: i32) -> Card {
    Card {
        creation_index: index,
        track,
        span,
        base,
        rows,
        lifecycle: Lifecycle::Liquid,
        candidates: candidates(span, rows),
    }
}

#[test]
fn geometry_uses_the_minimum_lane_inequality_and_shared_edges() {
    let four_lanes = Geometry::new(16.0, 1440.0);
    assert_eq!(four_lanes.gap, 12);
    assert_eq!(four_lanes.base_span, 3);
    assert_eq!(four_lanes.lane_count, 4);
    assert_eq!(four_lanes.module_lines, 3);
    assert_eq!(four_lanes.module, 60);
    assert_eq!(four_lanes.edges[0], 0);
    assert_eq!(four_lanes.edges[TRACK_COUNT], 1452);
    assert_eq!(four_lanes.card_width(0, TRACK_COUNT), 1440);

    assert_eq!(Geometry::new(16.0, 1000.0).base_span, 4);
    assert_eq!(Geometry::new(16.0, 700.0).base_span, 6);
    assert_eq!(Geometry::new(16.0, 300.0).base_span, 12);
}

#[test]
fn flat_frontiers_choose_the_widest_candidate_then_the_leftmost_anchor() {
    let geometry = Geometry::new(16.0, 1440.0);
    let sets = vec![
        vec![
            Candidate { span: 3, rows: 2 },
            Candidate { span: 6, rows: 2 },
        ],
        candidates(3, 2),
    ];
    let cards = pack(&sets, TRACK_COUNT);
    assert_eq!((cards[0].track, cards[0].span, cards[0].base), (0, 6, 0));
    assert_eq!((cards[1].track, cards[1].span, cards[1].base), (6, 3, 0));
    assert_eq!(geometry.lane_count, 4);
}

#[test]
fn repeated_base_cards_fill_columns_before_advancing_the_skyline() {
    let sets = vec![candidates(3, 2); 5];
    let cards = pack(&sets, TRACK_COUNT);
    let positions: Vec<_> = cards.iter().map(|card| (card.track, card.base)).collect();
    assert_eq!(positions, vec![(0, 0), (3, 0), (6, 0), (9, 0), (0, 2)]);
}

#[test]
fn reservations_follow_the_live_type_ramp_and_exact_letterboxes() {
    let quote = Reservation::Quote.pixels(16.0);
    assert!((quote - 54.84).abs() < 0.001);
    assert_eq!(Reservation::Acknowledgement.pixels(16.0), quote);
    assert_eq!(Reservation::Image.pixels(16.0), 140.0);
    assert_eq!(Reservation::ImageLarge.pixels(16.0), 252.0);
    assert_eq!(Reservation::Image.pixels(f64::NAN), 140.0);
}

#[test]
fn wide_measurement_occurs_only_for_a_tall_card_and_at_most_once() {
    let geometry = Geometry::new(16.0, 1440.0);
    let calls = Cell::new(0);
    let measured = span_candidates(&geometry, |span| {
        calls.set(calls.get() + 1);
        if span == geometry.base_span {
            240.0
        } else {
            180.0
        }
    });
    assert_eq!(calls.get(), 2);
    assert_eq!(
        measured,
        vec![
            Candidate { span: 3, rows: 5 },
            Candidate { span: 6, rows: 4 }
        ]
    );

    let calls = Cell::new(0);
    let measured = span_candidates(&geometry, |_| {
        calls.set(calls.get() + 1);
        120.0
    });
    assert_eq!(calls.get(), 1);
    assert_eq!(measured.len(), 1);
}

#[test]
fn freeze_is_a_latch_driven_by_minimum_burial_depth() {
    let cards = pack(&vec![candidates(12, 2); 3], TRACK_COUNT);
    assert_eq!(burial_depth(&cards[0], &cards), 2);
    assert_eq!(cards[0].lifecycle, Lifecycle::Frozen);
    assert_eq!(cards[1].lifecycle, Lifecycle::Liquid);
    assert_eq!(cards[2].lifecycle, Lifecycle::Liquid);
    assert_eq!(Lifecycle::Liquid.as_str(), "liquid");
    assert_eq!(Lifecycle::Frozen.as_str(), "frozen");
}

#[test]
fn liquid_replay_moves_only_the_vertical_base_and_ignores_input_order() {
    let mut frozen = card(0, 0, 6, 0, 4);
    frozen.lifecycle = Lifecycle::Frozen;
    let first = card(1, 0, 3, 0, 2);
    let second = card(2, 3, 3, 0, 2);
    let replayed = replay_liquid(
        &[second.clone(), frozen.clone(), first.clone()],
        TRACK_COUNT,
        9,
    );
    let by_index = |index| {
        replayed
            .iter()
            .find(|card| card.creation_index == index)
            .unwrap()
    };
    assert_eq!(
        (by_index(1).track, by_index(1).span, by_index(1).base),
        (0, 3, 4)
    );
    assert_eq!(
        (by_index(2).track, by_index(2).span, by_index(2).base),
        (3, 3, 4)
    );
    assert_eq!(by_index(0), &frozen);
}

#[test]
fn frozen_growth_ripples_transitively_over_the_original_layout() {
    let growing = card(0, 0, 3, 4, 2);
    let below = card(1, 1, 3, 2, 2);
    let further = card(2, 3, 3, 1, 2);
    let separate = card(3, 9, 3, 0, 2);
    let cards = vec![growing, below, further, separate.clone()];
    let resized = ripple_rows(&cards, 0, 2);
    assert_eq!((resized[0].base, resized[0].rows), (2, 4));
    assert_eq!(resized[1].base, 0);
    assert_eq!(resized[2].base, -1);
    assert_eq!(resized[3], separate);
    assert_eq!(resolve_frozen_resize(&cards, 0, 1), cards);
}

#[test]
fn full_replay_depends_on_creation_order_and_current_candidates_only() {
    let mut cards = pack(&vec![candidates(3, 2); 5], TRACK_COUNT);
    cards.reverse();
    for card in &mut cards {
        card.track = 9;
        card.base = -30;
        card.lifecycle = Lifecycle::Frozen;
    }
    let replayed = full_replay(&cards, TRACK_COUNT, FREEZE_DEPTH);
    let replayed_again = full_replay(&replayed, TRACK_COUNT, FREEZE_DEPTH);
    assert_eq!(replayed, replayed_again);
    let positions: Vec<_> = replayed
        .iter()
        .map(|card| (card.creation_index, card.track, card.base))
        .collect();
    assert_eq!(
        positions,
        vec![(4, 0, 2), (3, 9, 0), (2, 6, 0), (1, 3, 0), (0, 0, 0)]
    );
}

#[test]
fn extent_grid_rows_and_scroll_compensation_share_the_module() {
    let cards = vec![card(0, 0, 3, -1, 3), card(1, 3, 3, 2, 4)];
    assert_eq!(extent(&cards), (6, -1));
    assert_eq!(css_row_start(&cards[1], 6), 1);
    assert!(!pane_is_scrolled(80.0, None));
    assert!(pane_is_scrolled(80.1, None));
    assert_eq!(scroll_compensation(4, 6, 60, true, false), Some(120.0));
    assert_eq!(scroll_compensation(4, 6, 60, false, false), None);
    assert_eq!(scroll_compensation(4, 6, 60, true, true), None);
}
