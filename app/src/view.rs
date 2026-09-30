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
    Dash,
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
    Dash,
    Work,
    Chat,
}

impl TeamDestination {
    /// Every team-scoped destination, left to right.
    pub const ALL: [Self; 3] = [Self::Dash, Self::Work, Self::Chat];

    /// Corresponding destination in the global Canvas vocabulary.
    #[must_use]
    pub const fn canvas(self) -> CanvasDestination {
        match self {
            Self::Dash => CanvasDestination::Dash,
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
    pub const ALL: [Self; 3] = [Self::Dash, Self::Work, Self::Chat];
    /// Exact hash-history path for this destination.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Dash => "/dash",
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
            Self::Dash => "Dash",
            Self::Work => "Work",
            Self::Chat => "Chat",
        }
    }

    /// Destination represented by one analytical axis.
    #[must_use]
    pub const fn for_axis(axis: Axis) -> Self {
        match axis {
            Axis::Topology => Self::Dash,
            Axis::Flow => Self::Work,
        }
    }

    /// Analytical axis selected by this destination, absent for live agents.
    #[must_use]
    pub const fn axis(self) -> Option<Axis> {
        match self {
            Self::Dash => Some(Axis::Topology),
            Self::Work => Some(Axis::Flow),
            Self::Chat => None,
        }
    }

    /// Accessible description shared by every navigation surface.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Dash => "Dash for consolidated state, controls, and details",
            Self::Work => "Work across tasks, claims, deferrals, and reviews",
            Self::Chat => "Chat across persistent team agents",
        }
    }
}

/// Connection phase relevant to persistent Canvas chrome.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionStage {
    Starting,
    Connecting,
    Online,
    Offline,
}

/// Notice shown beside the global Spice control while the Canvas cannot yet
/// present a current admitted lane snapshot.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanvasNotice {
    LoadingCanvas,
    OpeningResident,
    Reconnecting,
    LoadingAgents,
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
impl CanvasNotice {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::LoadingCanvas => "Loading Canvas",
            Self::OpeningResident => "Opening resident",
            Self::Reconnecting => "Reconnecting",
            Self::LoadingAgents => "Loading agents",
        }
    }
}

