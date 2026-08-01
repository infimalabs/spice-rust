//! The four view axes and the panes standing on them.
//!
//! The view vocabulary is complete and closed: magnitude, flow, topology,
//! chronology. A new plane becomes a pane on one of those axes rather than a
//! fifth axis beside them, which is what keeps the canvas a fixed set of ways
//! to look at authority rather than a drawer that grows a compartment every
//! time the server learns a noun.
//!
//! Every pane reads a list, and every list crosses as [`wire::Packed`], so
//! overflow and horizon arrive stated rather than inferred. A pane never
//! compares a row count against a page size to decide it was truncated: the
//! answer says what it omitted and where to resume, or it omitted nothing.
//!
//! Provenance is structural, and it is [`Answer`] that makes it so. A pane
//! reads its fold, generation, and cursor straight off the answer it was
//! given, so a head-direct read shows none of the three for want of a field to
//! hold them rather than for want of a rule someone remembered.

use wire::{FoldName, Overflow, Seq};

use crate::state::Answer;

/// The complete view vocabulary.
///
/// Every question the canvas answers is one of these four: how much there is,
/// how it is moving, how it is connected, and when it happened. The set is
/// closed for the same reason the command registry is — a match over it is
/// exhaustive, so a pane added without an axis does not compile — and naming a
/// fifth is a doctrine change rather than a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Axis {
    /// How much there is: totals, weights, and the shape of a distribution.
    Magnitude,
    /// How work moves: what advanced, what is held, what is waiting.
    Flow,
    /// How things relate: edges, containment, and occupancy.
    Topology,
    /// What happened in order: positions and instants along the journal.
    Chronology,
}

impl Axis {
    /// Every axis, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Magnitude,
        Self::Flow,
        Self::Topology,
        Self::Chronology,
    ];

    /// The rendered spelling of this axis.
    ///
    /// The match is exhaustive, so a new axis must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Magnitude => "magnitude",
            Self::Flow => "flow",
            Self::Topology => "topology",
            Self::Chronology => "chronology",
        }
    }
}

/// Every pane the canvas renders, one registry row apiece.
///
/// A pane is named for what it shows rather than for the crate that answers
/// it, because the axis it stands on is a claim about the reader's question,
/// not about the server's module layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pane {
    /// Outstanding work by project and phase.
    Backlog,
    /// Executable surfaces by trust state.
    Trust,
    /// Phase advances across the workflows in play.
    Phases,
    /// Possession as it is won, carried, renewed, and released.
    Claims,
    /// Dependency edges between tasks.
    Depends,
    /// Lanes and the residents occupying them.
    Lanes,
    /// Facts in journal position order.
    Journal,
    /// Directives in the order they landed.
    Steer,
}

impl Pane {
    /// Every pane in the registry, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Backlog,
        Self::Trust,
        Self::Phases,
        Self::Claims,
        Self::Depends,
        Self::Lanes,
        Self::Journal,
        Self::Steer,
    ];

    /// The axis this pane stands on.
    ///
    /// This is the registry. The match is exhaustive, so a pane added without
    /// an axis fails to compile — the axis cannot be defaulted or deferred,
    /// only stated. Exactly one axis is returned because a pane that answered
    /// two questions would be two panes.
    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Self::Backlog | Self::Trust => Axis::Magnitude,
            Self::Phases | Self::Claims => Axis::Flow,
            Self::Depends | Self::Lanes => Axis::Topology,
            Self::Journal | Self::Steer => Axis::Chronology,
        }
    }

    /// The rendered spelling of this pane.
    ///
    /// This match is exhaustive, so a new pane must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Trust => "trust",
            Self::Phases => "phases",
            Self::Claims => "claims",
            Self::Depends => "depends",
            Self::Lanes => "lanes",
            Self::Journal => "journal",
            Self::Steer => "steer",
        }
    }
}

/// One pane holding one answer.
///
/// Everything a pane renders is read back through here, so the accessors are
/// the whole rendering contract: rows, the overflow marker, the horizon, and
/// the three provenance fields that are present exactly when the answer was
/// fold-backed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneState<T> {
    pane: Pane,
    answer: Answer<T>,
}

impl<T> PaneState<T> {
    /// Seat one answer in one pane.
    #[must_use]
    pub const fn new(pane: Pane, answer: Answer<T>) -> Self {
        Self { pane, answer }
    }

    /// Which pane this is.
    #[must_use]
    pub const fn pane(&self) -> Pane {
        self.pane
    }

    /// The axis this pane stands on.
    #[must_use]
    pub const fn axis(&self) -> Axis {
        self.pane.axis()
    }

    /// The answer backing this pane.
    #[must_use]
    pub const fn answer(&self) -> &Answer<T> {
        &self.answer
    }

    /// The rows this pane renders, in the order they were ranked.
    #[must_use]
    pub fn rows(&self) -> &[T] {
        &self.answer.packed().rows
    }

    /// What the answer left out, when it left anything out.
    ///
    /// Read from the answer rather than computed. A pane that compared
    /// `rows().len()` against a page size would be guessing at truncation, and
    /// would guess wrong for every list shorter than its page.
    #[must_use]
    pub fn overflow(&self) -> Option<Overflow> {
        self.answer.packed().overflow
    }

    /// The position through which the answer's inputs are complete.
    #[must_use]
    pub fn horizon(&self) -> Seq {
        self.answer.packed().horizon
    }

    /// True when ranked rows were omitted from this answer.
    #[must_use]
    pub fn is_overflowing(&self) -> bool {
        self.answer.packed().is_overflowing()
    }

    /// The fold that produced this answer, or `None` for a head-direct read.
    #[must_use]
    pub const fn fold(&self) -> Option<&FoldName> {
        self.answer.fold()
    }

    /// The generation of that fold, or `None` for a head-direct read.
    #[must_use]
    pub const fn generation(&self) -> Option<u64> {
        self.answer.generation()
    }

    /// How far that generation had applied the journal, or `None` for a
    /// head-direct read.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.answer.cursor()
    }
}
