use wire::intent::{DirectionGroup, DirectionHistoryAnswer, DirectionWindow};
use wire::{CanonicalUuid, Disposition, Outcome, Response, Seq};

use crate::socket::Admitted;

use super::CanvasRuntime;

/// Number of fanout groups in one bounded Work Directions page.
pub const DIRECTION_HISTORY_PAGE_SIZE: u16 = 20;

/// Refusal naming an anchor no operator direction holds.
const ANCHOR_UNKNOWN: &str = "steer::history-anchor-unknown";

/// One operator direction a durable route names: a directive or fanout
/// identity, or a directive's acknowledgment key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectionRef {
    Identity(CanonicalUuid),
    Key(String),
}

impl DirectionRef {
    /// Whether this group is the direction named.
    #[must_use]
    pub fn names(&self, group: &DirectionGroup) -> bool {
        match self {
            Self::Identity(identity) => {
                group.fanout == *identity
                    || group
                        .recipients
                        .iter()
                        .any(|recipient| recipient.directive == *identity)
            }
            Self::Key(key) => group
                .recipients
                .iter()
                .any(|recipient| recipient.key == *key),
        }
    }

    fn window(&self) -> DirectionWindow {
        match self {
            Self::Identity(identity) => DirectionWindow::Anchor {
                identity: *identity,
            },
            Self::Key(key) => DirectionWindow::AnchorKey { key: key.clone() },
        }
    }
}

impl std::fmt::Display for DirectionRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity(identity) => identity.fmt(formatter),
            Self::Key(key) => formatter.write_str(key),
        }
    }
}

/// One exact request against the current Work Directions generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectionHistoryRequest {
    pub revision: u64,
    pub window: DirectionWindow,
    pub query: Option<String>,
    pub lanes: Vec<CanonicalUuid>,
    pub dispositions: Vec<Disposition>,
    pub authored_before: Option<wire::Instant>,
}

/// Capability-scoped state for one replaceable operator-direction page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectionHistoryState {
    revision: u64,
    query: Option<String>,
    lanes: Vec<CanonicalUuid>,
    dispositions: Vec<Disposition>,
    authored_before: Option<wire::Instant>,
    groups: Vec<DirectionGroup>,
    offset: u64,
    total: Option<u64>,
    older: Option<Seq>,
    newer: Option<Seq>,
    pending: Option<DirectionHistoryRequest>,
    loading: bool,
    issue: Option<String>,
    /// An anchored read's target that no operator direction holds.
    missing: Option<DirectionRef>,
}

impl DirectionHistoryState {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn groups(&self) -> &[DirectionGroup] {
        &self.groups
    }

    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    #[must_use]
    pub const fn loading(&self) -> bool {
        self.loading
    }

    #[must_use]
    pub fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }

    #[must_use]
    pub const fn first_result(&self) -> u64 {
        if self.groups.is_empty() {
            0
        } else {
            self.offset.saturating_add(1)
        }
    }

    #[must_use]
    pub fn last_result(&self) -> u64 {
        self.offset
            .saturating_add(u64::try_from(self.groups.len()).unwrap_or(u64::MAX))
    }

    #[must_use]
    pub const fn has_newer(&self) -> bool {
        self.newer.is_some()
    }

    #[must_use]
    pub const fn has_older(&self) -> bool {
        self.older.is_some()
    }

    #[must_use]
    pub fn filtered(&self) -> bool {
        self.query.is_some()
            || !self.lanes.is_empty()
            || !self.dispositions.is_empty()
            || self.authored_before.is_some()
    }

    /// Settled evidence for one routed direction: held by the loaded page,
    /// refused by an anchored read, or not yet known.
    #[must_use]
    pub fn locate(&self, target: &DirectionRef) -> Option<bool> {
        if self.groups.iter().any(|group| target.names(group)) {
            Some(true)
        } else if self.missing.as_ref() == Some(target) {
            Some(false)
        } else {
            None
        }
    }

    /// The loaded group a route names.
    #[must_use]
    pub fn group(&self, target: &DirectionRef) -> Option<&DirectionGroup> {
        self.groups.iter().find(|group| target.names(group))
    }

    /// Whether a read anchored on this target is in flight.
    #[must_use]
    pub fn anchoring(&self, target: &DirectionRef) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.window == target.window())
    }

    pub(super) const fn active(&self) -> bool {
        self.total.is_some() || self.loading || self.pending.is_some()
    }

    fn reset_page(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.groups.clear();
        self.offset = 0;
        self.total = None;
        self.older = None;
        self.newer = None;
        self.pending = None;
        self.loading = false;
        self.issue = None;
        self.missing = None;
    }
}

/// Convert a date-only authored cutoff into its inclusive UTC day boundary.
pub fn direction_history_authored_cutoff(value: &str) -> Result<Option<wire::Instant>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    format!("{value}T23:59:59.999Z")
        .parse()
        .map(Some)
        .map_err(|_| "Choose a valid authored date.".to_owned())
}

impl CanvasRuntime {
    #[must_use]
    pub const fn direction_history(&self) -> &DirectionHistoryState {
        &self.direction_history
    }
    pub(crate) fn invalidate_direction_history(&mut self) {
        if self.direction_history.active() {
            self.direction_history.reset_page();
        }
    }