/// Select the one global notice that owns pre-lane and reconnect state.
///
/// Connectivity wins over hydration because a retained runtime may remain on
/// screen through an outage. Online Canvas yields the slot only after both its
/// runtime and its lane snapshot have been admitted.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) const fn canvas_notice(
    connection: ConnectionStage,
    runtime_admitted: bool,
    lanes_admitted: bool,
) -> Option<CanvasNotice> {
    match connection {
        ConnectionStage::Starting => Some(CanvasNotice::LoadingCanvas),
        ConnectionStage::Connecting => Some(CanvasNotice::OpeningResident),
        ConnectionStage::Offline => Some(CanvasNotice::Reconnecting),
        ConnectionStage::Online if !runtime_admitted => Some(CanvasNotice::LoadingCanvas),
        ConnectionStage::Online if !lanes_admitted => Some(CanvasNotice::LoadingAgents),
        ConnectionStage::Online => None,
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const CARD_TARGET_PATH_PREFIX: &str = "/card/";

#[cfg(any(target_arch = "wasm32", test))]
const CREATOR_CONTEXT_TARGET_PREFIX: &str = "creator-context-";

/// A task-creation boundary encoded as one canonical Chat route target.
///
/// Source identity is part of both the route and resolution. A lane can host
/// successive transcripts whose byte loci overlap; lane and locus alone are
/// never creator authority.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreatorContextTarget {
    pub(crate) lane: wire::CanonicalUuid,
    source: wire::CanonicalUuid,
    pub(crate) before: wire::body::TranscriptPosition,
}

#[cfg(any(target_arch = "wasm32", test))]
impl CreatorContextTarget {
    #[must_use]
    pub(crate) fn from_creation(context: &wire::body::TaskCreationContext) -> Option<Self> {
        let wire::body::TaskCreationContext::Transcript { boundary, .. } = context else {
            return None;
        };
        (boundary.stream == wire::body::TASK_CREATION_CONTEXT_STREAM).then_some(Self {
            lane: boundary.lane,
            source: boundary.source,
            before: boundary.next,
        })
    }

    #[must_use]
    pub(crate) fn parse(target: &str) -> Option<Self> {
        let encoded = target.strip_prefix(CREATOR_CONTEXT_TARGET_PREFIX)?;
        let lane = encoded.get(..36)?;
        let encoded = encoded.get(36..)?.strip_prefix('-')?;
        let source = encoded.get(..36)?;
        let position = encoded.get(36..)?;
        let (member, offset) = position
            .strip_prefix('-')?
            .split_once('-')
            .and_then(|(member, offset)| Some((member.parse().ok()?, offset.parse().ok()?)))?;
        Some(Self {
            lane: lane.parse().ok()?,
            source: source.parse().ok()?,
            before: wire::body::TranscriptPosition { member, offset },
        })
    }

    #[must_use]
    pub(crate) fn target(&self) -> String {
        format!(
            "{CREATOR_CONTEXT_TARGET_PREFIX}{}-{}-{}-{}",
            self.lane, self.source, self.before.member, self.before.offset,
        )
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    fn owns_source(&self, message: &wire::AgentMessage) -> bool {
        self.owns_source_parts(
            message.lane,
            message
                .quote_target
                .as_ref()
                .and_then(wire::QuoteTarget::tape_source),
        )
    }

    #[must_use]
    fn owns_source_parts(
        &self,
        lane: wire::CanonicalUuid,
        source: Option<wire::CanonicalUuid>,
    ) -> bool {
        lane == self.lane && source == Some(self.source)
    }

    /// Whether one visible message from the exact source lies before this
    /// task's admission boundary.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) fn contains(&self, message: &wire::AgentMessage) -> bool {
        self.owns_source(message) && self.contains_locus(message.lane, message.locus)
    }

    #[must_use]
    fn contains_locus(&self, lane: wire::CanonicalUuid, locus: wire::MessageLocus) -> bool {
        lane == self.lane && (locus.member, locus.offset) < (self.before.member, self.before.offset)
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) fn creator_context_href<'a>(
    context: &wire::body::TaskCreationContext,
    messages: impl IntoIterator<Item = &'a wire::AgentMessage>,
) -> Option<String> {
    let target = CreatorContextTarget::from_creation(context)?;
    messages
        .into_iter()
        .any(|message| target.owns_source(message))
        .then(|| card_href(&target.target()))
}

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

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
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
    use super::{
        CanvasDestination, CanvasNotice, CanvasRoute, ConnectionStage, CreatorContextTarget,
        canvas_notice,
    };

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

    #[test]
    fn creator_context_targets_round_trip_the_exact_source_boundary() {
        let lane = "01234567-89ab-cdef-0123-456789abcdef"
            .parse()
            .expect("fixture lane");
        let source = "fedcba98-7654-3210-fedc-ba9876543210"
            .parse()
            .expect("fixture source");
        let context = wire::body::TaskCreationContext::Transcript {
            session: "session".to_owned(),
            boundary: wire::TranscriptSourceHead {
                lane,
                vendor: "omp".parse().expect("fixture vendor"),
                stream: wire::body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
                source,
                next: wire::body::TranscriptPosition {
                    member: 17,
                    offset: 4096,
                },
            },
            creator_team: None,
        };
        let expected = concat!(
            "creator-context-01234567-89ab-cdef-0123-456789abcdef-",
            "fedcba98-7654-3210-fedc-ba9876543210-17-4096",
        );
        let target =
            CreatorContextTarget::from_creation(&context).expect("transcript context links");

        assert_eq!(target.target(), expected);
        assert_eq!(CreatorContextTarget::parse(expected), Some(target.clone()));
        assert!(CanvasRoute::card(expected.to_owned()).is_some());
        let locus = |member, offset| wire::MessageLocus {
            member,
            offset,
            length: 1,
            event: 0,
        };
        assert!(target.owns_source_parts(lane, Some(source)));
        assert!(!target.owns_source_parts(lane, None));
        assert!(
            !target.owns_source_parts(
                lane,
                Some(
                    "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                        .parse()
                        .expect("other fixture source"),
                ),
            )
        );
        assert!(target.contains_locus(lane, locus(17, 4095)));
        assert!(target.contains_locus(lane, locus(16, u64::MAX)));
        assert!(!target.contains_locus(lane, locus(17, 4096)));
        assert!(!target.contains_locus(lane, locus(18, 0)));
        assert!(
            CreatorContextTarget::from_creation(&wire::body::TaskCreationContext::unavailable(
                wire::body::TaskCreationUnavailable::OperatorActor,
            ),)
            .is_none()
        );
    }

    #[test]
    fn global_notice_yields_only_to_an_online_admitted_lane_snapshot() {
        assert_eq!(
            canvas_notice(ConnectionStage::Starting, true, true),
            Some(CanvasNotice::LoadingCanvas)
        );
        assert_eq!(
            canvas_notice(ConnectionStage::Connecting, true, true),
            Some(CanvasNotice::OpeningResident)
        );
        assert_eq!(
            canvas_notice(ConnectionStage::Offline, true, true),
            Some(CanvasNotice::Reconnecting)
        );
        assert_eq!(
            canvas_notice(ConnectionStage::Online, false, false),
            Some(CanvasNotice::LoadingCanvas)
        );
        assert_eq!(
            canvas_notice(ConnectionStage::Online, true, false),
            Some(CanvasNotice::LoadingAgents)
        );
        assert_eq!(canvas_notice(ConnectionStage::Online, true, true), None);
    }
}
