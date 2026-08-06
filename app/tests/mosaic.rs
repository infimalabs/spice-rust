//! That the mosaic lattice places cards the way the predecessor's did.
//!
//! Ported from the predecessor's `tests/fixtures/mosaic_engine.js`, which is
//! the executable spec for `app.mosaic-engine.js`. It keeps that fixture's
//! seeds, its seeded PRNG, and its candidate generation, so a divergence here
//! is a divergence from the layout the operator has actually been looking at
//! rather than from a restatement of it.
//!
//! The properties are what a packer can get wrong quietly: cards that overlap
//! in both axes, a placement that mutates a card already on screen, a replay
//! that diverges, and a wide tier that digs a hole to take width. None of
//! those show up as a crash, which is the reason for asserting them directly.

use std::collections::BTreeSet;

use app::mosaic::{
    self, Candidate, Candidates, Card, DEFAULT_TRACK_COUNT, anchor_for, derive_row_floor,
};

const TRACKS: u32 = 12;
const LEGAL_BASE_SPANS: [u32; 5] = [2, 3, 4, 6, 12];
const TALL_ROW_COUNT_THRESHOLD: u32 = 4;
const SEEDS: [u32; 7] = [1, 2, 3, 4, 5, 42, 1337];

/// The fixture's `mulberry32`, so a "randomized" sequence replays identically.
///
/// Ported exactly, including the `>>> 0` truncations, because the point of the
/// seeds is that they reproduce the predecessor's sequences rather than merely
/// producing some sequence.
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    /// One element of `list`, chosen the way the fixture chooses.
    fn pick(&mut self, list: &[u32]) -> u32 {
        let index = (self.next() * list.len() as f64) as usize;
        list[index.min(list.len() - 1)]
    }

    /// An integer in `[low, high]`.
    fn between(&mut self, low: u32, high: u32) -> u32 {
        low + (self.next() * f64::from(high - low + 1)) as u32
    }
}

/// The fixture's candidate generation: a base width always, and the wide tier
/// only when the card's own row count clears the threshold.
///
/// Driven by the PRNG rather than by measurement, because measurement is a
/// sizing concern the engine deliberately does not own.
fn random_candidates(rng: &mut Mulberry32) -> Candidates {
    let base_span = rng.pick(&LEGAL_BASE_SPANS);
    let base_rows = rng.between(1, 6);
    let wide_span = (base_span * 2).min(TRACKS);
    let tall = base_rows >= TALL_ROW_COUNT_THRESHOLD;
    let wide = (wide_span > base_span && tall).then(|| {
        let wide_rows = base_rows.saturating_sub(1 + rng.between(0, 1)).max(2);
        Candidate {
            span: wide_span,
            rows: wide_rows,
        }
    });
    Candidates {
        base: Candidate {
            span: base_span,
            rows: base_rows,
        },
        wide,
    }
}

/// Run one seeded sequence, asserting the per-step properties as it goes.
fn run_sequence(seed: u32, count: usize) -> (Vec<Card>, Vec<u32>) {
    let mut rng = Mulberry32(seed);
    let mut cards: Vec<Card> = Vec::new();
    let mut floor = derive_row_floor(&cards, TRACKS);

    for step in 0..count {
        let candidates = random_candidates(&mut rng);
        let before = cards.clone();
        let result = mosaic::insert(&cards, &floor, candidates, TRACKS);

        assert_eq!(
            &result.cards[..before.len()],
            &before[..],
            "insertion moved an existing card at step {step} (seed {seed})"
        );

        // A wide placement may only win by landing strictly lower than the
        // best base placement, or by tying it with no more waste.
        if let Some(wide) = candidates.wide
            && result.card.span == wide.span
            && wide.span != candidates.base.span
        {
            let base = anchor_for(candidates.base.span, &floor, TRACKS);
            let taken = anchor_for(wide.span, &floor, TRACKS);
            assert!(
                taken.base < base.base || (taken.base == base.base && taken.waste <= base.waste),
                "wide placement dug a hole at step {step} (seed {seed}): \
                 base {base:?} wide {taken:?}"
            );
        }

        floor = result.floor;
        cards = result.cards;
    }

    (cards, floor)
}

#[test]
fn no_two_cards_ever_occupy_the_same_cell() {
    for seed in SEEDS {
        let (cards, _) = run_sequence(seed, 40);
        for (index, card) in cards.iter().enumerate() {
            for (other_index, other) in cards.iter().enumerate().skip(index + 1) {
                assert!(
                    !card.overlaps(*other),
                    "cards {index} and {other_index} overlap in both axes \
                     (seed {seed}): {card:?} {other:?}"
                );
            }
        }
    }
}

