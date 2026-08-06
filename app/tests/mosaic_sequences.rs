//! That the lattice holds over the predecessor's own randomized sequences.
//!
//! `mosaic.rs` is covered by hand-built cases that each pin one rule. This
//! covers the rules together, over the sequences the predecessor's
//! `tests/fixtures/mosaic_engine.js` actually runs: its `mulberry32`, its
//! seeds, its candidate generation, forty insertions apiece. Keeping the
//! generator rather than inventing one is the point, because a packer that
//! satisfies every rule in isolation can still break where they interact, and
//! these are the interleavings the predecessor is known to survive.
//!
//! The properties are the ones a packer loses quietly rather than loudly: two
//! cards in the same cell, a card that shifts after it was already on screen,
//! a replay that diverges, and a wide tier that drops into a hole to take
//! width. None of those panic on their own.
//!
//! The last of those is the reason this file is not redundant with `mosaic.rs`
//! and its own suite. Reordering the placement key so that span outranks the
//! frontier -- the one change that turns "prefer width when it costs nothing"
//! into "prefer width" -- leaves every hand-built case passing, and fails only
//! [`a_wide_tier_never_digs_a_hole_to_take_width`]. A rule that no test can
//! see is a rule the next edit removes for free.

use app::mosaic::{self, Candidate, Card, TRACK_COUNT};

const LEGAL_BASE_SPANS: [usize; 5] = [2, 3, 4, 6, 12];
const TALL_ROW_COUNT_THRESHOLD: i32 = 4;
const SEEDS: [u32; 7] = [1, 2, 3, 4, 5, 42, 1337];
const RUN_LENGTH: usize = 40;

/// The fixture's `mulberry32`, ported truncation for truncation.
///
/// The seeds only mean anything if they reproduce the predecessor's sequences
/// rather than merely producing some sequence, so the wrapping arithmetic is
/// exact rather than approximated with a convenient generator.
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn pick(&mut self, list: &[usize]) -> usize {
        let index = (self.next() * list.len() as f64) as usize;
        list[index.min(list.len() - 1)]
    }

    fn between(&mut self, low: i32, high: i32) -> i32 {
        low + (self.next() * f64::from(high - low + 1)) as i32
    }
}

/// The fixture's candidate generation: a base width always, and a wide tier
/// only when the card's own row count clears the threshold.
///
/// Driven by the generator rather than by measurement, because measurement is
/// a sizing concern the lattice deliberately does not own. The gate reads the
/// card's own height and nothing about the card, which is the prohibition the
/// predecessor recorded when it deleted its span-by-class packer.
fn random_candidates(rng: &mut Mulberry32) -> Vec<Candidate> {
    let base_span = rng.pick(&LEGAL_BASE_SPANS);
    let base_rows = rng.between(1, 6);
    let mut candidates = vec![Candidate {
        span: base_span,
        rows: base_rows,
    }];
    let wide_span = (base_span * 2).min(TRACK_COUNT);
    if wide_span > base_span && base_rows >= TALL_ROW_COUNT_THRESHOLD {
        candidates.push(Candidate {
            span: wide_span,
            rows: (base_rows - 1 - rng.between(0, 1)).max(2),
        });
    }
    candidates
}

/// One seeded run's candidate sets, in creation order.
fn sequence(seed: u32, count: usize) -> Vec<Vec<Candidate>> {
    let mut rng = Mulberry32(seed);
    (0..count).map(|_| random_candidates(&mut rng)).collect()
}

fn cells_overlap(left: &Card, right: &Card) -> bool {
    let tracks = left.track < right.track + right.span && right.track < left.track + left.span;
    let rows = left.base < right.base + right.rows && right.base < left.base + left.rows;
    tracks && rows
}

