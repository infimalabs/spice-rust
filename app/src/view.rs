//! The derived task and team view axes and the Canvas destinations over them.
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

/// One team-local direction named by a durable Canvas route.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectionTarget {
    team: wire::CanonicalUuid,
    direction: wire::CanonicalUuid,
}

#[cfg(any(target_arch = "wasm32", test))]
impl DirectionTarget {
    #[must_use]
    pub(crate) const fn new(team: wire::CanonicalUuid, direction: wire::CanonicalUuid) -> Self {
        Self { team, direction }
    }

    #[must_use]
    pub(crate) const fn team(&self) -> wire::CanonicalUuid {
        self.team
    }

    #[must_use]
    pub(crate) fn element_id(&self) -> String {
        format!("team-work-direction-{}-{}", self.team, self.direction)
    }
}

/// What a Chat route focuses: one retained card or one team direction.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChatTarget {
    Card(String),
    Direction(DirectionTarget),
}

/// One admitted Canvas URL state: the complete addressable view.
///
/// Global destinations remain the complete navigation vocabulary. A Work
/// scope and task detail refine Work and a card or direction refines Chat, so
/// a route can never select an impossible global view. Dash has no subordinate
/// selection; per-team modes, menus, and arrangement stay browser-local.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CanvasRoute {
    Dash,
    Work {
        scope: WorkScope,
        task: Option<TaskStamp>,
    },
    Chat {
        target: Option<ChatTarget>,
    },
}

/// One history transition a navigation makes.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteStep {
    Push(CanvasRoute),
    Replace(CanvasRoute),
}

