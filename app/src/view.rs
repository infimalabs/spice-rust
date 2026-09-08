//! The derived task and organization view axes and the panes standing on them.
//!
//! Flow and topology are the complete analytical view vocabulary. A new plane
//! becomes a pane on one of these two rather than a third view,
//! and a feature that cannot make that mapping is a contract gap rather than a
//! new axis. The set is closed for the same reason the command registry is: a
//! match over it is exhaustive, so a pane added without an axis does not
//! compile.
//!
//! This is the derivation leaf that audits the mapping. It carries the
//! vocabulary and the registry only; the per-axis derivations land beside their
//! sources rather than as empty modules waiting for them.
//!
//! Claims divide across the remaining views, moving through [`Axis::Flow`] and
//! possessed at [`Axis::Topology`].

/// The complete view vocabulary.
///
/// Each axis is a question, and every pane answers exactly one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Axis {
    /// What work is moving?
    Flow,
    /// What is connected and possessed?
    Topology,
}

impl Axis {
    /// Every axis, in declaration order.
    pub const ALL: &'static [Self] = &[Self::Flow, Self::Topology];

    /// The rendered spelling of this axis.
    ///
    /// The match is exhaustive, so a new axis must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Topology => "topology",
        }
    }

    /// The question this axis answers.
    #[must_use]
    pub const fn question(self) -> &'static str {
        match self {
            Self::Flow => "What work is moving?",
            Self::Topology => "What is connected and possessed?",
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

/// User-facing Canvas destinations in their shared navigation order.
///
/// Internal axes and lane vocabulary remain stable; this registry is the
/// presentation boundary for global Canvas controls. Org controls select
/// their closed, scoped subset through `LineageMode` in the browser shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasDestination {
    Org,
    Tasks,
    Agents,
}

/// Canvas destinations that are meaningful inside one organization surface.
///
/// This type is intentionally distinct from [`CanvasDestination`]: adding a
/// global destination cannot make it appear in an organization tablist without an
/// explicit scoped variant and mapping here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgDestination {
    Org,
    Tasks,
    Agents,
}

impl OrgDestination {
    /// Every organization-scoped destination, left to right.
    pub const ALL: [Self; 3] = [Self::Org, Self::Tasks, Self::Agents];

    /// Corresponding destination in the global Canvas vocabulary.
    #[must_use]
    pub const fn canvas(self) -> CanvasDestination {
        match self {
            Self::Org => CanvasDestination::Org,
            Self::Tasks => CanvasDestination::Tasks,
            Self::Agents => CanvasDestination::Agents,
        }
    }

    /// Presentation label shared with the corresponding global destination.
    #[must_use]
    pub const fn label(self) -> &'static str {
        self.canvas().label()
    }
}

impl CanvasDestination {
    /// Every global destination, left to right.
    pub const ALL: [Self; 3] = [Self::Org, Self::Tasks, Self::Agents];

    /// Presentation label independent of internal protocol spelling.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Org => "Team",
            Self::Tasks => "Tasks",
            Self::Agents => "Threads",
        }
    }

    /// Destination represented by one analytical axis.
    #[must_use]
    pub const fn for_axis(axis: Axis) -> Self {
        match axis {
            Axis::Topology => Self::Org,
            Axis::Flow => Self::Tasks,
        }
    }

    /// Analytical axis selected by this destination, absent for live agents.
    #[must_use]
    pub const fn axis(self) -> Option<Axis> {
        match self {
            Self::Org => Some(Axis::Topology),
            Self::Tasks => Some(Axis::Flow),
            Self::Agents => None,
        }
    }

    /// Accessible description shared by every navigation surface.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Org => "Resident organizations, agents, capabilities, and possession",
            Self::Tasks => "Tasks moving through their workflow",
            Self::Agents => "Converse across persistent organization agents",
        }
    }
}

/// Every derived pane, one registry row apiece.
///
/// A pane is named for the question it answers rather than for the crate that
/// frames it, because its axis is a claim about what the reader wanted to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pane {
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
}

impl Pane {
    /// Every pane in the registry, grouped by axis in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::Tasks,
        Self::Claims,
        Self::Deferrals,
        Self::Reviews,
        Self::Lanes,
        Self::Worktrees,
        Self::Lineage,
        Self::Occupancy,
        Self::Residents,
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
            Self::Tasks | Self::Claims | Self::Deferrals | Self::Reviews => Axis::Flow,
            Self::Lanes | Self::Worktrees | Self::Lineage | Self::Occupancy | Self::Residents => {
                Axis::Topology
            }
        }
    }

    /// The rendered spelling of this pane.
    ///
    /// This match is exhaustive, so a new pane must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tasks => "tasks",
            Self::Claims => "claims",
            Self::Deferrals => "deferrals",
            Self::Reviews => "reviews",
            Self::Lanes => "lanes",
            Self::Worktrees => "worktrees",
            Self::Lineage => "lineage",
            Self::Occupancy => "occupancy",
            Self::Residents => "residents",
        }
    }

    /// User-facing pane label independent of internal protocol spelling.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tasks => "Tasks",
            Self::Claims => "Claims",
            Self::Deferrals => "Deferrals",
            Self::Reviews => "Reviews",
            Self::Lanes => "Agents",
            Self::Worktrees => "Worktrees",
            Self::Lineage => "Lineage",
            Self::Occupancy => "Occupancy",
            Self::Residents => "Residents",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_destinations_own_the_complete_presentation_order() {
        assert_eq!(
            CanvasDestination::ALL.map(CanvasDestination::label),
            ["Team", "Tasks", "Threads"],
        );
        assert_eq!(
            OrgDestination::ALL.map(OrgDestination::canvas),
            [
                CanvasDestination::Org,
                CanvasDestination::Tasks,
                CanvasDestination::Agents,
            ],
        );
        assert_eq!(CanvasDestination::Agents.axis(), None);
        for axis in Axis::ALL {
            assert_eq!(CanvasDestination::for_axis(*axis).axis(), Some(*axis));
        }
        assert_eq!(Pane::Lanes.as_str(), "lanes");
        assert_eq!(Pane::Lanes.label(), "Agents");
    }
}
