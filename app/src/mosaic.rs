//! Pure lattice used by the predecessor-style Mosaic view.
//!
//! Placement is derived only from creation order, measured candidates, and
//! the current geometry. Browser measurement and rendering remain outside
//! this module, which keeps replay deterministic and lets native tests prove
//! the same decisions the Wasm view applies.

use std::collections::{BTreeMap, BTreeSet};

/// Number of tracks in every Mosaic lattice.
pub const TRACK_COUNT: usize = 12;
/// Number of later overlapping layers that latches a card as frozen.
pub const FREEZE_DEPTH: usize = 2;
/// Scroll distance after which incremental pushes preserve the viewport.
pub const NEAR_TOP_THRESHOLD_PX: f64 = 80.0;
/// Base-width height that admits one distinct wider measurement.
pub const TALL_THRESHOLD_PX: f64 = 230.0;

const LEGAL_BASE_SPANS: [usize; 5] = [2, 3, 4, 6, 12];
const GAP_REM: f64 = 0.75;
const MIN_LANE_REM: f64 = 17.0;
const MODULE_LINE_HEIGHT_REM: f64 = 1.25;
const WIDE_FACTOR: usize = 2;
const QUOTE_FONT_SIZE_REM: f64 = 0.8125;
const QUOTE_LINE_HEIGHT_RATIO: f64 = 1.34;
const QUOTE_CHROME_REM: f64 = 1.25;
const QUOTE_TYPICAL_LINES: f64 = 2.0;
const IMAGE_HEIGHT_REM: f64 = 8.75;
const IMAGE_LARGE_HEIGHT_REM: f64 = 15.75;

/// Convert the DOM client box, which includes padding, into usable content width.
#[must_use]
pub fn content_box_width(client_width_px: f64, padding_left_px: f64, padding_right_px: f64) -> f64 {
    (client_width_px - padding_left_px - padding_right_px).max(0.0)
}

/// Geometry resolved from the live root type size and host width.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Integer gap shared by tracks and rows.
    pub gap: i32,
    /// Minimum usable lane width.
    pub min_lane: f64,
    /// Fractional width of one of the twelve tracks.
    pub column_width: f64,
    /// Smallest legal span that clears [`Self::min_lane`].
    pub base_span: usize,
    /// Number of base lanes available at this width.
    pub lane_count: usize,
    /// Type-ramp lines in one vertical module.
    pub module_lines: usize,
    /// Integer vertical module height.
    pub module: i32,
    /// Shared rounded track edges, including both outer edges.
    pub edges: Vec<i32>,
}

impl Geometry {
    /// Resolve one geometry pass.
    #[must_use]
    pub fn new(root_font_size_px: f64, container_width_px: f64) -> Self {
        let rem = positive_or(root_font_size_px, 16.0);
        let width = positive_or(container_width_px, 0.0);
        let gap = (GAP_REM * rem).round() as i32;
        let min_lane = MIN_LANE_REM * rem;
        let column_width = (width - (TRACK_COUNT - 1) as f64 * f64::from(gap)) / TRACK_COUNT as f64;
        let base_span = base_span(column_width, f64::from(gap), min_lane);
        let lane_count = TRACK_COUNT / base_span;
        let module_lines = module_lines(lane_count);
        let module = (module_lines as f64 * MODULE_LINE_HEIGHT_REM * rem).round() as i32;
        let edges = (0..=TRACK_COUNT)
            .map(|track| (track as f64 * (column_width + f64::from(gap))).round() as i32)
            .collect();
        Self {
            gap,
            min_lane,
            column_width,
            base_span,
            lane_count,
            module_lines,
            module,
            edges,
        }
    }

    /// Rendered width for one anchored card, using the shared edge table.
    #[must_use]
    pub fn card_width(&self, track: usize, span: usize) -> i32 {
        let start = track.min(TRACK_COUNT);
        let end = track.saturating_add(span).min(TRACK_COUNT);
        self.edges[end] - self.edges[start] - self.gap
    }
}

fn positive_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn base_span(column_width: f64, gap: f64, min_lane: f64) -> usize {
    LEGAL_BASE_SPANS
        .into_iter()
        .find(|span| *span as f64 * column_width + (*span - 1) as f64 * gap >= min_lane)
        .unwrap_or(TRACK_COUNT)
}

fn module_lines(lane_count: usize) -> usize {
    match lane_count {
        3.. => 3,
        2 => 2,
        _ => 1,
    }
}

/// One measured width/height option offered to the placement key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// Track span measured by the caller.
    pub span: usize,
    /// Natural height converted into vertical modules.
    pub rows: i32,
}

