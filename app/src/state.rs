//! One in-browser state over received frames.
//!
//! The app appends nothing and derives nothing it was not told. This state
//! remembers the highest position it has applied, a bounded window of the
//! causal headers it has received, and the most recent answer wearing whatever
//! provenance it arrived with. It never merges two answers, repairs a gap, or
//! reports a horizon, overflow, cursor, fold, or causal link it did not
//! receive.

use std::collections::{BTreeMap, BTreeSet};

use wire::{
    Accepted, Body, CanonicalUuid, Derived, Envelope, Fact, FoldName, Origin, Outcome, Overflow,
    Packed, Response, Seq,
};

use crate::socket::{Admitted, SessionScope};

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
    /// Another capability-scoped session admitted the frame.
    ForeignScope,
}

impl Applied {
    /// True when this envelope moved the cursor.
    #[must_use]
    pub const fn advanced(self) -> bool {
        matches!(self, Self::Advanced)
    }
}

/// Causal headers one state retains, keeping the highest positions received.
///
/// Retention is bounded because the app is a long-lived tab. A browser held
/// open against one generation receives positions for as long as that
/// generation lasts, and a per-position header nothing ever releases is a leak
/// by construction rather than a tuning question. Every other field of
/// [`BrowserState`] is a single value or one packed answer, so this is the only
/// place growth had nowhere to stop, and wasm32 linear memory is the tightest
/// budget any spice surface runs under.
///
/// The window keeps the newest positions because a walk starts where a reader
/// just looked and follows causes backward, so the chains that render whole are
/// the recent ones. An evicted header reads exactly like a position the session
/// never received: the walk stops at it as [`ChainEnd::Dangling`]. That is the
/// point of evicting rather than compacting, since a summarized or stitched
/// window would report a link no producer minted.
///
/// The number is 1024 because it clears two marks at once. A chain a pane
/// renders whole — a command and the facts it provoked, a task and its phases —
/// runs to tens of links, so a window of 1024 leaves roughly a hundredfold of
/// headroom before a reader meets a dangle where the whole chain would
/// otherwise have rendered. And a retained header costs about a hundred bytes: a
/// [`Seq`], an [`Origin`] such as `task:1kKRRsDP` or `lane:<uuid>`, an
/// optional [`Seq`], plus the map node that keys it. The window is therefore near
/// 100 KiB — a fixed price a tab pays once against an otherwise unbounded cost.
///
/// It is a constant rather than configuration because the socket is this
/// crate's whole system boundary. A configured bound could only arrive from the
/// server, which would let the far side of the wire size the tab's memory, and
/// the bound exists to protect the tab rather than to express operator policy.
pub const CAUSAL_HEADERS: usize = 1024;

/// One received envelope header in a causal walk.
///
/// The browser retains only the fields the walk renders, and only for the
/// [`CAUSAL_HEADERS`] highest positions it has received. Every link comes
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
    /// A received envelope cited a position the browser does not hold, because
    /// it never arrived or because the retention window evicted its header.
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

/// How a list answer was produced.
///
/// The two arms are the two reads the socket makes, and they differ in exactly
/// one way that matters downstream: whether the answer can say which fold
/// generation produced it and how far that generation had applied the journal.
/// A head-direct read cannot, and this enum gives it nowhere to claim
/// otherwise, so "never invent that provenance" holds by shape rather than by a
/// rule a renderer has to remember.
///
/// The arm is chosen by the wire shape the reply carried rather than by a
/// caller's argument. `From` is implemented for [`Packed`] and [`Derived`] and
/// for nothing else, so a head-direct read has no expression that produces
/// [`Answer::Derived`].
///
/// A wire shape crosses the door and becomes an answer on the way in:
///
/// ```no_run
/// let id: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163c1".parse().unwrap();
/// let session = app::Session::open(id, id);
/// let admitted: app::Admitted<wire::Response<wire::Packed<String>>> =
///     session.admit_frame(b"").unwrap();
/// ```
///
/// An `Answer` cannot make that crossing as itself, because it is not a wire
/// shape and holds no `Deserialize`. Deriving one would let a hand-written
/// frame name the `Derived` arm directly and mint the provenance this enum
/// exists to keep a head-direct read from claiming:
///
/// ```compile_fail
/// let id: wire::CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163c1".parse().unwrap();
/// let session = app::Session::open(id, id);
/// let admitted: app::Admitted<wire::Response<app::Answer<String>>> =
///     session.admit_frame(b"").unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// A fold-backed answer, carrying the provenance the fold stamped on it.
    Derived(Derived<T>),
    /// A head-direct answer, read straight off the authoritative heads.
    HeadDirect(Packed<T>),
}

