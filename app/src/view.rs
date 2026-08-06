//! The four derived view axes and the panes standing on them.
//!
//! Magnitude, flow, topology, and chronology are the complete view vocabulary.
//! A new plane becomes a pane on one of these four rather than a fifth view,
//! and a feature that cannot make that mapping is a contract gap rather than a
//! new axis. The set is closed for the same reason the command registry is: a
//! match over it is exhaustive, so a pane added without an axis does not
//! compile.
//!
//! This is the derivation leaf that audits the mapping. It carries the
//! vocabulary and the registry only; the per-axis derivations land beside their
//! sources rather than as empty modules waiting for them.
//!
//! One plane may stand on two axes, and when it does it is two panes rather
//! than one pane with two homes. Consent is the worked example: its remaining
//! budgets and disposition rates are amounts, while its asked, queued, granted,
//! denied, and floored dispositions are events in time. Claims divide the same
//! way, moving through [`Axis::Flow`] and possessed at [`Axis::Topology`].
//! Splitting them is what lets every pane answer exactly one question.

/// The complete view vocabulary.
///
/// Each axis is a question, and every pane answers exactly one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Axis {
    /// How much exists now?
    Magnitude,
    /// What work is moving?
    Flow,
    /// What is connected and possessed?
    Topology,
    /// What happened, in what order, and what waits?
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

    /// The question this axis answers.
    #[must_use]
    pub const fn question(self) -> &'static str {
        match self {
            Self::Magnitude => "How much exists now?",
            Self::Flow => "What work is moving?",
            Self::Topology => "What is connected and possessed?",
            Self::Chronology => "What happened, in what order, and what waits?",
        }
    }

    /// The panes standing on this axis, in registry order.
    ///
    /// A view renders one axis at a time and needs the panes under it, which
    /// is the direction [`Pane::axis`] does not run. This derives that
    /// direction from the registry instead of declaring it, so there is no
    /// second table that could come to disagree with the first: a pane moved
    /// to another axis leaves this list and joins that one in the same edit.
    pub fn panes(self) -> impl Iterator<Item = Pane> {
        Pane::ALL
            .iter()
            .copied()
            .filter(move |pane| pane.axis() == self)
    }
}

/// What the operator has the rail pointed at.
///
/// The axes answer questions *about* the appliance, and every one of them is a
/// reading: panes derived from what the resident already holds. The mosaic is
/// not a reading, it is the surface an operator works lanes from, so it stands
/// beside the axis vocabulary rather than joining it as a fifth entry.
///
/// This is the distinction that keeps [`Axis`] closed. A new pane still has to
/// map onto one of the four, and a feature that cannot make that mapping is
/// still a contract gap; what changed is that the rail can now also select
/// something that was never a pane to begin with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Surface {
    /// One derived view axis and the panes standing on it.
    View(Axis),
    /// The lane mosaic, where work is directed rather than read.
    Mosaic,
}

impl Surface {
    /// Every surface the rail offers, in rail order.
    ///
    /// The axes come first and keep their declaration order, so adding an axis
    /// extends this list in place and the mosaic stays last.
    pub fn all() -> impl Iterator<Item = Self> {
        Axis::ALL
            .iter()
            .copied()
            .map(Self::View)
            .chain(std::iter::once(Self::Mosaic))
    }

    /// The rendered spelling of this surface.
    ///
    /// Axis surfaces spell themselves exactly as their axis does, so the rail
    /// and the stylesheet keep naming them by the one name they already had.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::View(axis) => axis.as_str(),
            Self::Mosaic => "mosaic",
        }
    }

    /// The question this surface answers.
    #[must_use]
    pub const fn question(self) -> &'static str {
        match self {
            Self::View(axis) => axis.question(),
            Self::Mosaic => "Which lanes are working, and on what?",
        }
    }

    /// The panes standing on this surface, in registry order.
    ///
    /// The mosaic carries none, and that is the honest answer rather than a
    /// gap: a pane is a derivation placed on an axis, and the mosaic is not an
    /// axis. Callers render an empty pane strip for it instead of being handed
    /// panes that answer a question the operator did not ask here.
    pub fn panes(self) -> impl Iterator<Item = Pane> {
        let axis = match self {
            Self::View(axis) => Some(axis),
            Self::Mosaic => None,
        };
        axis.into_iter().flat_map(Axis::panes)
    }
}

