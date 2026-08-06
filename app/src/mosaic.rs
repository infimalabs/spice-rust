//! The mosaic lattice: where a card lands, decided without a DOM.
//!
//! This is a port of the predecessor's `app.mosaic-engine.js`, and it keeps
//! that module's defining property: layout is a pure function of the ordered
//! insertion sequence and the current geometry, and of nothing else. No
//! randomness, no wall-clock read, no dependence on resize history. Replaying
//! the same sequence reproduces the same lattice card for card, which is what
//! makes a mosaic testable at all.
//!
//! Rows count upward, so a larger [`Card::base`] is nearer the top. Measuring
//! how many rows a card's content needs is a sizing concern and stays out of
//! this module; candidates arrive already measured.
//!
//! One prohibition carries over from the source and is worth restating,
//! because it is the kind of thing a reimplementation re-invents by accident:
//! **no card type is special.** A card is offered a wider tier because its own
//! measured height cleared a threshold, never because of its class, kind, or
//! content type. The predecessor deleted a packer that granted width by
//! category and recorded that those preferences "must never be ported back in
//! as biases, priors, or thresholds-by-class". They are not here either.

/// The lattice width used when a caller offers none.
pub const DEFAULT_TRACK_COUNT: u32 = 12;

/// A card placed on the lattice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Card {
    /// Anchor track, counting from the left.
    pub track: u32,
    /// How many tracks the card covers.
    pub span: u32,
    /// Base row. Rows count upward, so a larger base is nearer the top.
    pub base: u32,
    /// How many rows the card occupies.
    pub rows: u32,
}

impl Card {
    /// Whether two cards cover any track in common.
    #[must_use]
    pub const fn tracks_overlap(self, other: Self) -> bool {
        self.track < other.track + other.span && other.track < self.track + self.span
    }

    /// Whether two cards cover any row in common.
    #[must_use]
    pub const fn rows_overlap(self, other: Self) -> bool {
        self.base < other.base + other.rows && other.base < self.base + self.rows
    }

    /// Whether two cards occupy any of the same cell.
    ///
    /// Overlapping on one axis alone is ordinary: cards sit side by side and
    /// they stack. Overlapping on both is the lattice being wrong.
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        self.tracks_overlap(other) && self.rows_overlap(other)
    }
}

/// A shape offered for placement, before a track has been chosen for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// How many tracks this shape would cover.
    pub span: u32,
    /// How many rows the content needs at that width.
    pub rows: u32,
}

/// The shapes one card is offered, arbitrated by [`decide`].
///
/// The predecessor passed an array and threw when it was empty, which put a
/// failure in the engine that its own callers could never actually produce:
/// candidate generation always measures the base width and only sometimes
/// measures a wider tier. That shape is stated here instead, so the empty case
/// is unrepresentable and every function below is total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidates {
    /// The width every card is measured at.
    pub base: Candidate,
    /// The wider tier, offered only when the base measurement was tall enough
    /// and there is somewhere wider to go.
    pub wide: Option<Candidate>,
}

impl Candidates {
    /// A card offered only its base width.
    #[must_use]
    pub const fn base(span: u32, rows: u32) -> Self {
        Self {
            base: Candidate { span, rows },
            wide: None,
        }
    }

    /// This card's shapes, base first.
    fn iter(self) -> impl Iterator<Item = Candidate> {
        std::iter::once(self.base).chain(self.wide)
    }
}

/// Where a span would sit at one track, and the dead space it would leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// The row the span would rest on: the highest floor it covers.
    pub base: u32,
    /// Dead space left underneath, summed across the covered tracks.
    pub waste: u32,
    /// The anchor track.
    pub track: u32,
    /// The span, already clamped to the lattice.
    pub span: u32,
}

/// The lattice width to actually use.
///
/// A width of zero is not a lattice: it yields an empty floor, a placement
/// range of nothing, and an anchor search with no candidate track at all. The
/// predecessor also had to guard fractional and non-finite widths, because its
/// numbers were floats and a value like `0.5` passed a `> 0` test and then
/// floored to zero. A `u32` cannot be fractional, infinite, or absent, so zero
/// is the whole degenerate space here rather than one member of it.
#[must_use]
pub const fn track_count(tracks: u32) -> u32 {
    if tracks >= 1 {
        tracks
    } else {
        DEFAULT_TRACK_COUNT
    }
}

/// A span clamped into `[1, tracks]`.
#[must_use]
pub const fn clamp_span(span: u32, tracks: u32) -> u32 {
    let tracks = track_count(tracks);
    if span < 1 {
        1
    } else if span > tracks {
        tracks
    } else {
        span
    }
}