    /// Reserve the newest direction page once per filter generation.
    pub fn begin_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        if self.direction_history.total.is_some() {
            return None;
        }
        self.reserve_direction_history(DirectionWindow::Newest)
    }

    /// Replace every server-side filter and return to the newest matching page.
    pub fn apply_direction_history_filters(
        &mut self,
        query: String,
        mut lanes: Vec<CanonicalUuid>,
        mut dispositions: Vec<Disposition>,
        authored_before: Option<wire::Instant>,
    ) -> Option<DirectionHistoryRequest> {
        lanes.sort_unstable();
        lanes.dedup();
        dispositions.sort_unstable();
        dispositions.dedup();
        self.direction_history.query = match query.trim() {
            "" => None,
            query => Some(query.to_owned()),
        };
        self.direction_history.lanes = lanes;
        self.direction_history.dispositions = dispositions;
        self.direction_history.authored_before = authored_before;
        self.direction_history.reset_page();
        self.begin_direction_history()
    }

    /// Clear every filter and read the page starting at one routed direction.
    pub fn anchor_direction_history(
        &mut self,
        target: &DirectionRef,
    ) -> Option<DirectionHistoryRequest> {
        let history = &mut self.direction_history;
        history.query = None;
        history.lanes.clear();
        history.dispositions.clear();
        history.authored_before = None;
        history.reset_page();
        self.reserve_direction_history(target.window())
    }

    pub fn begin_older_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        let position = self.direction_history.older?;
        self.reserve_direction_history(DirectionWindow::Before { position })
    }

    pub fn begin_newer_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        let position = self.direction_history.newer?;
        self.reserve_direction_history(DirectionWindow::After { position })
    }

    pub fn retry_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        if self.direction_history.loading {
            return None;
        }
        let request = self.direction_history.pending.clone()?;
        self.direction_history.loading = true;
        self.direction_history.issue = None;
        Some(request)
    }

    pub fn refuse_direction_history_request(
        &mut self,
        request: &DirectionHistoryRequest,
        issue: String,
    ) {
        if self.direction_history.pending.as_ref() == Some(request) {
            self.direction_history.loading = false;
            self.direction_history.issue = Some(issue);
        }
    }

    fn reserve_direction_history(
        &mut self,
        window: DirectionWindow,
    ) -> Option<DirectionHistoryRequest> {
        if self.direction_history.loading {
            return None;
        }
        let request = DirectionHistoryRequest {
            revision: self.direction_history.revision,
            window,
            query: self.direction_history.query.clone(),
            lanes: self.direction_history.lanes.clone(),
            dispositions: self.direction_history.dispositions.clone(),
            authored_before: self.direction_history.authored_before,
        };
        self.direction_history.loading = true;
        self.direction_history.issue = None;
        self.direction_history.pending = Some(request.clone());
        Some(request)
    }

    /// Replace the visible page with one exact correlated resident answer.
    pub fn apply_direction_history(
        &mut self,
        request: &DirectionHistoryRequest,
        admitted: &Admitted<Response<DirectionHistoryAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope()
            || request.revision != self.direction_history.revision
            || self.direction_history.pending.as_ref() != Some(request)
        {
            return;
        }
        self.direction_history.loading = false;
        let answer = match &admitted.get().outcome {
            Outcome::Reply(reply) => &reply.body,
            Outcome::Refusal(refusal) => {
                let reason = refusal
                    .diagnostic()
                    .map_or("steer::history-refused", wire::RefusalDiagnostic::reason);
                let anchor = match &request.window {
                    DirectionWindow::Anchor { identity } => Some(DirectionRef::Identity(*identity)),
                    DirectionWindow::AnchorKey { key } => Some(DirectionRef::Key(key.clone())),
                    DirectionWindow::Newest
                    | DirectionWindow::Before { .. }
                    | DirectionWindow::After { .. } => None,
                };
                if let Some(target) = anchor.filter(|_| reason == ANCHOR_UNKNOWN) {
                    // The route names nothing; a new generation reads the
                    // newest page while the evidence refuses the route.
                    let history = &mut self.direction_history;
                    history.revision = history.revision.saturating_add(1);
                    history.pending = None;
                    history.issue = None;
                    history.missing = Some(target);
                } else {
                    self.direction_history.issue = Some(reason.to_owned());
                }
                return;
            }
            Outcome::Fault(fault) => {
                self.direction_history.issue = Some(fault.reason.clone());
                return;
            }
        };
        if answer.limit != DIRECTION_HISTORY_PAGE_SIZE
            || answer.groups.len() > usize::from(answer.limit)
            || answer.offset.saturating_add(answer.groups.len() as u64) > answer.total
            || answer
                .groups
                .windows(2)
                .any(|pair| pair[0].position <= pair[1].position)
        {
            self.direction_history.issue = Some("steer::history-answer-invalid".to_owned());
            return;
        }
        self.direction_history.groups.clone_from(&answer.groups);
        self.direction_history.offset = answer.offset;
        self.direction_history.total = Some(answer.total);
        self.direction_history.older = answer.older;
        self.direction_history.newer = answer.newer;
        self.direction_history.pending = None;
        self.direction_history.issue = None;
    }
}
