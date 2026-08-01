//! One in-browser state over received frames.
//!
//! The app appends nothing and derives nothing it was not told. This state
//! remembers the highest position it has applied, the causal headers it has
//! received, and the most recent packed answer. It never merges two answers,
//! repairs a gap, or reports a horizon, overflow, cursor, or causal link it
//! did not receive.

use std::collections::{BTreeMap, BTreeSet};

use wire::{Body, CanonicalUuid, Envelope, Fact, Origin, Outcome, Overflow, Packed, Response, Seq};

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

/// One received envelope header in a causal walk.
///
/// The browser retains only the fields the walk renders. Every link comes
/// from an admitted envelope; no missing position is materialized as a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalLink {
    seq: Seq,
    origin: Origin,
    cause: Option<Seq>,
}

impl CausalLink {
    /// This link's durable position.
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.seq
    }

    /// Root attribution inherited by this link.
    #[must_use]
    pub const fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The immediately provoking position, absent only at a root.
    #[must_use]
    pub const fn cause(&self) -> Option<Seq> {
        self.cause
    }
}

/// Why a causal walk stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainEnd {
    /// The final received envelope omitted `cause` and is the chain root.
    Root,
    /// A received envelope cited a position the browser has not received.
    Dangling(Seq),
    /// Following causes reached a position already visited by this walk.
    Cycle(Seq),
}

/// Received links from a starting position back toward its root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainWalk {
    links: Vec<CausalLink>,
    end: ChainEnd,
}

impl ChainWalk {
    /// Links in walk order, beginning at the requested position.
    #[must_use]
    pub fn links(&self) -> &[CausalLink] {
        &self.links
    }

    /// The observed condition that stopped the walk.
    #[must_use]
    pub const fn end(&self) -> ChainEnd {
        self.end
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
    causal: BTreeMap<Seq, CausalLink>,
}

impl<T> BrowserState<T> {
    /// Begin empty against one generation.
    #[must_use]
    pub const fn new(epoch: CanonicalUuid) -> Self {
        Self {
            epoch,
            cursor: None,
            packed: None,
            causal: BTreeMap::new(),
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
    /// reordered arrival both leave the cursor and the packed answer alone;
    /// their admitted header remains available to complete a causal walk.
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
        self.causal
            .entry(envelope.seq)
            .or_insert_with(|| CausalLink {
                seq: envelope.seq,
                origin: envelope.origin.clone(),
                cause: envelope.cause,
            });
        match self.cursor {
            Some(cursor) if envelope.seq == cursor => Applied::Duplicate,
            Some(cursor) if envelope.seq < cursor => Applied::Stale,
            _ => {
                self.cursor = Some(envelope.seq);
                Applied::Advanced
            }
        }
    }

    /// Walk received cause links from `start` toward a root.
    ///
    /// A root is recognized only when its received envelope omits `cause`.
    /// Missing positions and cycles are terminal observations rather than
    /// placeholders or repair attempts, so this method always terminates and
    /// never reports a link the session did not admit.
    #[must_use]
    pub fn walk(&self, start: Seq) -> ChainWalk {
        let mut links = Vec::new();
        let mut visited = BTreeSet::new();
        let mut position = start;

        loop {
            if !visited.insert(position) {
                return ChainWalk {
                    links,
                    end: ChainEnd::Cycle(position),
                };
            }

            let Some(link) = self.causal.get(&position) else {
                return ChainWalk {
                    links,
                    end: ChainEnd::Dangling(position),
                };
            };
            links.push(link.clone());

            match link.cause {
                Some(cause) => position = cause,
                None => {
                    return ChainWalk {
                        links,
                        end: ChainEnd::Root,
                    };
                }
            }
        }
    }

    /// Replace the packed answer with an admitted reply.
    ///
    /// A packed answer carries its own rank, overflow, and horizon, so a later
    /// answer replaces the earlier one whole. Merging two answers would
    /// produce a list no producer ever ranked and a horizon no source ever
    /// declared.
    ///
    /// The response must carry the socket door's [`Admitted`] receipt. A bare
    /// [`Packed`] value has no path into state, so rows, overflow, and horizon
    /// cannot be populated by bytes that bypassed [`crate::Session`]. A
    /// refusal, fault, or answer from another state generation changes
    /// nothing and remains available to the caller through `admitted`.
    #[must_use]
    pub fn replace(&mut self, admitted: &Admitted<Response<Packed<T>>>) -> bool
    where
        T: Clone,
    {
        let response = admitted.get();
        let Outcome::Reply(reply) = &response.outcome else {
            return false;
        };
        if response.epoch != self.epoch {
            return false;
        }
        self.packed = Some(reply.body.clone());
        true
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
    /// Reconnecting to the same generation resumes: the cursor, causal headers,
    /// and packed answer remain valid observations of that authority.
    /// Reconnecting to a new one starts empty, because positions and horizons
    /// are only meaningful within the generation that assigned them.
    pub fn reconnect(&mut self, epoch: CanonicalUuid) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.cursor = None;
            self.packed = None;
            self.causal.clear();
        }
    }
}