/// The two card lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    /// The card can still settle vertically as deferred content resolves.
    Liquid,
    /// The card has enough burial depth that its placement is latched.
    Frozen,
}

impl Lifecycle {
    /// Stable DOM/CSS spelling of this state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Liquid => "liquid",
            Self::Frozen => "frozen",
        }
    }
}

/// One placed card in the lattice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Card {
    /// Stable stream creation order.
    pub creation_index: usize,
    /// Zero-based anchor track.
    pub track: usize,
    /// Number of tracks occupied.
    pub span: usize,
    /// Base row; rows increase toward the leading edge.
    pub base: i32,
    /// Number of vertical modules occupied.
    pub rows: i32,
    /// Current lifecycle latch.
    pub lifecycle: Lifecycle,
    /// Measurements used when a full replay re-decides placement.
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Anchor {
    base: i32,
    waste: i32,
    track: usize,
    span: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Placement {
    track: usize,
    span: usize,
    base: i32,
    rows: i32,
}

/// Convert a pixel height into vertical modules.
#[must_use]
pub fn rows_for(px: f64, gap: i32, module: i32) -> i32 {
    let module = module.max(1);
    2.max(((px + f64::from(gap)) / f64::from(module)).ceil() as i32)
}

/// Build the base candidate and, only for a tall base measurement, one wide candidate.
pub fn span_candidates(
    geometry: &Geometry,
    mut measure: impl FnMut(usize) -> f64,
) -> Vec<Candidate> {
    span_candidates_with_threshold(geometry, &mut measure, TALL_THRESHOLD_PX)
}

fn span_candidates_with_threshold(
    geometry: &Geometry,
    measure: &mut impl FnMut(usize) -> f64,
    threshold: f64,
) -> Vec<Candidate> {
    let base_px = measure(geometry.base_span);
    let mut candidates = vec![Candidate {
        span: geometry.base_span,
        rows: rows_for(base_px, geometry.gap, geometry.module),
    }];
    let wide_span = (geometry.base_span * WIDE_FACTOR).min(TRACK_COUNT);
    if wide_span > geometry.base_span && base_px >= threshold {
        candidates.push(Candidate {
            span: wide_span,
            rows: rows_for(measure(wide_span), geometry.gap, geometry.module),
        });
    }
    candidates
}

/// Height prior for deferred quote or image content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reservation {
    /// Acknowledgement quote using the standard text ramp.
    Acknowledgement,
    /// Explicit quoted reply using the standard text ramp.
    Quote,
    /// Normal letterboxed image.
    Image,
    /// Large letterboxed image.
    ImageLarge,
}

impl Reservation {
    /// Resolve the reservation against the live root type size.
    #[must_use]
    pub fn pixels(self, root_font_size_px: f64) -> f64 {
        let rem = positive_or(root_font_size_px, 16.0);
        match self {
            Self::Acknowledgement | Self::Quote => {
                let font_size = QUOTE_FONT_SIZE_REM * rem;
                QUOTE_CHROME_REM * rem + QUOTE_TYPICAL_LINES * font_size * QUOTE_LINE_HEIGHT_RATIO
            }
            Self::Image => IMAGE_HEIGHT_REM * rem,
            Self::ImageLarge => IMAGE_LARGE_HEIGHT_REM * rem,
        }
    }
}

fn normalize_tracks(track_count: usize) -> usize {
    track_count.max(1)
}

fn clamp_span(span: usize, tracks: usize) -> usize {
    span.max(1).min(normalize_tracks(tracks))
}

/// Derive the skyline from a placed card set.
#[must_use]
pub fn derive_row_floor(cards: &[Card], track_count: usize) -> Vec<i32> {
    let tracks = normalize_tracks(track_count);
    let mut floor = vec![0; tracks];
    for card in cards {
        let top = card.base + card.rows;
        let end = card.track.saturating_add(card.span).min(tracks);
        for value in floor.iter_mut().take(end).skip(card.track.min(tracks)) {
            *value = (*value).max(top);
        }
    }
    floor
}

fn anchor_at(track: usize, span: usize, floor: &[i32]) -> Anchor {
    let covered = &floor[track..track + span];
    let base = covered.iter().copied().max().unwrap_or(0);
    let waste = covered.iter().map(|value| base - value).sum();
    Anchor {
        base,
        waste,
        track,
        span,
    }
}