/// Settled evidence for the data a route names; `None` until authoritative
/// state resolves that segment.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RouteEvidence {
    pub(crate) scope: Option<bool>,
    pub(crate) task: Option<bool>,
    pub(crate) card: Option<bool>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl CanvasRoute {
    #[must_use]
    pub(crate) const fn destination(destination: CanvasDestination) -> Self {
        match destination {
            CanvasDestination::Dash => Self::Dash,
            CanvasDestination::Work => Self::Work {
                scope: WorkScope::AllOpen,
                task: None,
            },
            CanvasDestination::Chat => Self::Chat { target: None },
        }
    }

    pub(crate) fn card(target: String) -> Option<Self> {
        valid_card_target(&target).then_some(Self::Chat {
            target: Some(ChatTarget::Card(target)),
        })
    }

    #[must_use]
    pub(crate) const fn direction(target: DirectionTarget) -> Self {
        Self::Chat {
            target: Some(ChatTarget::Direction(target)),
        }
    }

    /// Parse one exact canonical path; any other spelling is not a route.
    #[must_use]
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let mut segments = path.strip_prefix('/')?.split('/');
        match segments.next()? {
            "dash" => segments.next().is_none().then_some(Self::Dash),
            "chat" => match (
                segments.next(),
                segments.next(),
                segments.next(),
                segments.next(),
            ) {
                (None, ..) => Some(Self::Chat { target: None }),
                (Some(CARD_SEGMENT), Some(target), None, _) => Self::card(target.to_owned()),
                (Some(DIRECTION_SEGMENT), Some(team), Some(direction), None) => {
                    Some(Self::direction(DirectionTarget::new(
                        team.parse().ok()?,
                        direction.parse().ok()?,
                    )))
                }
                _ => None,
            },
            "work" => {
                let rest = segments.collect::<Vec<_>>();
                let (scope, rest) = match rest.as_slice() {
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
    pub(crate) const fn canvas_destination(&self) -> CanvasDestination {
        match self {
            Self::Dash => CanvasDestination::Dash,
            Self::Work { .. } => CanvasDestination::Work,
            Self::Chat { .. } => CanvasDestination::Chat,
        }
    }

    #[must_use]
    pub(crate) fn card_target(&self) -> Option<&str> {
        match self {
            Self::Chat {
                target: Some(ChatTarget::Card(card)),
            } => Some(card),
            Self::Dash | Self::Work { .. } | Self::Chat { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn direction_target(&self) -> Option<&DirectionTarget> {
        match self {
            Self::Chat {
                target: Some(ChatTarget::Direction(target)),
            } => Some(target),
            Self::Dash | Self::Work { .. } | Self::Chat { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn work_scope(&self) -> Option<&WorkScope> {
        match self {
            Self::Work { scope, .. } => Some(scope),
            Self::Dash | Self::Chat { .. } => None,
        }
    }

    #[must_use]
    pub(crate) const fn task(&self) -> Option<&TaskStamp> {
        match self {
            Self::Work { task, .. } => task.as_ref(),
            Self::Dash | Self::Chat { .. } => None,
        }
    }

    /// The one canonical path spelling this route.
    #[must_use]
    pub(crate) fn path(&self) -> String {
        match self {
            Self::Dash => CanvasDestination::Dash.path().to_owned(),
            Self::Chat { target: None } => CanvasDestination::Chat.path().to_owned(),
            Self::Chat {
                target: Some(ChatTarget::Card(card)),
            } => format!("/chat/{CARD_SEGMENT}/{card}"),
            Self::Chat {
                target: Some(ChatTarget::Direction(target)),
            } => format!(
                "/chat/{}",
                direction_quote_source(target.team, target.direction)
            ),
            Self::Work { scope, task } => {
                let mut path = CanvasDestination::Work.path().to_owned();
                if let Some(stem) = scope.stem() {
                    path.push('/');
                    path.push_str(if stem.hidden() {
                        HIDDEN_SEGMENT
                    } else {
                        PUBLIC_SEGMENT
                    });
                    path.push('/');
                    encode_segment(stem.stem(), &mut path);
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
    pub(crate) fn show(&self, destination: CanvasDestination) -> Option<RouteStep> {
        let next = Self::destination(destination);
        (*self != next).then_some(RouteStep::Push(next))
    }

    /// Show one Work scope. Inside Work the scope replaces the entry and an
    /// open task detail stays open; from elsewhere Work opens as a new entry.
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
            Self::Dash | Self::Chat { .. } => {
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
            Self::Dash | Self::Work { task: None, .. } | Self::Chat { .. } => None,
        }
    }

    /// The route replacing this one once settled data refuses a segment,
    /// with a notice naming the refused identifier. A missing stem leaves
    /// All open work, a missing task leaves its containing scope, and a
    /// missing card leaves plain Chat.
    #[must_use]
    pub(crate) fn unavailable(&self, evidence: RouteEvidence) -> Option<(Self, String)> {
        match self {
            Self::Work {
                scope: WorkScope::Stem(stem),
                ..
            } if evidence.scope == Some(false) => Some((
                Self::destination(CanvasDestination::Work),
                format!("Work scope {stem} is no longer available; showing All open work."),
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
            Self::Chat {
                target: Some(ChatTarget::Card(card)),
            } if evidence.card == Some(false) => Some((
                Self::destination(CanvasDestination::Chat),
                format!("Linked card {card} is unavailable in retained Chat history; opened Chat."),
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
const TASK_SEGMENT: &str = "task";
#[cfg(any(target_arch = "wasm32", test))]
const PUBLIC_SEGMENT: &str = "public";
#[cfg(any(target_arch = "wasm32", test))]
const HIDDEN_SEGMENT: &str = "hidden";

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

/// One browser location resolved to a usable Canvas route.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NormalizedCanvasLocation {
    route: CanvasRoute,
    replace: bool,
    notice: Option<String>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl NormalizedCanvasLocation {
    #[must_use]
    pub(crate) const fn route(&self) -> &CanvasRoute {
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

/// The one-time bearer token carried by an authenticated Canvas opening.
///
/// This mirrors the resident's published URL-safe token contract. Canonical
/// routes contain a slash and shorthand destinations are below its minimum.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn opening_auth_token(hash: &str) -> Option<&str> {
    let token = hash.strip_prefix('#')?;
    canvas_auth_token_is_valid(token).then_some(token)
}

#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn canvas_auth_token_is_valid(token: &str) -> bool {
    (OPENING_AUTH_TOKEN_MIN_BYTES..=OPENING_AUTH_TOKEN_MAX_BYTES).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

/// Resolve a browser query and hash through the one Canvas route grammar.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn normalize_canvas_location(search: &str, hash: &str) -> NormalizedCanvasLocation {
    let fragment = hash.strip_prefix('#').unwrap_or(hash);
    if search.is_empty() {
        if let Some(route) = CanvasRoute::from_path(fragment) {
            let replace = hash.strip_prefix('#') != Some(route.path().as_str());
            return NormalizedCanvasLocation {
                route,
                replace,
                notice: None,
            };
        }
        if let Some(destination) = normalized_destination(fragment) {
            return NormalizedCanvasLocation {
                route: CanvasRoute::destination(destination),
                replace: true,
                notice: None,
            };
        }
    }

    NormalizedCanvasLocation {
        route: CanvasRoute::destination(CanvasDestination::Chat),
        replace: true,
        notice: Some(format!(
            "Unrecognized Canvas route `{search}{hash}`; opened Chat."
        )),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn normalized_destination(fragment: &str) -> Option<CanvasDestination> {
    let name = fragment.trim_matches('/');
    if name.contains('/') {
        return None;
    }
    match name {
        "" | "chat" => Some(CanvasDestination::Chat),
        "dash" => Some(CanvasDestination::Dash),
        "work" => Some(CanvasDestination::Work),
        _ => None,
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn card_href(target: &str) -> String {
    let route = CanvasRoute::card(target.to_owned())
        .expect("a rendered Canvas card has a canonical route target");
    format!("#{}", route.path())
}

/// Link one quote source: a direction source is itself the Chat route suffix
/// `direction/{team}/{direction}`; every other source names a card.
#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn quote_source_href(source: &str) -> String {
    source
        .strip_prefix(DIRECTION_SEGMENT)
        .filter(|rest| rest.starts_with('/'))
        .map_or_else(|| card_href(source), |_| format!("#/chat/{source}"))
}

#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
pub(crate) fn direction_quote_source(
    team: wire::CanonicalUuid,
    direction: wire::CanonicalUuid,
) -> String {
    format!("{DIRECTION_SEGMENT}/{team}/{direction}")
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
        DirectionTarget, RouteEvidence, RouteStep, canvas_notice, normalize_canvas_location,
        opening_auth_token, quote_source_href,
    };
    use crate::work::{WorkScope, WorkStemIdentity, WorkVisibility};

    const CARD: &str = "message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1";

    fn stem(visibility: WorkVisibility, stem: &str) -> WorkScope {
        WorkScope::Stem(WorkStemIdentity::new(visibility, stem.to_owned()))
    }

    fn task(stamp: &str) -> wire::TaskStamp {
        stamp.parse().expect("fixture task stamp")
    }

    fn work(scope: WorkScope, task: Option<wire::TaskStamp>) -> CanvasRoute {
        CanvasRoute::Work { scope, task }
    }

    #[test]
    fn canonical_routes_round_trip_exactly() {
        for (path, route) in [
            ("/dash", CanvasRoute::Dash),
            ("/work", work(WorkScope::AllOpen, None)),
            (
                "/work/task/dTv7zpzTN",
                work(WorkScope::AllOpen, Some(task("dTv7zpzTN"))),
            ),
            (
                "/work/public/canvas",
                work(stem(WorkVisibility::Public, "canvas"), None),
            ),
            (
                "/work/hidden/oops/task/dTv7zpzT",
                work(stem(WorkVisibility::Hidden, "oops"), Some(task("dTv7zpzT"))),
            ),
            (
                "/work/public/a%20b%2F%C3%A9.-_~",
                work(stem(WorkVisibility::Public, "a b/é.-_~"), None),
            ),
            ("/chat", CanvasRoute::Chat { target: None }),
            (
                "/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1",
                CanvasRoute::card(CARD.to_owned()).expect("fixture card"),
            ),
            (
                concat!(
                    "/chat/direction/01234567-89ab-cdef-0123-456789abcdef/",
                    "fedcba98-7654-3210-fedc-ba9876543210",
                ),
                CanvasRoute::direction(direction_fixture()),
            ),
        ] {
            assert_eq!(CanvasRoute::from_path(path), Some(route.clone()), "{path}");
            assert_eq!(route.path(), path);
        }
        let card = CanvasRoute::from_path(&format!("/chat/card/{CARD}")).expect("card route");
        assert_eq!(card.canvas_destination(), CanvasDestination::Chat);
        assert_eq!(card.card_target(), Some(CARD));
    }

    fn direction_fixture() -> DirectionTarget {
        DirectionTarget::new(
            "01234567-89ab-cdef-0123-456789abcdef"
                .parse()
                .expect("fixture team"),
            "fedcba98-7654-3210-fedc-ba9876543210"
                .parse()
                .expect("fixture direction"),
        )
    }

    #[test]
    fn direction_routes_refine_chat_and_name_the_exact_team_card() {
        let target = direction_fixture();
        let route = CanvasRoute::direction(target.clone());
        assert_eq!(route.canvas_destination(), CanvasDestination::Chat);
        assert_eq!(route.direction_target(), Some(&target));
        assert_eq!(route.card_target(), None);
        assert_eq!(
            target.element_id(),
            concat!(
                "team-work-direction-01234567-89ab-cdef-0123-456789abcdef-",
                "fedcba98-7654-3210-fedc-ba9876543210",
            )
        );
        assert_eq!(
            quote_source_href(&super::direction_quote_source(
                target.team(),
                "fedcba98-7654-3210-fedc-ba9876543210"
                    .parse()
                    .expect("fixture direction"),
            )),
            format!("#{}", route.path()),
        );
        assert_eq!(quote_source_href(CARD), format!("#/chat/card/{CARD}"));
    }

    #[test]
    fn only_resident_token_fragments_become_opening_credentials() {
        assert_eq!(
            opening_auth_token("#remote-token-0123456789"),
            Some("remote-token-0123456789")
        );
        for hash in [
            "",
            "#dash",
            "#/chat",
            "#/work",
            "#/chat/card/message-agent-01234567",
            "#short",
            "#remote/token/0123456789",
            &format!("#{}", "a".repeat(257)),
        ] {
            assert_eq!(opening_auth_token(hash), None, "{hash}");
        }
    }

    #[test]
    fn stem_visibility_is_part_of_route_identity() {
        let public = CanvasRoute::from_path("/work/public/oops").expect("public stem");
        let hidden = CanvasRoute::from_path("/work/hidden/oops").expect("hidden stem");
        assert_ne!(public, hidden);
        assert_eq!(
            public.work_scope(),
            Some(&stem(WorkVisibility::Public, "oops"))
        );
        assert_eq!(
            hidden.work_scope(),
            Some(&stem(WorkVisibility::Hidden, "oops"))
        );
    }

    #[test]
    fn malformed_or_noncanonical_segments_are_not_routes() {
        for path in [
            "/card/message-agent",
            "/chat/card/",
            "/chat/card/message/agent",
            "/chat/card/message_agent",
            "/chat/message-agent",
            "/dash/card/message-agent",
            "/work/public",
            "/work/public/",
            "/work/private/canvas",
            "/work/public/a%2",
            "/work/public/a%zz",
            "/work/public/a%2f",
            "/work/public/%61",
            "/work/public/a b",
            "/work/public/%FF",
            "/work/task",
            "/work/task/dTv7zpzTNx",
            "/work/task/aeiouaeio",
            "/work/public/canvas/task/dTv7zpzTN/",
            "/work/public/canvas/card/dTv7zpzTN",
            "/direction/01234567-89ab-cdef-0123-456789abcdef/fedcba98-7654-3210-fedc-ba9876543210",
            "/chat/direction/not-a-team/not-a-direction",
            "/chat/direction/01234567-89ab-cdef-0123-456789abcdef",
        ] {
            assert_eq!(CanvasRoute::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn canvas_locations_canonicalize_destination_spellings_without_changing_routes() {
        for (hash, destination, replace) in [
            ("#/chat", CanvasDestination::Chat, false),
            ("#/dash", CanvasDestination::Dash, false),
            ("#/work", CanvasDestination::Work, false),
            ("", CanvasDestination::Chat, true),
            ("#", CanvasDestination::Chat, true),
            ("#/", CanvasDestination::Chat, true),
            ("#dash", CanvasDestination::Dash, true),
            ("#/work/", CanvasDestination::Work, true),
            ("#//dash/", CanvasDestination::Dash, true),
        ] {
            let location = normalize_canvas_location("", hash);
            assert_eq!(location.route().canvas_destination(), destination, "{hash}");
            assert_eq!(location.needs_replace(), replace, "{hash}");
            assert_eq!(location.notice(), None, "{hash}");
        }

        for hash in [
            "#/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1",
            "#/work/hidden/oops/task/dTv7zpzTN",
        ] {
            let location = normalize_canvas_location("", hash);
            assert_eq!(format!("#{}", location.route().path()), hash);
            assert!(!location.needs_replace(), "{hash}");
            assert_eq!(location.notice(), None, "{hash}");
        }
    }

    #[test]
    fn unknown_canvas_locations_open_chat_and_name_the_refused_input() {
        for (search, hash) in [
            ("?page=work", ""),
            ("", "#/unknown"),
            ("", "#/work?scope=all"),
            ("", "#/card/message-agent"),
            ("", "#/work/public/a%2f"),
        ] {
            let location = normalize_canvas_location(search, hash);
            assert_eq!(location.route(), &CanvasRoute::Chat { target: None });
            assert!(location.needs_replace());
            let notice = location.notice().expect("invalid input is named");
            assert!(notice.contains(&format!("{search}{hash}")), "{notice}");
        }
    }

    #[test]
    fn history_steps_push_openings_and_replace_changes_inside_one_view() {
        let canvas = stem(WorkVisibility::Public, "canvas");
        let all = work(WorkScope::AllOpen, None);
        let open = work(canvas.clone(), Some(task("dTv7zpzTN")));
        let chat = CanvasRoute::Chat { target: None };

        assert_eq!(chat.show(CanvasDestination::Chat), None);
        assert_eq!(
            open.show(CanvasDestination::Work),
            Some(RouteStep::Push(all.clone()))
        );
        assert_eq!(
            CanvasRoute::card(CARD.to_owned())
                .expect("fixture card")
                .show(CanvasDestination::Chat),
            Some(RouteStep::Push(chat.clone()))
        );

        assert_eq!(
            chat.show_work(canvas.clone()),
            Some(RouteStep::Push(work(canvas.clone(), None)))
        );
        assert_eq!(
            all.show_work(canvas.clone()),
            Some(RouteStep::Replace(work(canvas.clone(), None)))
        );
        assert_eq!(
            open.show_work(WorkScope::AllOpen),
            Some(RouteStep::Replace(work(
                WorkScope::AllOpen,
                Some(task("dTv7zpzTN"))
            )))
        );
        assert_eq!(open.show_work(canvas.clone()), None);

        assert_eq!(
            all.open_task(task("dTv7zpzTN"), canvas.clone()),
            Some(RouteStep::Push(open.clone()))
        );
        assert_eq!(
            open.open_task(task("dTv7xh1gD"), WorkScope::AllOpen),
            Some(RouteStep::Replace(work(
                WorkScope::AllOpen,
                Some(task("dTv7xh1gD"))
            )))
        );
        assert_eq!(open.open_task(task("dTv7zpzTN"), WorkScope::AllOpen), None);

        assert_eq!(open.without_task(), Some(work(canvas, None)));
        assert_eq!(all.without_task(), None);
        assert_eq!(chat.without_task(), None);
    }

    #[test]
    fn settled_missing_segments_rewrite_to_their_containing_view() {
        let canvas = stem(WorkVisibility::Hidden, "canvas");
        let open = work(canvas.clone(), Some(task("dTv7zpzTN")));
        let card = CanvasRoute::card(CARD.to_owned()).expect("fixture card");
        let pending = RouteEvidence::default();
        let settled = |scope, task, card| RouteEvidence { scope, task, card };

        assert_eq!(open.unavailable(pending), None);
        assert_eq!(card.unavailable(pending), None);
        assert_eq!(
            open.unavailable(settled(Some(true), Some(true), None)),
            None
        );

        let (next, notice) = open
            .unavailable(settled(Some(false), None, None))
            .expect("missing stem refuses");
        assert_eq!(next, work(WorkScope::AllOpen, None));
        assert!(notice.contains(".canvas.*"), "{notice}");

        let (next, notice) = open
            .unavailable(settled(Some(false), Some(false), None))
            .expect("a missing stem outranks its task");
        assert_eq!(next, work(WorkScope::AllOpen, None));
        assert!(notice.contains(".canvas.*"), "{notice}");

        let (next, notice) = open
            .unavailable(settled(Some(true), Some(false), None))
            .expect("missing task refuses");
        assert_eq!(next, work(canvas, None));
        assert!(notice.contains("dTv7zpzTN"), "{notice}");

        let (next, notice) = card
            .unavailable(settled(None, None, Some(false)))
            .expect("missing card refuses");
        assert_eq!(next, CanvasRoute::Chat { target: None });
        assert!(notice.contains(CARD), "{notice}");
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
