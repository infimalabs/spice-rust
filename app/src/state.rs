//! One in-browser state over received frames.
//!
//! The app appends nothing and derives nothing it was not told. This state
//! remembers the highest position it has applied and the most recent answer;
//! it never merges two answers, never repairs a gap, and never reports a
//! horizon, overflow, or cursor it did not receive.

use wire::{Body, CanonicalUuid, Derived, Envelope, Fact, FoldName, Overflow, Packed, Seq};

use crate::socket::Admitted;

/// What applying one envelope did to the state.
///
/// Frames arrive over a network. A retried delivery repeats a position and a
/// reordered delivery lowers one, so the state names both outcomes rather than
/// folding them into a silent no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The position advanced the cursor.
    Advanced,
    /// The position was already applied.
    Duplicate,
    /// The position precedes the cursor, so a later frame already answered it.
    Stale,
}

impl Applied {
    /// True when this envelope moved the cursor.
    #[must_use]
    pub const fn advanced(self) -> bool {
        matches!(self, Self::Advanced)
    }
}

/// How a list answer was produced.
///
/// The two arms are the two reads the socket makes, and they differ in exactly
/// one way that matters downstream: whether the answer can say which fold
/// generation produced it and how far that generation had applied the journal.
/// A head-direct read cannot, and this enum gives it nowhere to claim
/// otherwise, so "never fabricate provenance" holds by shape rather than by a
/// rule a renderer has to remember.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// A fold-backed answer, carrying the provenance the fold stamped on it.
    Derived(Derived<T>),
    /// A head-direct answer, read straight off the authoritative heads.
    HeadDirect(Packed<T>),
}

impl<T> Answer<T> {
    /// The packed rows, however this answer was produced.
    ///
    /// Both arms carry one, because the list frame is the same either way and
    /// only the provenance around it differs.
    #[must_use]
    pub const fn packed(&self) -> &Packed<T> {
        match self {
            Self::Derived(derived) => &derived.packed,
            Self::HeadDirect(packed) => packed,
        }
    }

    /// The fold that produced this answer, or `None` for a head-direct read.
    #[must_use]
    pub const fn fold(&self) -> Option<&FoldName> {
        match self {
            Self::Derived(derived) => Some(&derived.fold),
            Self::HeadDirect(_) => None,
        }
    }

    /// The generation of that fold, or `None` for a head-direct read.
    #[must_use]
    pub const fn generation(&self) -> Option<u64> {
        match self {
            Self::Derived(derived) => Some(derived.r#gen),
            Self::HeadDirect(_) => None,
        }
    }

    /// How far that generation had applied the journal, or `None` for a
    /// head-direct read.
    ///
    /// This is deliberately not the packed horizon. The horizon says how far
    /// the declared inputs are complete; this says how far the fold itself had
    /// gotten. Collapsing the two would let a stale generation read as current.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        match self {
            Self::Derived(derived) => Some(derived.cursor),
            Self::HeadDirect(_) => None,
        }
    }

    /// True when this answer carries fold provenance at all.
    #[must_use]
    pub const fn is_derived(&self) -> bool {
        matches!(self, Self::Derived(_))
    }
}

/// Browser state scoped to one resident generation.
///
/// The generation is part of the state because a reconnect against a new epoch
/// is a different authority, not a continuation of this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState<T> {
    epoch: CanonicalUuid,
    cursor: Option<Seq>,
    answer: Option<Answer<T>>,
}

impl<T> BrowserState<T> {
    /// Begin empty against one generation.
    #[must_use]
    pub const fn new(epoch: CanonicalUuid) -> Self {
        Self {
            epoch,
            cursor: None,
            answer: None,
        }
    }

    /// The generation this state belongs to.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// The highest position applied from the stream, or `None` before the
    /// first frame.
    ///
    /// Absent is reported as absent. A state that has seen nothing does not
    /// claim position zero.
    ///
    /// This is the browser's own progress through the stream, which is not the
    /// same claim as [`Answer::cursor`]: that one reports how far the fold
    /// behind an answer had applied. Two subjects walking one journal keep two
    /// cursors, so they are reached through two receivers and never merged.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.cursor
    }

    /// Apply one admitted envelope, keeping the cursor monotone.
    ///
    /// Only a position above the cursor advances it. A duplicate and a
    /// reordered arrival both leave the cursor and the packed answer alone.
    ///
    /// The parameter is [`Admitted`] rather than a bare envelope so that a
    /// frame this build cannot read has no path in. Version admission belongs
    /// at the door; requiring the door's own receipt here means state cannot
    /// be updated by bytes that never reached it.
    ///
    /// [`Body`] bounds the fact for the same reason one step earlier: the
    /// envelope carried a kind from the closed vocabulary, so state advances
    /// on facts a producer can actually mint and on nothing else.
    pub fn apply<F: Body>(&mut self, admitted: &Admitted<Envelope<Fact<F>>>) -> Applied {
        let envelope = admitted.get();
        match self.cursor {
            Some(cursor) if envelope.seq == cursor => Applied::Duplicate,
            Some(cursor) if envelope.seq < cursor => Applied::Stale,
            _ => {
                self.cursor = Some(envelope.seq);
                Applied::Advanced
            }
        }
    }

    /// Replace the current answer with a newly received one.
    ///
    /// An answer carries its own rank, overflow, and horizon, so a later one
    /// replaces the earlier whole. Merging two would produce a list no producer
    /// ever ranked and a horizon no source ever declared.
    ///
    /// The parameter is [`Answer`] rather than a bare [`Packed`] so that a
    /// fold-backed read arrives with its provenance attached instead of being
    /// flattened on the way in. A state that accepted only the rows would have
    /// discarded the fold and generation at the door and then had nothing
    /// honest to render.
    pub fn replace(&mut self, answer: Answer<T>) {
        self.answer = Some(answer);
    }

    /// The current answer, or `None` before one arrives.
    #[must_use]
    pub const fn answer(&self) -> Option<&Answer<T>> {
        self.answer.as_ref()
    }

    /// The rows of the current answer, empty before one arrives.
    #[must_use]
    pub fn rows(&self) -> &[T] {
        self.answer
            .as_ref()
            .map_or(&[], |answer| &answer.packed().rows)
    }

    /// What the current answer left out, when it left anything out.
    #[must_use]
    pub fn overflow(&self) -> Option<Overflow> {
        self.answer
            .as_ref()
            .and_then(|answer| answer.packed().overflow)
    }

    /// The completeness boundary the current answer declared.
    #[must_use]
    pub fn horizon(&self) -> Option<Seq> {
        self.answer.as_ref().map(|answer| answer.packed().horizon)
    }

    /// True when the current answer omitted ranked rows.
    #[must_use]
    pub fn is_overflowing(&self) -> bool {
        self.overflow().is_some()
    }

    /// Adopt a generation, discarding state that belonged to another.
    ///
    /// Reconnecting to the same generation resumes: the cursor and the current
    /// answer remain valid observations of that authority. Reconnecting to a
    /// new one starts empty, because positions and horizons are only meaningful
    /// within the generation that assigned them.
    pub fn reconnect(&mut self, epoch: CanonicalUuid) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.cursor = None;
            self.answer = None;
        }
    }
}