fn anchor_for(span: usize, floor: &[i32], track_count: usize) -> Anchor {
    let tracks = normalize_tracks(track_count);
    let span = clamp_span(span, tracks);
    let mut best = anchor_at(0, span, floor);
    for track in 1..=tracks - span {
        let candidate = anchor_at(track, span, floor);
        if (candidate.base, candidate.waste, candidate.track) < (best.base, best.waste, best.track)
        {
            best = candidate;
        }
    }
    best
}

fn decide(candidates: &[Candidate], floor: &[i32], track_count: usize) -> Placement {
    let mut decisions = candidates.iter().map(|candidate| {
        let anchor = anchor_for(candidate.span, floor, track_count);
        (
            (
                anchor.base,
                anchor.waste,
                -(anchor.span as i32),
                anchor.track,
            ),
            Placement {
                track: anchor.track,
                span: anchor.span,
                base: anchor.base,
                rows: candidate.rows,
            },
        )
    });
    let first = decisions
        .next()
        .expect("a Mosaic card has at least one measured candidate");
    decisions
        .fold(first, |best, candidate| {
            if candidate.0 < best.0 {
                candidate
            } else {
                best
            }
        })
        .1
}

fn commit(floor: &[i32], card: &Card, track_count: usize) -> Vec<i32> {
    let tracks = normalize_tracks(track_count);
    let mut next = floor.to_vec();
    let top = card.base + card.rows;
    let end = card.track.saturating_add(card.span).min(tracks);
    for value in next.iter_mut().take(end).skip(card.track.min(tracks)) {
        *value = (*value).max(top);
    }
    next
}

/// Insert measured cards in creation order and latch frozen cards after each insertion.
#[must_use]
pub fn pack(candidate_sets: &[Vec<Candidate>], track_count: usize) -> Vec<Card> {
    let tracks = normalize_tracks(track_count);
    let mut cards = Vec::new();
    let mut floor = vec![0; tracks];
    for (creation_index, candidates) in candidate_sets.iter().enumerate() {
        let placement = decide(candidates, &floor, tracks);
        let card = Card {
            creation_index,
            track: placement.track,
            span: placement.span,
            base: placement.base,
            rows: placement.rows,
            lifecycle: Lifecycle::Liquid,
            candidates: candidates.clone(),
        };
        floor = commit(&floor, &card, tracks);
        cards.push(card);
        cards = recompute_frozen(&cards, FREEZE_DEPTH);
    }
    cards
}

fn tracks_overlap(left: &Card, right: &Card) -> bool {
    left.track < right.track + right.span && right.track < left.track + left.span
}

/// Minimum later-card coverage across every track occupied by a card.
#[must_use]
pub fn burial_depth(card: &Card, cards: &[Card]) -> usize {
    (card.track..card.track + card.span)
        .map(|track| {
            cards
                .iter()
                .filter(|other| {
                    other.creation_index > card.creation_index
                        && track >= other.track
                        && track < other.track + other.span
                })
                .count()
        })
        .min()
        .unwrap_or(0)
}

/// Latch cards whose burial depth reaches the configured threshold.
#[must_use]
pub fn recompute_frozen(cards: &[Card], freeze_depth: usize) -> Vec<Card> {
    cards
        .iter()
        .map(|card| {
            if card.lifecycle == Lifecycle::Frozen || burial_depth(card, cards) >= freeze_depth {
                let mut frozen = card.clone();
                frozen.lifecycle = Lifecycle::Frozen;
                frozen
            } else {
                card.clone()
            }
        })
        .collect()
}

fn rest_liquid(card: &Card, floor: &[i32], track_count: usize) -> (Card, Vec<i32>) {
    let tracks = normalize_tracks(track_count);
    let end = card.track.saturating_add(card.span).min(tracks);
    let base = floor
        .iter()
        .take(end)
        .skip(card.track.min(tracks))
        .copied()
        .max()
        .unwrap_or(0);
    let mut placed = card.clone();
    placed.base = base;
    let floor = commit(floor, &placed, tracks);
    (placed, floor)
}

/// Re-rest liquid cards vertically while retaining their insert-time track and span.
#[must_use]
pub fn replay_liquid(cards: &[Card], track_count: usize, freeze_depth: usize) -> Vec<Card> {
    let frozen: Vec<_> = cards
        .iter()
        .filter(|card| card.lifecycle == Lifecycle::Frozen)
        .cloned()
        .collect();
    let mut floor = derive_row_floor(&frozen, track_count);
    let mut ordered: Vec<_> = cards
        .iter()
        .filter(|card| card.lifecycle == Lifecycle::Liquid)
        .cloned()
        .collect();
    ordered.sort_by_key(|card| card.creation_index);
    let mut placed = BTreeMap::new();
    for card in ordered {
        let (card, next) = rest_liquid(&card, &floor, track_count);
        floor = next;
        placed.insert(card.creation_index, card);
    }
    let replayed = cards
        .iter()
        .map(|card| {
            placed
                .get(&card.creation_index)
                .cloned()
                .unwrap_or_else(|| card.clone())
        })
        .collect::<Vec<_>>();
    recompute_frozen(&replayed, freeze_depth)
}

