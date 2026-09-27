//! The derived task and team view axes and the Canvas destinations over them.
//!
//! Flow and topology are the complete analytical view vocabulary. A new plane
//! lands on one of these two rather than becoming a third view, and a feature
//! that cannot make that mapping is a contract gap rather than a new axis. The
//! set is closed for the same reason the command registry is: a match over it
//! is exhaustive.

#[cfg(any(target_arch = "wasm32", test))]
use std::borrow::Cow;

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
        Self::ALL
            .into_iter()
            .find(|destination| destination.path() == path)
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

#[cfg(any(target_arch = "wasm32", test))]
const CARD_TARGET_PATH_PREFIX: &str = "/card/";

/// One admitted Canvas URL state.
///
/// Global destinations remain the complete navigation vocabulary. A card
/// target refines Chat rather than becoming a fourth destination, so opening a
/// permalink cannot select an impossible global view.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanvasRoute {
    destination: CanvasDestination,
    card_target: Option<String>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl CanvasRoute {
    #[must_use]
    pub(crate) const fn destination(destination: CanvasDestination) -> Self {
        Self {
            destination,
            card_target: None,
        }
    }

    pub(crate) fn card(target: String) -> Option<Self> {
        valid_card_target(&target).then_some(Self {
            destination: CanvasDestination::Chat,
            card_target: Some(target),
        })
    }

    #[must_use]
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        CanvasDestination::from_path(path)
            .map(Self::destination)
            .or_else(|| {
                let target = path.strip_prefix(CARD_TARGET_PATH_PREFIX)?;
                Self::card(target.to_owned())
            })
    }

    #[must_use]
    pub(crate) const fn canvas_destination(&self) -> CanvasDestination {
        self.destination
    }

    #[must_use]
    pub(crate) fn card_target(&self) -> Option<&str> {
        self.card_target.as_deref()
    }

    #[must_use]
    pub(crate) fn path(&self) -> Cow<'_, str> {
        self.card_target.as_ref().map_or_else(
            || Cow::Borrowed(self.destination.path()),
            |target| Cow::Owned(format!("{CARD_TARGET_PATH_PREFIX}{target}")),
        )
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn card_href(target: &str) -> String {
    let route = CanvasRoute::card(target.to_owned())
        .expect("a rendered Canvas card has a canonical route target");
    format!("#{}", route.path())
}

#[cfg(any(target_arch = "wasm32", test))]
fn valid_card_target(target: &str) -> bool {
    !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{CanvasDestination, CanvasRoute};

    #[test]
    fn card_routes_refine_chat_and_round_trip_the_exact_target() {
        let path = "/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1";
        let route = CanvasRoute::from_path(path).expect("canonical card route parses");

        assert_eq!(route.canvas_destination(), CanvasDestination::Chat);
        assert_eq!(
            route.card_target(),
            Some("message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1")
        );
        assert_eq!(route.path(), path);
    }

    #[test]
    fn card_routes_refuse_missing_or_noncanonical_targets() {
        for path in [
            "/card/",
            "/card/message/agent",
            "/card/message_agent",
            "/card/message-agent?copy=true",
        ] {
            assert!(
                CanvasRoute::from_path(path).is_none(),
                "{path:?} unexpectedly named a card"
            );
        }
    }
}