impl<T> From<Derived<T>> for Answer<T> {
    fn from(derived: Derived<T>) -> Self {
        Self::Derived(derived)
    }
}

impl<T> From<Packed<T>> for Answer<T> {
    fn from(packed: Packed<T>) -> Self {
        Self::HeadDirect(packed)
    }
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
    /// This is deliberately not the packed horizon. The horizon names the
    /// source completeness boundary; this says how far the fold itself had
    /// gotten. Collapsing the two would let a stale generation read as current.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        match self {
            Self::Derived(derived) => Some(derived.cursor),
            Self::HeadDirect(_) => None,
        }
    }
}

/// Browser state scoped to one admitted principal and resident generation.
///
/// Capability scope and generation travel together because a same-epoch peer
/// is not this principal, and a reconnect against a new epoch is a different
/// authority rather than a continuation of this one. They travel together by
/// construction: the scope already names the generation, so this state keeps
/// no second copy that could be left disagreeing with it.
///
/// An epoch without an admitted principal cannot create browser state:
///
/// ```compile_fail
/// let epoch = "018f82d4-6f59-7d06-92e3-bce9131163c3"
///     .parse::<wire::CanonicalUuid>()
///     .unwrap();
/// let _ = app::BrowserState::<String>::new(epoch);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState<T> {
    scope: SessionScope,
    cursor: Option<Seq>,
    answer: Option<Answer<T>>,
    causal: BTreeMap<Seq, CausalLink>,
}

impl<T> BrowserState<T> {
    /// Begin empty for the principal named by one admitted acceptance.
    #[must_use]
    pub fn new(accepted: &Admitted<Accepted>) -> Self {
        Self {
            scope: accepted.scope(),
            cursor: None,
            answer: None,
            causal: BTreeMap::new(),
        }
    }

    pub(crate) const fn scope(&self) -> SessionScope {
        self.scope
    }

