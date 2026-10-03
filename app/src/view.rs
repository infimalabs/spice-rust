//! The derived task and team view axes and the app destinations over them.
//!
//! Flow and topology are the complete analytical view vocabulary. A new plane
//! lands on one of these two rather than becoming a third view, and a feature
//! that cannot make that mapping is a contract gap rather than a new axis. The
//! set is closed for the same reason the command registry is: a match over it
//! is exhaustive.

#[cfg(any(target_arch = "wasm32", test))]
use wire::TaskStamp;

#[cfg(any(target_arch = "wasm32", test))]
use crate::work::{WorkScope, WorkStemIdentity, WorkVisibility};

#[cfg(any(target_arch = "wasm32", test))]
use crate::runtime::DirectionRef;

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
    Work,
    Chat,
}

impl AppDestination {
    /// Every global destination, left to right.
    pub const ALL: [Self; 3] = [Self::Dash, Self::Work, Self::Chat];
    /// Exact hash-history path for this destination.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Dash => "/dash",
            Self::Work => "/work",
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

/// One admitted app URL state: the complete addressable view.
///
/// Global destinations remain the complete navigation vocabulary. A Work
/// scope, task detail, or the operator's Directions refine Work and a card
/// refines Chat, so a route can never select an impossible global view.
/// Dash has no subordinate selection; per-team modes, menus, and
/// arrangement stay browser-local.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppRoute {
    Dash,
    Work {
        scope: WorkScope,
        task: Option<TaskStamp>,
    },
    /// All open Work with its operator Directions in view, narrowed to one
    /// team or focused on one direction when named.
    Directions {
        focus: Option<DirectionsFocus>,
    },
    Chat {
        card: Option<String>,
    },
}

/// What a Directions route names within operator direction history.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirectionsFocus {
    /// Directions sent to one team's members.
    Team(wire::CanonicalUuid),
    /// One direction, read from the page that holds it.
    Direction(DirectionRef),
}

/// One history transition a navigation makes.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteStep {
    Push(AppRoute),
    Replace(AppRoute),
}

/// Settled evidence for the data a route names; `None` until authoritative
/// state resolves that segment.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RouteEvidence {
    pub(crate) scope: Option<bool>,
    pub(crate) task: Option<bool>,
    pub(crate) card: Option<bool>,
    pub(crate) direction: Option<bool>,
}

/// The scope a Directions route shows its history within.
#[cfg(any(target_arch = "wasm32", test))]
static DIRECTIONS_SCOPE: WorkScope = WorkScope::AllOpen;

#[cfg(any(target_arch = "wasm32", test))]
impl AppRoute {
    #[must_use]
    pub(crate) const fn destination(destination: AppDestination) -> Self {
        match destination {
            AppDestination::Dash => Self::Dash,
            AppDestination::Work => Self::Work {
                scope: WorkScope::AllOpen,
                task: None,
            },
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
            "work" => {
                let rest = segments.collect::<Vec<_>>();
                match rest.as_slice() {
                    [DIRECTIONS_SEGMENT] => return Some(Self::Directions { focus: None }),
                    [DIRECTIONS_SEGMENT, TEAM_SEGMENT, team] => {
                        return Some(Self::Directions {
                            focus: Some(DirectionsFocus::Team(team.parse().ok()?)),
                        });
                    }
                    [DIRECTION_SEGMENT, direction] => {
                        return Some(Self::Directions {
                            focus: Some(DirectionsFocus::Direction(direction_ref(direction)?)),
                        });
                    }
                    _ => {}
                }
                let (scope, rest) = match rest.as_slice() {
                    [TEAM_SEGMENT, team, rest @ ..] => (WorkScope::Team(team.parse().ok()?), rest),
                    [visibility, stem, rest @ ..]
                        if matches!(*visibility, PUBLIC_SEGMENT | HIDDEN_SEGMENT) =>
                    {
                        let visibility = if *visibility == PUBLIC_SEGMENT {
                            WorkVisibility::Public
                        } else {
                            WorkVisibility::Hidden
                        };
                        let stem = WorkStemIdentity::new(visibility, decode_segment(stem)?);
                        (WorkScope::Stem(stem), rest)
                    }
                    rest => (WorkScope::AllOpen, rest),
                };
                let task = match rest {
                    [] => None,
                    [TASK_SEGMENT, task] => Some(task.parse().ok()?),
                    _ => return None,
                };
                Some(Self::Work { scope, task })
            }
            _ => None,
        }
    }

