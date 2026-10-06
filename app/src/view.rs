//! The derived task and team view axes and the app destinations over them.
//!
//! Flow and topology are the complete analytical view vocabulary. A new plane
//! lands on one of these two rather than becoming a third view, and a feature
//! that cannot make that mapping is a contract gap rather than a new axis. The
//! set is closed for the same reason the command registry is: a match over it
//! is exhaustive.

/// The complete view vocabulary.
///
/// Each axis is a question one app destination answers.
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

/// User-facing app destinations in their shared navigation order.
///
/// Internal axes and lane vocabulary remain stable; this registry is the
/// presentation boundary for global app controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDestination {
    Dash,
    Chat,
}

impl AppDestination {
    /// Every durable global destination, left to right.
    pub const ALL: [Self; 2] = [Self::Dash, Self::Chat];

    /// Exact hash-history path for this destination.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Dash => "/dash",
            Self::Chat => "/chat",
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
            Self::Chat => "Chat",
        }
    }

    /// Analytical axis selected by this destination, absent for live agents.
    #[must_use]
    pub const fn axis(self) -> Option<Axis> {
        match self {
            Self::Dash => Some(Axis::Topology),
            Self::Chat => None,
        }
    }

    /// Accessible description shared by every navigation surface.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Dash => "Dash for consolidated state, controls, and details",
            Self::Chat => "Chat across persistent team agents",
        }
    }
}

/// Connection phase relevant to persistent app chrome.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionStage {
    Starting,
    Connecting,
    Online,
    Offline,
}

/// Notice shown beside the global Spice control while the app cannot yet
/// present a current admitted lane snapshot.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppNotice {
    LoadingApp,
    OpeningResident,
    Reconnecting,
    LoadingAgents,
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
impl AppNotice {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::LoadingApp => "Loading spice",
            Self::OpeningResident => "Opening resident",
            Self::Reconnecting => "Reconnecting",
            Self::LoadingAgents => "Loading agents",
        }
    }
}

/// Select the one global notice that owns pre-lane and reconnect state.
///
/// Connectivity wins over hydration because a retained runtime may remain on
/// screen through an outage. The online app yields the slot only after both its
/// runtime and its lane snapshot have been admitted.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) const fn app_notice(
    connection: ConnectionStage,
    runtime_admitted: bool,
    lanes_admitted: bool,
) -> Option<AppNotice> {
    match connection {
        ConnectionStage::Starting => Some(AppNotice::LoadingApp),
        ConnectionStage::Connecting => Some(AppNotice::OpeningResident),
        ConnectionStage::Offline => Some(AppNotice::Reconnecting),
        ConnectionStage::Online if !runtime_admitted => Some(AppNotice::LoadingApp),
        ConnectionStage::Online if !lanes_admitted => Some(AppNotice::LoadingAgents),
        ConnectionStage::Online => None,
    }
}

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

/// One admitted durable app URL state.
///
/// Task overviews are browser-local overlays and therefore never refine this
/// route or create history entries.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppRoute {
    Dash,
    Chat { card: Option<String> },
}

/// One history transition a navigation makes.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteStep {
    Push(AppRoute),
}