    /// The generation this state belongs to.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.scope.epoch()
    }

    /// The highest position applied, or `None` before the first frame.
    ///
    /// Absent is reported as absent. A state that has seen nothing does not
    /// claim position zero.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.cursor
    }

    /// Apply one same-scope admitted envelope, keeping the cursor monotone.
    ///
    /// Only a position above the cursor advances it. A duplicate and a
    /// reordered arrival both leave the cursor and the packed answer alone.
    /// Their admitted header remains available to complete a causal walk only
    /// when it falls among the [`CAUSAL_HEADERS`] highest positions received;
    /// an arrival below a full window's floor is discarded.
    ///
    /// The parameter is [`Admitted`] rather than a bare envelope so that a
    /// frame this build cannot read has no path in. Version admission belongs
    /// at the door; requiring the door's own receipt here means state cannot
    /// be updated by bytes that never reached it.
    ///
    /// [`Body`] bounds the fact for the same reason one step earlier: the
    /// envelope carried a kind from the closed vocabulary, so state advances
    /// on facts a producer can actually mint and on nothing else.
    ///
    /// Retaining the header may evict the lowest one held, since the window is
    /// the [`CAUSAL_HEADERS`] highest positions received and nothing else. The
    /// eviction is silent to this call's answer: whether a header was dropped
    /// says nothing about what this envelope did to the cursor.
    pub fn apply<F: Body>(&mut self, admitted: &Admitted<Envelope<Fact<F>>>) -> Applied {
        if admitted.scope() != self.scope {
            return Applied::ForeignScope;
        }
        let envelope = admitted.get();
        self.observe_header(envelope.seq, envelope.origin.clone(), envelope.cause)
    }

    /// Walk received cause links from `start` toward a root.
    ///
    /// A root is recognized only when its received envelope omits `cause`.
    /// Missing positions and cycles are terminal observations rather than
    /// placeholders or repair attempts, so this method always terminates and
    /// never reports a link the session did not admit.
    ///
    /// A position [`CAUSAL_HEADERS`] retention has evicted is missing in
    /// exactly that sense. A chain reaching past the window stops there as
    /// [`ChainEnd::Dangling`], never as [`ChainEnd::Root`], because a root is
    /// read off a received envelope and an evicted one is no longer held.
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

    /// Replace the current answer with an admitted reply.
    ///
    /// An answer carries its own rank, overflow, and horizon, so a later one
    /// replaces the earlier one whole. Merging two answers would produce a list
    /// no producer ever ranked and a horizon no source ever declared.
    ///
    /// The response must carry the socket door's [`Admitted`] receipt. A bare
    /// value has no path into state, so rows, overflow, horizon, and fold
    /// provenance cannot be populated by bytes that bypassed
    /// [`crate::Session`]. A refusal, fault, or answer from another state
    /// generation changes nothing and remains available to the caller through
    /// `admitted`.
    ///
    /// The reply body is whichever wire shape the read returned, and the
    /// [`Answer`] arm follows from it rather than from an argument. A
    /// [`Derived`] reply keeps the fold, generation, and cursor it arrived
    /// with; a [`Packed`] reply has nowhere to put them and so claims none.
    /// Flattening both to rows here is what would have forced every pane above
    /// to either invent provenance or drop it silently.
    #[must_use]
    pub fn replace<B>(&mut self, admitted: &Admitted<Response<B>>) -> bool
    where
        B: Clone + Into<Answer<T>>,
    {
        let response = admitted.get();
        let Outcome::Reply(reply) = &response.outcome else {
            return false;
        };
        if admitted.scope() != self.scope {
            return false;
        }
        self.answer = Some(reply.body.clone().into());
        true
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

    /// Adopt an admitted principal, discarding state that belonged to another.
    ///
    /// Reconnecting to the same principal and generation resumes: the cursor,
    /// causal headers, and current answer remain valid observations of that
    /// authority. A different capability or generation starts empty, because
    /// neither observations nor positions transfer across that boundary, and
    /// the fold provenance an answer wore was stamped by the generation that
    /// produced it rather than by the one now answering.
    pub fn reconnect(&mut self, accepted: &Admitted<Accepted>) {
        let scope = accepted.scope();
        if self.scope != scope {
            self.scope = scope;
            self.cursor = None;
            self.answer = None;
            self.causal.clear();
        }
    }

    fn observe_header(&mut self, seq: Seq, origin: Origin, cause: Option<Seq>) -> Applied {
        let below_retention_floor = self.causal.len() == CAUSAL_HEADERS
            && self
                .causal
                .first_key_value()
                .is_some_and(|(lowest, _)| seq < *lowest);
        if !below_retention_floor {
            self.causal
                .entry(seq)
                .or_insert(CausalLink { seq, origin, cause });
            while self.causal.len() > CAUSAL_HEADERS {
                self.causal.pop_first();
            }
        }
        match self.cursor {
            Some(cursor) if seq == cursor => Applied::Duplicate,
            Some(cursor) if seq < cursor => Applied::Stale,
            _ => {
                self.cursor = Some(seq);
                Applied::Advanced
            }
        }
    }
}