/// Grow one card downward and move every transitively lower overlapping card.
#[must_use]
pub fn ripple_rows(cards: &[Card], creation_index: usize, delta: i32) -> Vec<Card> {
    let Some(growing) = cards
        .iter()
        .find(|card| card.creation_index == creation_index)
    else {
        return cards.to_vec();
    };
    let mut shifted = BTreeMap::new();
    let mut seen = BTreeSet::from([creation_index]);
    let mut stack = vec![growing.creation_index];
    while let Some(frontier_index) = stack.pop() {
        let frontier = cards
            .iter()
            .find(|card| card.creation_index == frontier_index)
            .expect("ripple frontier originated in the card set");
        for candidate in cards {
            if seen.contains(&candidate.creation_index) {
                continue;
            }
            if tracks_overlap(candidate, frontier) && candidate.base < frontier.base {
                shifted.insert(candidate.creation_index, candidate.base - delta);
                seen.insert(candidate.creation_index);
                stack.push(candidate.creation_index);
            }
        }
    }
    cards
        .iter()
        .map(|card| {
            let mut next = card.clone();
            if card.creation_index == creation_index {
                next.base -= delta;
                next.rows += delta;
            } else if let Some(base) = shifted.get(&card.creation_index) {
                next.base = *base;
            }
            next
        })
        .collect()
}

/// Resolve a frozen card resize, retaining its reservation for shrinkage.
#[must_use]
pub fn resolve_frozen_resize(cards: &[Card], creation_index: usize, new_rows: i32) -> Vec<Card> {
    let Some(card) = cards
        .iter()
        .find(|card| card.creation_index == creation_index)
    else {
        return cards.to_vec();
    };
    if new_rows <= card.rows {
        cards.to_vec()
    } else {
        ripple_rows(cards, creation_index, new_rows - card.rows)
    }
}

/// Clear lifecycle latches and re-decide every card from current measurements.
#[must_use]
pub fn full_replay(cards: &[Card], track_count: usize, freeze_depth: usize) -> Vec<Card> {
    let tracks = normalize_tracks(track_count);
    let mut ordered = cards.to_vec();
    ordered.sort_by_key(|card| card.creation_index);
    let mut floor = vec![0; tracks];
    let mut placed = BTreeMap::new();
    for card in ordered {
        let decision = decide(&card.candidates, &floor, tracks);
        let card = Card {
            creation_index: card.creation_index,
            track: decision.track,
            span: decision.span,
            base: decision.base,
            rows: decision.rows,
            lifecycle: Lifecycle::Liquid,
            candidates: card.candidates,
        };
        floor = commit(&floor, &card, tracks);
        placed.insert(card.creation_index, card);
    }
    let replayed = cards
        .iter()
        .map(|card| {
            placed
                .get(&card.creation_index)
                .cloned()
                .expect("full replay placed every input card")
        })
        .collect::<Vec<_>>();
    recompute_frozen(&replayed, freeze_depth)
}

/// Highest and lowest occupied lattice rows.
#[must_use]
pub fn extent(cards: &[Card]) -> (i32, i32) {
    cards.iter().fold((0, 0), |(max_row, min_base), card| {
        (max_row.max(card.base + card.rows), min_base.min(card.base))
    })
}

/// One-based CSS Grid row where this upward-counting card begins.
#[must_use]
pub fn css_row_start(card: &Card, max_row: i32) -> i32 {
    max_row - (card.base + card.rows) + 1
}

/// Whether the pane is far enough from its leading edge to preserve its viewport.
#[must_use]
pub fn pane_is_scrolled(scroll_top: f64, threshold: Option<f64>) -> bool {
    scroll_top
        > threshold
            .filter(|value| value.is_finite())
            .unwrap_or(NEAR_TOP_THRESHOLD_PX)
}

/// Same-frame scroll compensation for an incremental plane shift.
#[must_use]
pub fn scroll_compensation(
    previous_max_row: i32,
    max_row: i32,
    module: i32,
    scrolled: bool,
    replaying: bool,
) -> Option<f64> {
    (scrolled && !replaying)
        .then_some(f64::from((max_row - previous_max_row) * module))
        .filter(|delta| delta.is_finite())
}