/// Settled evidence for the data a durable route names.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RouteEvidence {
    pub(crate) card: Option<bool>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl AppRoute {
    #[must_use]
    pub(crate) const fn destination(destination: AppDestination) -> Self {
        match destination {
            AppDestination::Dash => Self::Dash,
            AppDestination::Chat => Self::Chat { card: None },
        }
    }

    pub(crate) fn card(target: String) -> Option<Self> {
        valid_card_target(&target).then_some(Self::Chat { card: Some(target) })
    }

    /// Parse one exact canonical path; any other spelling is not a route.
    #[must_use]
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let mut segments = path.strip_prefix('/')?.split('/');
        match segments.next()? {
            "dash" => segments.next().is_none().then_some(Self::Dash),
            "chat" => match (segments.next(), segments.next(), segments.next()) {
                (None, ..) => Some(Self::Chat { card: None }),
                (Some(CARD_SEGMENT), Some(target), None) => Self::card(target.to_owned()),
                _ => None,
            },
            _ => None,
        }
    }

    #[must_use]
    pub(crate) const fn app_destination(&self) -> AppDestination {
        match self {
            Self::Dash => AppDestination::Dash,
            Self::Chat { .. } => AppDestination::Chat,
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) fn card_target(&self) -> Option<&str> {
        match self {
            Self::Chat { card } => card.as_deref(),
            Self::Dash => None,
        }
    }

    /// The one canonical path spelling this route.
    #[must_use]
    pub(crate) fn path(&self) -> String {
        match self {
            Self::Dash => AppDestination::Dash.path().to_owned(),
            Self::Chat { card: None } => AppDestination::Chat.path().to_owned(),
            Self::Chat { card: Some(card) } => format!("/chat/{CARD_SEGMENT}/{card}"),
        }
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    /// Show one durable global destination: a new entry unless already there.
    #[must_use]
    pub(crate) fn show(&self, destination: AppDestination) -> Option<RouteStep> {
        let next = Self::destination(destination);
        (*self != next).then_some(RouteStep::Push(next))
    }

    /// Refuse a missing Chat card back to plain Chat.
    #[must_use]
    pub(crate) fn unavailable(&self, evidence: RouteEvidence) -> Option<(Self, String)> {
        match self {
            Self::Chat { card: Some(card) } if evidence.card == Some(false) => Some((
                Self::destination(AppDestination::Chat),
                format!("Linked card {card} is unavailable in retained Chat history; opened Chat."),
            )),
            Self::Dash | Self::Chat { .. } => None,
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const CARD_SEGMENT: &str = "card";
#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
const TEAM_STREAM_CARD_PREFIX: &str = "team-stream-";

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn team_stream_card_target(team: wire::CanonicalUuid) -> String {
    format!("{TEAM_STREAM_CARD_PREFIX}{team}")
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn team_stream_card_team(target: &str) -> Option<wire::CanonicalUuid> {
    target.strip_prefix(TEAM_STREAM_CARD_PREFIX)?.parse().ok()
}

/// One browser location resolved to a usable app route.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedAppLocation {
    route: AppRoute,
    replace: bool,
    notice: Option<String>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl NormalizedAppLocation {
    #[must_use]
    pub(crate) const fn route(&self) -> &AppRoute {
        &self.route
    }

    #[must_use]
    pub(crate) const fn needs_replace(&self) -> bool {
        self.replace
    }

    #[must_use]
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const OPENING_AUTH_TOKEN_MIN_BYTES: usize = 16;
#[cfg(any(target_arch = "wasm32", test))]
const OPENING_AUTH_TOKEN_MAX_BYTES: usize = 256;

/// The one-time bearer token carried by an authenticated app opening.
///
/// This mirrors the resident's published URL-safe token contract. Canonical
/// routes contain a slash and shorthand destinations are below its minimum.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn opening_auth_token(hash: &str) -> Option<&str> {
    let token = hash.strip_prefix('#')?;
    app_auth_token_is_valid(token).then_some(token)
}

#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn app_auth_token_is_valid(token: &str) -> bool {
    (OPENING_AUTH_TOKEN_MIN_BYTES..=OPENING_AUTH_TOKEN_MAX_BYTES).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

/// Resolve a browser query and hash through the one app route grammar.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn normalize_app_location(search: &str, hash: &str) -> NormalizedAppLocation {
    let fragment = hash.strip_prefix('#').unwrap_or(hash);
    if search.is_empty() {
        if let Some(route) = AppRoute::from_path(fragment) {
            let replace = hash.strip_prefix('#') != Some(route.path().as_str());
            return NormalizedAppLocation {
                route,
                replace,
                notice: None,
            };
        }
        if let Some(destination) = normalized_destination(fragment) {
            return NormalizedAppLocation {
                route: AppRoute::destination(destination),
                replace: true,
                notice: None,
            };
        }
    }

    NormalizedAppLocation {
        route: AppRoute::destination(AppDestination::Chat),
        replace: true,
        notice: Some(format!("Unrecognized route `{search}{hash}`; opened Chat.")),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn normalized_destination(fragment: &str) -> Option<AppDestination> {
    let name = fragment.trim_matches('/');
    if name.contains('/') {
        return None;
    }
    match name {
        "" | "chat" => Some(AppDestination::Chat),
        "dash" => Some(AppDestination::Dash),
        _ => None,
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) fn card_href(target: &str) -> String {
    let route = AppRoute::card(target.to_owned())
        .expect("a rendered app card has a canonical route target");
    format!("#{}", route.path())
}

/// Link one retained quote source to its durable Chat card.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) fn quote_source_href(source: &str) -> String {
    card_href(source)
}

#[cfg(any(target_arch = "wasm32", test))]
fn valid_card_target(target: &str) -> bool {
    !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests;