/// The skyline of a card list: per track, the highest row any card reaches.
///
/// Derived fresh from the cards rather than carried alongside them, which is
/// the shape a full replay needs in order to rebuild against a clean floor.
#[must_use]
pub fn derive_row_floor(cards: &[Card], tracks: u32) -> Vec<u32> {
    let tracks = track_count(tracks);
    let mut floor = vec![0; tracks as usize];
    for card in cards {
        raise(&mut floor, *card, tracks);
    }
    floor
}

/// Raise the tracks a card covers to at least its top row.
fn raise(floor: &mut [u32], card: Card, tracks: u32) {
    let top = card.base + card.rows;
    let end = card.track.saturating_add(card.span).min(tracks);
    for track in card.track..end {
        let cell = &mut floor[track as usize];
        *cell = (*cell).max(top);
    }
}

/// Advance the floor past a placed card, without disturbing the one given.
///
/// Returned rather than applied in place so callers can compose placements
/// without aliasing a floor that something else is still reading.
#[must_use]
pub fn commit(floor: &[u32], card: Card, tracks: u32) -> Vec<u32> {
    let tracks = track_count(tracks);
    let mut next = floor.to_vec();
    raise(&mut next, card, tracks);
    next
}

/// The frontier a span would rest on at one track.
#[must_use]
pub fn anchor_at(track: u32, span: u32, floor: &[u32]) -> Anchor {
    let covered = || (track..track + span).filter_map(|index| floor.get(index as usize).copied());
    let base = covered().max().unwrap_or(0);
    let waste = covered().map(|height| base - height).sum();
    Anchor {
        base,
        waste,
        track,
        span,
    }
}

/// The best anchor for a fixed span: flattest frontier, then least waste, then
/// leftmost.
///
/// Total by construction. The lattice is at least one track wide and the span
/// is clamped into it, so track zero is always a legal placement and seeds the
/// search. Callers read the anchor's fields directly rather than asking
/// whether there was one.
#[must_use]
pub fn anchor_for(span: u32, floor: &[u32], tracks: u32) -> Anchor {
    let tracks = track_count(tracks);
    let span = clamp_span(span, tracks);
    (0..=tracks - span)
        .map(|track| anchor_at(track, span, floor))
        .min_by_key(|anchor| (anchor.base, anchor.waste, anchor.track))
        .unwrap_or_else(|| anchor_at(0, span, floor))
}

/// Choose among a card's offered shapes.
///
/// The key is `(base, waste, -span, track)`: flattest frontier first, then
/// least waste, then widest, then leftmost. Width sorts last among the three
/// that can favour it, which is the property worth naming: a wide placement
/// wins only by landing strictly lower than the best base placement, or by
/// tying it with no more waste. It never digs a hole to take width.
#[must_use]
pub fn decide(candidates: Candidates, floor: &[u32], tracks: u32) -> Card {
    let tracks = track_count(tracks);
    candidates
        .iter()
        .map(|candidate| {
            let anchor = anchor_for(candidate.span, floor, tracks);
            Card {
                track: anchor.track,
                span: anchor.span,
                base: anchor.base,
                rows: candidate.rows,
            }
        })
        .min_by_key(|card| {
            let anchor = anchor_at(card.track, card.span, floor);
            // Negated so that a wider span sorts earlier, matching the
            // predecessor's `-span` term without leaving the integers.
            (anchor.base, anchor.waste, tracks - card.span, card.track)
        })
        // `Candidates` always carries a base, so the iterator is never empty.
        .unwrap_or_else(|| Card {
            track: 0,
            span: clamp_span(candidates.base.span, tracks),
            base: 0,
            rows: candidates.base.rows,
        })
}

/// One card placed: what landed, the lattice after it, and the floor after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    /// The card that landed.
    pub card: Card,
    /// Every card, with the new one appended.
    pub cards: Vec<Card>,
    /// The floor advanced past the new card.
    pub floor: Vec<u32>,
}

/// Decide, append, commit.
///
/// Never touches an existing card. Placement is append-only, which is what
/// lets a card that has already been read stay where the reader left it.
#[must_use]
pub fn insert(cards: &[Card], floor: &[u32], candidates: Candidates, tracks: u32) -> Insertion {
    let tracks = track_count(tracks);
    let card = decide(candidates, floor, tracks);
    let mut placed = cards.to_vec();
    placed.push(card);
    Insertion {
        card,
        cards: placed,
        floor: commit(floor, card, tracks),
    }
}