#[test]
fn replaying_a_sequence_reproduces_the_lattice_exactly() {
    for seed in SEEDS {
        let (first_cards, first_floor) = run_sequence(seed, 40);
        let (second_cards, second_floor) = run_sequence(seed, 40);
        assert_eq!(first_cards, second_cards, "replay diverged for seed {seed}");
        assert_eq!(
            first_floor, second_floor,
            "replay floor diverged for seed {seed}"
        );
    }
}

#[test]
fn a_sequence_places_every_card_it_was_given() {
    // Determinism is not enough on its own: an engine that dropped every card
    // would replay identically and never overlap. This is the count oracle
    // those two properties are missing.
    for seed in SEEDS {
        let (cards, _) = run_sequence(seed, 40);
        assert_eq!(cards.len(), 40, "seed {seed} lost a card");
    }
}

#[test]
fn the_first_card_anchors_the_origin_whatever_its_span() {
    for span in LEGAL_BASE_SPANS {
        let floor = derive_row_floor(&[], TRACKS);
        let placed = mosaic::insert(&[], &floor, Candidates::base(span, 3), TRACKS);
        assert_eq!(placed.card.track, 0, "span {span} did not anchor track 0");
        assert_eq!(placed.card.base, 0, "span {span} did not anchor row 0");
    }
}

#[test]
fn a_span_wider_than_the_lattice_clamps_to_it() {
    let floor = derive_row_floor(&[], TRACKS);
    let anchor = anchor_for(15, &floor, TRACKS);
    assert_eq!(anchor.span, TRACKS, "an oversized span must clamp");
    assert_eq!(anchor.track, 0, "a full-width span has one legal anchor");
}

#[test]
fn the_floor_is_the_skyline_of_the_cards_under_it() {
    // Hand-placed rather than produced by the engine, so the derivation is
    // checked against an independent statement of the answer.
    let cards = [
        Card {
            track: 0,
            span: 4,
            base: 0,
            rows: 2,
        },
        Card {
            track: 4,
            span: 4,
            base: 0,
            rows: 3,
        },
        Card {
            track: 8,
            span: 4,
            base: 0,
            rows: 1,
        },
    ];
    assert_eq!(
        derive_row_floor(&cards, TRACKS),
        vec![2, 2, 2, 2, 3, 3, 3, 3, 1, 1, 1, 1]
    );
}

#[test]
fn committing_a_card_leaves_the_floor_it_was_given_alone() {
    let before = vec![0, 0, 0, 0];
    let after = mosaic::commit(
        &before,
        Card {
            track: 0,
            span: 2,
            base: 0,
            rows: 3,
        },
        4,
    );
    assert_eq!(before, vec![0, 0, 0, 0], "commit disturbed its own input");
    assert_eq!(after, vec![3, 3, 0, 0], "commit advanced the wrong tracks");
}

#[test]
fn an_unusable_lattice_width_falls_back_to_the_default() {
    // Zero is the whole degenerate space a `u32` can express. The predecessor
    // also had to reject fractional and non-finite widths; those are not
    // representable here, which is the port doing the work rather than the
    // check having been dropped.
    assert_eq!(mosaic::track_count(0), DEFAULT_TRACK_COUNT);
    assert_eq!(derive_row_floor(&[], 0).len(), DEFAULT_TRACK_COUNT as usize);

    let floor = derive_row_floor(&[], 0);
    let anchor = anchor_for(1, &floor, 0);
    assert_eq!((anchor.track, anchor.span), (0, 1));

    let decided = mosaic::decide(Candidates::base(2, 1), &floor, 0);
    assert_eq!((decided.track, decided.span, decided.rows), (0, 2, 1));
}

#[test]
fn width_is_never_granted_by_anything_but_the_cards_own_height() {
    // The predecessor deleted a packer that granted tracks by card class and
    // recorded that those preferences must never return. The engine cannot
    // express such a preference: two cards offering identical shapes onto
    // identical floors get identical placements, whatever they contain.
    let floor = derive_row_floor(&[], TRACKS);
    let shapes = [
        Candidates::base(3, 5),
        Candidates {
            base: Candidate { span: 3, rows: 5 },
            wide: Some(Candidate { span: 6, rows: 4 }),
        },
    ];
    for shape in shapes {
        let placements: BTreeSet<Card> = (0..8)
            .map(|_| mosaic::decide(shape, &floor, TRACKS))
            .collect();
        assert_eq!(
            placements.len(),
            1,
            "the same shape placed onto the same floor landed differently"
        );
    }
}
