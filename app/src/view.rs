//! Durable Chat locations and their retained-card targets.

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
        Some(Self {
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
/// Chat/card and Work scopes are durable locations. A Work route opens its
/// task-set overview over the still-mounted Chat surface.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppRoute {
    Chat { card: Option<String> },
    Work(WorkRoute),
}

/// Stable identity carried by one durable Work route.
///
/// Team and working-group membership is deliberately absent: the current
/// topology and group partition resolve it after hydration.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkRoute {
    AllOpen,
    Project(WorkRouteProject),
    Planning { project: Option<WorkRouteProject> },
    Team(wire::CanonicalUuid),
    Group(wire::rooms::RoomAuthorityStamp),
}

/// Location effect of one task-overview navigation intent.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) enum OverviewRouteIntent {
    Work(WorkRoute),
    BrowserLocal,
    Close,
}

/// One exact project label in a Work route.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkRouteProject(String);

#[cfg(any(target_arch = "wasm32", test))]
impl WorkRouteProject {
    #[must_use]
    pub(crate) fn new(project: String) -> Option<Self> {
        valid_work_project(&project).then_some(Self(project))
    }

    #[must_use]
    pub(crate) fn project(&self) -> &str {
        &self.0
    }
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
    pub(crate) const fn chat() -> Self {
        Self::Chat { card: None }
    }

    pub(crate) fn card(target: String) -> Option<Self> {
        valid_card_target(&target).then_some(Self::Chat { card: Some(target) })
    }

    #[must_use]
    pub(crate) const fn work(route: WorkRoute) -> Self {
        Self::Work(route)
    }

    /// Parse one exact canonical path; any other spelling is not a route.
    #[must_use]
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let mut segments = path.strip_prefix('/')?.split('/');
        let route = match (
            segments.next()?,
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) {
            ("chat", None, None, None, None) => Self::chat(),
            ("chat", Some(CARD_SEGMENT), Some(target), None, None) => {
                return Self::card(target.to_owned());
            }
            ("work", None, None, None, None) => Self::work(WorkRoute::AllOpen),
            ("work", Some("project"), Some(project), None, None) => Self::work(WorkRoute::Project(
                WorkRouteProject::new(project.to_owned())?,
            )),
            ("work", Some("planning"), None, None, None) => {
                Self::work(WorkRoute::Planning { project: None })
            }
            ("work", Some("planning"), Some(project), None, None) => {
                Self::work(WorkRoute::Planning {
                    project: Some(WorkRouteProject::new(project.to_owned())?),
                })
            }
            ("work", Some("team"), Some(team), None, None) => {
                Self::work(WorkRoute::Team(team.parse().ok()?))
            }
            ("work", Some("group"), Some(group), None, None) => {
                Self::work(WorkRoute::Group(group.parse().ok()?))
            }
            _ => return None,
        };
        Some(route)
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) fn card_target(&self) -> Option<&str> {
        match self {
            Self::Chat { card } => card.as_deref(),
            Self::Work(_) => None,
        }
    }

    #[must_use]
    pub(crate) const fn work_route(&self) -> Option<&WorkRoute> {
        match self {
            Self::Work(route) => Some(route),
            Self::Chat { .. } => None,
        }
    }

    /// The one canonical path spelling this route.
    #[must_use]
    pub(crate) fn path(&self) -> String {
        match self {
            Self::Chat { card: None } => "/chat".to_owned(),
            Self::Chat { card: Some(card) } => format!("/chat/{CARD_SEGMENT}/{card}"),
            Self::Work(route) => route.path(),
        }
    }

    /// Route replacement required by one overview transition.
    #[must_use]
    pub(crate) fn overview_transition(&self, intent: OverviewRouteIntent) -> Option<Self> {
        match intent {
            OverviewRouteIntent::Work(route) => Some(Self::Work(route)),
            OverviewRouteIntent::BrowserLocal | OverviewRouteIntent::Close
                if self.work_route().is_some() =>
            {
                Some(Self::chat())
            }
            OverviewRouteIntent::BrowserLocal | OverviewRouteIntent::Close => None,
        }
    }

    /// Refuse a missing Chat card back to plain Chat.
    #[must_use]
    pub(crate) fn unavailable(&self, evidence: RouteEvidence) -> Option<(Self, String)> {
        let Self::Chat { card: Some(card) } = self else {
            return None;
        };
        (evidence.card == Some(false)).then(|| {
            (
                Self::chat(),
                format!("Linked card {card} is unavailable in retained Chat history; opened Chat."),
            )
        })
    }
}

