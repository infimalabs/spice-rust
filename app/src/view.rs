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

/// Every derived pane, one registry row apiece.
///
/// A pane is named for the question it answers rather than for the crate that
/// frames it, because its axis is a claim about what the reader wanted to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pane {
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
            Self::Pressure | Self::Residue | Self::ConsentVolume => Axis::Magnitude,
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
