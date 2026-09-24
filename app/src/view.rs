//! The derived task and team view axes and the Canvas destinations over them.
//!
//! Flow and topology are the complete analytical view vocabulary. A new plane
//! lands on one of these two rather than becoming a third view, and a feature
//! that cannot make that mapping is a contract gap rather than a new axis. The
//! set is closed for the same reason the command registry is: a match over it
//! is exhaustive.

/// The complete view vocabulary.
///
/// Each axis is a question one Canvas destination answers.
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
}

/// User-facing Canvas destinations in their shared navigation order.
///
/// Internal axes and lane vocabulary remain stable; this registry is the
/// presentation boundary for global Canvas controls. Team controls select
/// their closed, scoped subset through `TeamMode` in the browser shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasDestination {
    Info,
    Work,
    Chat,
}

/// Canvas destinations that are meaningful inside one team surface.
///
/// This type is intentionally distinct from [`CanvasDestination`]: adding a
/// global destination cannot make it appear in a team tablist without an
/// explicit scoped variant and mapping here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamDestination {
    Info,
    Work,
    Chat,
}

impl TeamDestination {
    /// Every team-scoped destination, left to right.
    pub const ALL: [Self; 3] = [Self::Info, Self::Work, Self::Chat];

    /// Corresponding destination in the global Canvas vocabulary.
    #[must_use]
    pub const fn canvas(self) -> CanvasDestination {
        match self {
            Self::Info => CanvasDestination::Info,
            Self::Work => CanvasDestination::Work,
            Self::Chat => CanvasDestination::Chat,
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
    pub const ALL: [Self; 3] = [Self::Info, Self::Work, Self::Chat];
    /// Exact hash-history path for this destination.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Info => "/info",
            Self::Work => "/work",
            Self::Chat => "/",
        }
    }

    /// Destination named by one exact hash-history path.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/info" => Some(Self::Info),
            "/work" => Some(Self::Work),
            "/" => Some(Self::Chat),
            _ => None,
        }
    }

    /// Presentation label independent of internal protocol spelling.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Work => "Work",
            Self::Chat => "Chat",
        }
    }

    /// Destination represented by one analytical axis.
    #[must_use]
    pub const fn for_axis(axis: Axis) -> Self {
        match axis {
            Axis::Topology => Self::Info,
            Axis::Flow => Self::Work,
        }
    }

    /// Analytical axis selected by this destination, absent for live agents.
    #[must_use]
    pub const fn axis(self) -> Option<Axis> {
        match self {
            Self::Info => Some(Axis::Topology),
            Self::Work => Some(Axis::Flow),
            Self::Chat => None,
        }
    }

    /// Accessible description shared by every navigation surface.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Info => "Info about resident teams, agents, capabilities, and possession",
            Self::Work => "Work across tasks, claims, deferrals, and reviews",
            Self::Chat => "Chat across persistent team agents",
        }
    }
}
