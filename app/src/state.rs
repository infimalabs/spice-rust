//! One in-browser state over received frames.
//!
//! The app appends nothing and derives nothing it was not told. This state
//! remembers the highest position it has applied and the most recent packed
//! answer; it never merges two answers, never repairs a gap, and never
//! reports a horizon, overflow, or cursor it did not receive.

use wire::{Body, CanonicalUuid, Envelope, Fact, Overflow, Packed, Seq};

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

/// Browser state scoped to one resident generation.
///
/// The generation is part of the state because a reconnect against a new epoch
/// is a different authority, not a continuation of this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState<T> {
    epoch: CanonicalUuid,
    cursor: Option<Seq>,
    packed: Option<Packed<T>>,
}

impl<T> BrowserState<T> {
    /// Begin empty against one generation.
    #[must_use]
    pub const fn new(epoch: CanonicalUuid) -> Self {
        Self {
            epoch,
            cursor: None,
            packed: None,
        }
    }

    /// The generation this state belongs to.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// The highest position applied, or `None` before the first frame.
    ///
    /// Absent is reported as absent. A state that has seen nothing does not
    /// claim position zero.
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

    /// Replace the packed answer with a newly received one.
    ///
    /// A packed answer carries its own rank, overflow, and horizon, so a later
    /// answer replaces the earlier one whole. Merging two answers would
    /// produce a list no producer ever ranked and a horizon no source ever
    /// declared.
    pub fn replace(&mut self, packed: Packed<T>) {
        self.packed = Some(packed);
    }

    /// The rows of the current answer, empty before one arrives.
    #[must_use]
    pub fn rows(&self) -> &[T] {
        self.packed.as_ref().map_or(&[], |packed| &packed.rows)
    }

    /// What the current answer left out, when it left anything out.
    #[must_use]
    pub fn overflow(&self) -> Option<Overflow> {
        self.packed.as_ref().and_then(|packed| packed.overflow)
    }

    /// The completeness boundary the current answer declared.
    #[must_use]
    pub fn horizon(&self) -> Option<Seq> {
        self.packed.as_ref().map(|packed| packed.horizon)
    }

    /// True when the current answer omitted ranked rows.
    #[must_use]
    pub fn is_overflowing(&self) -> bool {
        self.overflow().is_some()
    }

    /// Adopt a generation, discarding state that belonged to another.
    ///
    /// Reconnecting to the same generation resumes: the cursor and the packed
    /// answer remain valid observations of that authority. Reconnecting to a
    /// new one starts empty, because positions and horizons are only meaningful
    /// within the generation that assigned them.
    pub fn reconnect(&mut self, epoch: CanonicalUuid) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.cursor = None;
            self.packed = None;
        }
    }
}