    #[must_use]
    pub(crate) const fn app_destination(&self) -> AppDestination {
        match self {
            Self::Dash => AppDestination::Dash,
            Self::Work { .. } | Self::Directions { .. } => AppDestination::Work,
            Self::Chat { .. } => AppDestination::Chat,
        }
    }

    #[must_use]
    pub(crate) fn card_target(&self) -> Option<&str> {
        match self {
            Self::Chat { card } => card.as_deref(),
            Self::Dash | Self::Work { .. } | Self::Directions { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn work_scope(&self) -> Option<&WorkScope> {
        match self {
            Self::Work { scope, .. } => Some(scope),
            Self::Directions { .. } => Some(&DIRECTIONS_SCOPE),
            Self::Dash | Self::Chat { .. } => None,
        }
    }

    /// The Directions refinement: `Some(None)` for the section itself.
    #[must_use]
    pub(crate) const fn directions(&self) -> Option<Option<&DirectionsFocus>> {
        match self {
            Self::Directions { focus } => Some(focus.as_ref()),
            Self::Dash | Self::Work { .. } | Self::Chat { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn task(&self) -> Option<&TaskStamp> {
        match self {
            Self::Work { task, .. } => task.as_ref(),
            Self::Dash | Self::Directions { .. } | Self::Chat { .. } => None,
        }
    }

    /// The one canonical path spelling this route.
    #[must_use]
    pub(crate) fn path(&self) -> String {
        match self {
            Self::Dash => AppDestination::Dash.path().to_owned(),
            Self::Chat { card: None } => AppDestination::Chat.path().to_owned(),
            Self::Chat { card: Some(card) } => format!("/chat/{CARD_SEGMENT}/{card}"),
            Self::Directions { focus: None } => format!("/work/{DIRECTIONS_SEGMENT}"),
            Self::Directions {
                focus: Some(DirectionsFocus::Team(team)),
            } => format!("/work/{DIRECTIONS_SEGMENT}/{TEAM_SEGMENT}/{team}"),
            Self::Directions {
                focus: Some(DirectionsFocus::Direction(direction)),
            } => format!("/work/{DIRECTION_SEGMENT}/{direction}"),
            Self::Work { scope, task } => {
                let mut path = AppDestination::Work.path().to_owned();
                match scope {
                    WorkScope::AllOpen => {}
                    WorkScope::Stem(stem) => {
                        path.push('/');
                        path.push_str(if stem.hidden() {
                            HIDDEN_SEGMENT
                        } else {
                            PUBLIC_SEGMENT
                        });
                        path.push('/');
                        encode_segment(stem.stem(), &mut path);
                    }
                    WorkScope::Team(team) => {
                        path.push('/');
                        path.push_str(TEAM_SEGMENT);
                        path.push('/');
                        path.push_str(&team.to_string());
                    }
                }
                if let Some(task) = task {
                    path.push('/');
                    path.push_str(TASK_SEGMENT);
                    path.push('/');
                    path.push_str(&task.to_string());
                }
                path
            }
        }
    }

    /// Show one global destination: a new entry unless already there.
    #[must_use]
    pub(crate) fn show(&self, destination: AppDestination) -> Option<RouteStep> {
        let next = Self::destination(destination);
        (*self != next).then_some(RouteStep::Push(next))
    }

    /// Show one Work scope. Inside Work the scope replaces the entry and an
    /// open task detail stays open; from elsewhere, Directions included, Work
    /// opens as a new entry.
    #[must_use]
    pub(crate) fn show_work(&self, scope: WorkScope) -> Option<RouteStep> {
        match self {
            Self::Work {
                scope: current,
                task,
            } => (*current != scope).then(|| {
                RouteStep::Replace(Self::Work {
                    scope,
                    task: task.clone(),
                })
            }),
            Self::Dash | Self::Directions { .. } | Self::Chat { .. } => {
                Some(RouteStep::Push(Self::Work { scope, task: None }))
            }
        }
    }

    /// Open one task detail under the Work scope that shows it. Opening
    /// pushes; switching an already-open detail to another task replaces.
    #[must_use]
    pub(crate) fn open_task(&self, task: TaskStamp, scope: WorkScope) -> Option<RouteStep> {
        let open = self.task();
        if open == Some(&task) {
            return None;
        }
        let next = Self::Work {
            scope,
            task: Some(task),
        };
        Some(if open.is_some() {
            RouteStep::Replace(next)
        } else {
            RouteStep::Push(next)
        })
    }

    /// This route with its task detail closed, when one is open.
    #[must_use]
    pub(crate) fn without_task(&self) -> Option<Self> {
        match self {
            Self::Work {
                scope,
                task: Some(_),
            } => Some(Self::Work {
                scope: scope.clone(),
                task: None,
            }),
            Self::Dash
            | Self::Work { task: None, .. }
            | Self::Directions { .. }
            | Self::Chat { .. } => None,
        }
    }

    /// The route replacing this one once settled data refuses a segment,
    /// with a notice naming the refused identifier. A missing Work scope leaves
    /// All open work, a missing task leaves its containing scope, a missing
    /// card leaves plain Chat, and a missing direction leaves Directions.
    #[must_use]
    pub(crate) fn unavailable(&self, evidence: RouteEvidence) -> Option<(Self, String)> {
        match self {
            Self::Work { scope, .. } if evidence.scope == Some(false) => Some((
                Self::destination(AppDestination::Work),
                match scope {
                    WorkScope::Team(team) => {
                        format!("Team {team} is not in the roster; showing All open work.")
                    }
                    WorkScope::Stem(stem) => {
                        format!("Work scope {stem} is no longer available; showing All open work.")
                    }
                    WorkScope::AllOpen => unreachable!("all Work is always available"),
                },
            )),
            Self::Work {
                scope,
                task: Some(task),
            } if evidence.task == Some(false) => Some((
                Self::Work {
                    scope: scope.clone(),
                    task: None,
                },
                format!("Task {task} is unavailable; closed its detail."),
            )),
            Self::Chat { card: Some(card) } if evidence.card == Some(false) => Some((
                Self::destination(AppDestination::Chat),
                format!("Linked card {card} is unavailable in retained Chat history; opened Chat."),
            )),
            Self::Directions { focus: Some(focus) } if evidence.direction == Some(false) => Some((
                Self::Directions { focus: None },
                match focus {
                    DirectionsFocus::Team(team) => {
                        format!("Team {team} is not in the roster; showing all Directions.")
                    }
                    DirectionsFocus::Direction(direction) => format!(
                        "Direction {direction} is not in operator direction history; showing Directions."
                    ),
                },
            )),
            _ => None,
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const CARD_SEGMENT: &str = "card";
#[cfg(any(target_arch = "wasm32", test))]
const DIRECTION_SEGMENT: &str = "direction";
#[cfg(any(target_arch = "wasm32", test))]
const DIRECTIONS_SEGMENT: &str = "directions";
#[cfg(any(target_arch = "wasm32", test))]
const TEAM_SEGMENT: &str = "team";
#[cfg(any(target_arch = "wasm32", test))]
const TASK_SEGMENT: &str = "task";
#[cfg(any(target_arch = "wasm32", test))]
const PUBLIC_SEGMENT: &str = "public";
#[cfg(any(target_arch = "wasm32", test))]
const HIDDEN_SEGMENT: &str = "hidden";

/// A routed direction: a canonical directive or fanout UUID, or a minted
/// acknowledgment key. Each has one spelling, so the segment round-trips.
#[cfg(any(target_arch = "wasm32", test))]
fn direction_ref(segment: &str) -> Option<DirectionRef> {
    if let Ok(identity) = segment.parse::<wire::CanonicalUuid>() {
        return (identity.to_string() == segment).then_some(DirectionRef::Identity(identity));
    }
    wire::steering_key(segment).then(|| DirectionRef::Key(segment.to_owned()))
}

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

/// Bytes a route segment carries literally; every other byte is `%XX`.
#[cfg(any(target_arch = "wasm32", test))]
const fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

#[cfg(any(target_arch = "wasm32", test))]
fn encode_segment(text: &str, into: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        if unreserved(byte) {
            into.push(char::from(byte));
        } else {
            into.push('%');
            into.push(char::from(HEX[usize::from(byte >> 4)]));
            into.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
}

/// Decode one canonical segment: unreserved bytes literal, every other byte
/// as uppercase `%XX`, UTF-8, and non-empty. Anything else is refused.
#[cfg(any(target_arch = "wasm32", test))]
fn decode_segment(segment: &str) -> Option<String> {
    const fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let value = hex(*bytes.get(index + 1)?)? << 4 | hex(*bytes.get(index + 2)?)?;
            if unreserved(value) {
                return None;
            }
            decoded.push(value);
            index += 3;
        } else if unreserved(byte) {
            decoded.push(byte);
            index += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(decoded)
        .ok()
        .filter(|text| !text.is_empty())
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
        if let Some(segment) = malformed_direction(fragment) {
            return NormalizedAppLocation {
                route: AppRoute::Directions { focus: None },
                replace: true,
                notice: Some(format!(
                    "`{segment}` names no direction or team; showing Directions."
                )),
            };
        }
    }

    NormalizedAppLocation {
        route: AppRoute::destination(AppDestination::Chat),
        replace: true,
        notice: Some(format!("Unrecognized route `{search}{hash}`; opened Chat.")),
    }
}

/// The single refused segment of a Directions route that names neither a
/// direction nor a team; any other shape is simply unrecognized.
#[cfg(any(target_arch = "wasm32", test))]
fn malformed_direction(fragment: &str) -> Option<&str> {
    let rest = fragment.strip_prefix("/work/")?;
    rest.strip_prefix("direction/")
        .or_else(|| rest.strip_prefix("directions/team/"))
        .filter(|segment| !segment.is_empty() && !segment.contains('/'))
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
        "work" => Some(AppDestination::Work),
        _ => None,
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn card_href(target: &str) -> String {
    let route = AppRoute::card(target.to_owned())
        .expect("a rendered app card has a canonical route target");
    format!("#{}", route.path())
}

/// Link one quote source: a direction source is the Work route suffix
/// `work/direction/{reference}`, and every other source names a card.
#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn quote_source_href(source: &str) -> String {
    if source.starts_with(DIRECTION_SOURCE) {
        format!("#/{source}")
    } else {
        card_href(source)
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
const DIRECTION_SOURCE: &str = "work/direction/";

/// The global Work source of an operator direction. Resident and lane
/// directions are absent from global direction history, so they have no
/// canonical source link.
#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn direction_quote_source(
    actor: &wire::Actor,
    direction: &crate::runtime::DirectionRef,
) -> Option<String> {
    matches!(actor, wire::Actor::Operator).then(|| format!("{DIRECTION_SOURCE}{direction}"))
}

/// The global Work Directions card of one fanout.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) fn global_direction_card_id(fanout: wire::CanonicalUuid) -> String {
    format!("work-direction-{fanout}")
}

/// The global Work Directions section a Directions route brings into view.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) const DIRECTIONS_SECTION_ID: &str = "work-directions";

#[cfg(any(target_arch = "wasm32", test))]
fn valid_card_target(target: &str) -> bool {
    !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests;
