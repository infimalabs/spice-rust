use wire::intent::{DirectionGroup, DirectionHistoryAnswer, DirectionWindow};
use wire::{CanonicalUuid, Disposition, Outcome, Response, Seq};

use crate::socket::Admitted;

use super::CanvasRuntime;

/// Number of fanout groups in one bounded Work Directions page.
pub const DIRECTION_HISTORY_PAGE_SIZE: u16 = 20;

/// One exact request against the current Work Directions generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectionHistoryRequest {
    pub revision: u64,
    pub window: DirectionWindow,
    pub query: Option<String>,
    pub lanes: Vec<CanonicalUuid>,
    pub dispositions: Vec<Disposition>,
    pub authored_before: Option<wire::Instant>,
    offset: u64,
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
        self.reserve_direction_history(DirectionWindow::Newest, 0)
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

    pub fn begin_older_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        let position = self.direction_history.older?;
        self.reserve_direction_history(
            DirectionWindow::Before { position },
            self.direction_history.last_result(),
        )
    }

    pub fn begin_newer_direction_history(&mut self) -> Option<DirectionHistoryRequest> {
        let position = self.direction_history.newer?;
        self.reserve_direction_history(
            DirectionWindow::After { position },
            self.direction_history
                .offset
                .saturating_sub(u64::from(DIRECTION_HISTORY_PAGE_SIZE)),
        )
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
        offset: u64,
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
            offset,
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
                self.direction_history.issue = Some(
                    refusal
                        .diagnostic()
                        .map_or("steer::history-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                );
                return;
            }
            Outcome::Fault(fault) => {
                self.direction_history.issue = Some(fault.reason.clone());
                return;
            }
        };
        if answer.limit != DIRECTION_HISTORY_PAGE_SIZE
            || answer.groups.len() > usize::from(answer.limit)
            || request.offset.saturating_add(answer.groups.len() as u64) > answer.total
            || answer
                .groups
                .windows(2)
                .any(|pair| pair[0].position <= pair[1].position)
        {
            self.direction_history.issue = Some("steer::history-answer-invalid".to_owned());
            return;
        }
        self.direction_history.groups.clone_from(&answer.groups);
        self.direction_history.offset = request.offset;
        self.direction_history.total = Some(answer.total);
        self.direction_history.older = answer.older;
        self.direction_history.newer = answer.newer;
        self.direction_history.pending = None;
        self.direction_history.issue = None;
    }
}