/// Every derived pane, one registry row apiece.
///
/// A pane is named for the question it answers rather than for the crate that
/// frames it, because its axis is a claim about what the reader wanted to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pane {
    /// Effort spent, at the packed horizon.
    Effort,
    /// Pressure standing against the lanes.
    Pressure,
    /// Residue left behind by work already counted.
    Residue,
    /// Remaining consent budgets, pending volume, and disposition rates.
    ConsentVolume,
    /// Tasks crossing phases, with their current authoritative heads.
    Tasks,
    /// Claims as they are won, carried, renewed, and released.
    Claims,
    /// Deferrals placed and waiting to wake.
    Deferrals,
    /// Reviews in flight and the verdicts closing them.
    Reviews,
    /// Lanes and their current standing.
    Lanes,
    /// Worktrees and the lanes they belong to.
    Worktrees,
    /// Lineage among tasks and the work descended from them.
    Lineage,
    /// Occupants and the claims they currently hold.
    Occupancy,
    /// Resident state as the appliance reports it.
    Residents,
    /// Compaction as it runs and settles.
    Compaction,
    /// Queues and the human delay ahead of them.
    Queues,
    /// Directives in the order they landed.
    Directives,
    /// Consent asked, queued, granted, denied, and floored, with rule and actor.
    ConsentDisposition,
    /// Trust digests pended, granted, and revoked, operator-attributed.
    Trust,
    /// Replies, refusal telemetry, faults, receipts, and remedies in order.
    Commands,
    /// Quiescing, draining, migration, backup, restore, and verification.
    Recovery,
}

impl Pane {
    /// Every pane in the registry, grouped by axis in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Effort,
        Self::Pressure,
        Self::Residue,
        Self::ConsentVolume,
        Self::Tasks,
        Self::Claims,
        Self::Deferrals,
        Self::Reviews,
        Self::Lanes,
        Self::Worktrees,
        Self::Lineage,
        Self::Occupancy,
        Self::Residents,
        Self::Compaction,
        Self::Queues,
        Self::Directives,
        Self::ConsentDisposition,
        Self::Trust,
        Self::Commands,
        Self::Recovery,
    ];

    /// The axis this pane stands on.
    ///
    /// This is the registry. The match is exhaustive, so a pane added without
    /// an axis fails to compile: the axis cannot be defaulted or deferred, only
    /// stated. Exactly one axis is returned because a pane answering two
    /// questions would be two panes.
    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Self::Effort | Self::Pressure | Self::Residue | Self::ConsentVolume => Axis::Magnitude,
            Self::Tasks | Self::Claims | Self::Deferrals | Self::Reviews => Axis::Flow,
            Self::Lanes | Self::Worktrees | Self::Lineage | Self::Occupancy | Self::Residents => {
                Axis::Topology
            }
            Self::Compaction
            | Self::Queues
            | Self::Directives
            | Self::ConsentDisposition
            | Self::Trust
            | Self::Commands
            | Self::Recovery => Axis::Chronology,
        }
    }

    /// The rendered spelling of this pane.
    ///
    /// This match is exhaustive, so a new pane must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Effort => "effort",
            Self::Pressure => "pressure",
            Self::Residue => "residue",
            Self::ConsentVolume => "consent-volume",
            Self::Tasks => "tasks",
            Self::Claims => "claims",
            Self::Deferrals => "deferrals",
            Self::Reviews => "reviews",
            Self::Lanes => "lanes",
            Self::Worktrees => "worktrees",
            Self::Lineage => "lineage",
            Self::Occupancy => "occupancy",
            Self::Residents => "residents",
            Self::Compaction => "compaction",
            Self::Queues => "queues",
            Self::Directives => "directives",
            Self::ConsentDisposition => "consent-disposition",
            Self::Trust => "trust",
            Self::Commands => "commands",
            Self::Recovery => "recovery",
        }
    }
}