#[test]
fn no_two_cards_ever_occupy_the_same_cell() {
    for seed in SEEDS {
        let cards = mosaic::pack(&sequence(seed, RUN_LENGTH), TRACK_COUNT);
        for (index, card) in cards.iter().enumerate() {
            for (other_index, other) in cards.iter().enumerate().skip(index + 1) {
                assert!(
                    !cells_overlap(card, other),
                    "cards {index} and {other_index} overlap in both axes \
                     (seed {seed}): {card:?} {other:?}"
                );
            }
        }
    }
}

#[test]
fn packing_the_same_sequence_twice_reproduces_the_lattice() {
    for seed in SEEDS {
        let candidates = sequence(seed, RUN_LENGTH);
        assert_eq!(
            mosaic::pack(&candidates, TRACK_COUNT),
            mosaic::pack(&candidates, TRACK_COUNT),
            "replay diverged for seed {seed}"
        );
    }
}

#[test]
fn a_run_places_every_card_it_was_given() {
    // Determinism and non-overlap are both satisfied by a packer that drops
    // every card, so neither one can notice a lost card. This can.
    for seed in SEEDS {
        let cards = mosaic::pack(&sequence(seed, RUN_LENGTH), TRACK_COUNT);
        assert_eq!(cards.len(), RUN_LENGTH, "seed {seed} lost a card");
        for (index, card) in cards.iter().enumerate() {
            assert_eq!(card.creation_index, index, "seed {seed} reordered cards");
        }
    }
}

#[test]
fn a_later_card_never_moves_the_placement_of_an_earlier_one() {
    // The batch equivalent of the fixture's non-mutation check: packing the
    // first n cards must agree with the first n placements of a longer run,
    // except where a card has since frozen, which only latches a placement
    // rather than moving it.
    for seed in SEEDS {
        let full = sequence(seed, RUN_LENGTH);
        let long = mosaic::pack(&full, TRACK_COUNT);
        for prefix_len in 1..RUN_LENGTH {
            let short = mosaic::pack(&full[..prefix_len], TRACK_COUNT);
            for (index, card) in short.iter().enumerate() {
                let later = &long[index];
                assert_eq!(
                    (card.track, card.span),
                    (later.track, later.span),
                    "seed {seed}: card {index} moved horizontally once \
                     {prefix_len} cards had been placed"
                );
            }
        }
    }
}

#[test]
fn a_wide_tier_never_digs_a_hole_to_take_width() {
    // The invariant the placement key exists to guarantee: width sorts after
    // the frontier and the waste, so a wide candidate wins only by landing
    // strictly lower than the best base placement, or tying it with no more
    // waste. Checked against the floor as it stood when each card was placed.
    for seed in SEEDS {
        let sets = sequence(seed, RUN_LENGTH);
        for taken in 0..RUN_LENGTH {
            let candidates = &sets[taken];
            let Some(wide) = candidates.get(1) else {
                continue;
            };
            let placed = mosaic::pack(&sets[..=taken], TRACK_COUNT);
            let card = &placed[taken];
            if card.span != wide.span {
                continue;
            }
            let floor = mosaic::derive_row_floor(&placed[..taken], TRACK_COUNT);
            let base_anchor = anchor(candidates[0].span, &floor);
            let wide_anchor = anchor(wide.span, &floor);
            assert!(
                wide_anchor.0 < base_anchor.0
                    || (wide_anchor.0 == base_anchor.0 && wide_anchor.1 <= base_anchor.1),
                "seed {seed}: wide placement dug a hole at card {taken}: \
                 base {base_anchor:?} wide {wide_anchor:?}"
            );
        }
    }
}

/// The frontier and waste a span would take on this floor, at its best track.
///
/// Restated here rather than read from the module under test, so the check has
/// its own account of what a good placement is.
fn anchor(span: usize, floor: &[i32]) -> (i32, i32) {
    (0..=floor.len() - span)
        .map(|track| {
            let covered = &floor[track..track + span];
            let base = covered.iter().copied().max().unwrap_or(0);
            (base, covered.iter().map(|value| base - value).sum::<i32>())
        })
        .min()
        .unwrap_or((0, 0))
}