#[cfg(any(target_arch = "wasm32", test))]
impl WorkRoute {
    #[must_use]
    pub(crate) fn path(&self) -> String {
        match self {
            Self::AllOpen => "/work".to_owned(),
            Self::Project(project) => format!("/work/project/{}", project.project()),
            Self::Planning { project: None } => "/work/planning".to_owned(),
            Self::Planning {
                project: Some(project),
            } => format!("/work/planning/{}", project.project()),
            Self::Team(team) => format!("/work/team/{team}"),
            Self::Group(group) => format!("/work/group/{group}"),
        }
    }
}

/// One exact project label: dot-separated nonempty `[0-9a-z_]+` runs.
#[cfg(any(target_arch = "wasm32", test))]
fn valid_work_project(project: &str) -> bool {
    project.split('.').all(|run| {
        !run.is_empty()
            && run
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
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
    design: Option<wire::TaskStamp>,
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

    /// The Design whose read view a permalink arrival opens over Chat.
    #[must_use]
    pub(crate) const fn design(&self) -> Option<&wire::TaskStamp> {
        self.design.as_ref()
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const OPENING_AUTH_TOKEN_MIN_BYTES: usize = 16;
#[cfg(any(target_arch = "wasm32", test))]
const OPENING_AUTH_TOKEN_MAX_BYTES: usize = 256;

/// The one-time bearer token carried by an authenticated app opening.
///
/// This mirrors the resident's published URL-safe token contract. Canonical
/// routes contain a slash and shorthand route spellings are below its minimum.
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

/// Resolve a browser query and hash through the one Chat route grammar. A
/// Design permalink opens that Design's read view over Chat and leaves the
/// location at Chat, since the view is a browser-local overview.
#[cfg(any(target_arch = "wasm32", test))]
#[must_use]
pub(crate) fn normalize_app_location(search: &str, hash: &str) -> NormalizedAppLocation {
    let fragment = hash.strip_prefix('#').unwrap_or(hash);
    if search.is_empty() {
        if let Some(design) = fragment
            .strip_prefix(DESIGN_PREFIX)
            .and_then(|stamp| stamp.parse::<wire::TaskStamp>().ok())
        {
            return NormalizedAppLocation {
                route: AppRoute::chat(),
                replace: true,
                notice: None,
                design: Some(design),
            };
        }
        if let Some(route) = AppRoute::from_path(fragment) {
            let replace = hash.strip_prefix('#') != Some(route.path().as_str());
            return NormalizedAppLocation {
                route,
                replace,
                notice: None,
                design: None,
            };
        }
        let name = fragment.trim_matches('/');
        if !name.contains('/') && matches!(name, "" | "chat") {
            return NormalizedAppLocation {
                route: AppRoute::chat(),
                replace: true,
                notice: None,
                design: None,
            };
        }
    }

    NormalizedAppLocation {
        route: AppRoute::chat(),
        replace: true,
        notice: Some(format!("Unrecognized route `{search}{hash}`; opened Chat.")),
        design: None,
    }
}

/// The location prefix of a Design permalink, followed by its birth stamp.
#[cfg(any(target_arch = "wasm32", rust_analyzer, test))]
const DESIGN_PREFIX: &str = "/chat/design/";

/// Link one Design's read view by its immutable birth stamp.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(crate) fn design_href(design: &wire::TaskStamp) -> String {
    format!("#{DESIGN_PREFIX}{design}")
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
