use crate::socket::Admitted;
use wire::intent::{BoardAnswer, TaskHistoryCursor};
use wire::{Outcome, Response};

use super::AppRuntime;

/// One exact request against the current Work-history generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkHistoryRequest {
    pub revision: u64,
    pub before: Option<TaskHistoryCursor>,
    pub query: Option<String>,
    pub completed_before: Option<wire::Instant>,
    offset: u64,
    direction: WorkHistoryDirection,
}

/// Convert a date-only archive cutoff into its inclusive UTC day boundary.
pub fn work_history_completion_cutoff(value: &str) -> Result<Option<wire::Instant>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    format!("{value}T23:59:59.999Z")
        .parse()
        .map(Some)
        .map_err(|_| "Choose a valid completion date.".to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkHistoryDirection {
    Current,
    Older,
    Newer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WorkHistoryPosition {
    before: Option<TaskHistoryCursor>,
    offset: u64,
}

/// Capability-scoped paging state for one replaceable terminal Work page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkHistoryState {
    revision: u64,
    query: Option<String>,
    completed_before: Option<wire::Instant>,
    before: Option<TaskHistoryCursor>,
    offset: u64,
    loaded: usize,
    total: Option<u64>,
    next: Option<TaskHistoryCursor>,
    newer: Vec<WorkHistoryPosition>,
    pending: Option<WorkHistoryRequest>,
    loading: bool,
    awaiting_board: bool,
    issue: Option<String>,
}

impl WorkHistoryState {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn loaded(&self) -> usize {
        self.loaded
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
        if self.loaded == 0 {
            0
        } else {
            self.offset.saturating_add(1)
        }
    }

    #[must_use]
    pub fn last_result(&self) -> u64 {
        self.offset
            .saturating_add(u64::try_from(self.loaded).unwrap_or(u64::MAX))
    }

    #[must_use]
    pub const fn has_newer(&self) -> bool {
        !self.newer.is_empty()
    }

    #[must_use]
    pub const fn has_older(&self) -> bool {
        self.next.is_some()
    }

    #[must_use]
    pub const fn filtered(&self) -> bool {
        self.query.is_some() || self.completed_before.is_some()
    }

    pub(super) const fn active(&self) -> bool {
        self.total.is_some() || self.loading || self.pending.is_some()
    }

    pub(super) fn invalidate(&mut self, awaiting_board: bool) {
        self.revision = self.revision.saturating_add(1);
        self.before = None;
        self.offset = 0;
        self.loaded = 0;
        self.total = None;
        self.next = None;
        self.newer.clear();
        self.pending = None;
        self.loading = false;
        self.awaiting_board = awaiting_board;
        self.issue = None;
    }
}

impl AppRuntime {
    /// Paging state for terminal Work rows retained in the current board.
    #[must_use]
    pub const fn work_history(&self) -> &WorkHistoryState {
        &self.work_history
    }

    /// Reserve the newest history page once an open-board snapshot is present.
    pub fn begin_work_history(&mut self) -> Option<WorkHistoryRequest> {
        if self.work_history.total.is_some() {
            return None;
        }
        self.reserve_work_history(
            WorkHistoryPosition::default(),
            WorkHistoryDirection::Current,
        )
    }

    /// Apply a new finder query and return to its newest result page.
    pub fn apply_work_history_filters(
        &mut self,
        query: String,
        completed_before: Option<wire::Instant>,
    ) -> Option<WorkHistoryRequest> {
        self.work_history.query = match query.trim() {
            "" => None,
            query => Some(query.to_owned()),
        };
        self.work_history.completed_before = completed_before;
        self.evict_work_history(false);
        self.begin_work_history()
    }

    /// Reserve the next older page advertised by the current filtered result.
    pub fn begin_older_work_history(&mut self) -> Option<WorkHistoryRequest> {
        let before = self.work_history.next.clone()?;
        let target = WorkHistoryPosition {
            before: Some(before),
            offset: self.work_history.last_result(),
        };
        self.reserve_work_history(target, WorkHistoryDirection::Older)
    }

    /// Reserve the page immediately newer than the current filtered result.
    pub fn begin_newer_work_history(&mut self) -> Option<WorkHistoryRequest> {
        let target = self.work_history.newer.last()?.clone();
        self.reserve_work_history(target, WorkHistoryDirection::Newer)
    }

    /// Retry the exact request that was refused or could not be queued.
    pub fn retry_work_history(&mut self) -> Option<WorkHistoryRequest> {
        if self.board().is_none() || self.work_history.awaiting_board || self.work_history.loading {
            return None;
        }
        let request = self.work_history.pending.clone()?;
        self.work_history.loading = true;
        self.work_history.issue = None;
        Some(request)
    }

    /// Restore one request to retryable state when browser admission fails.
    pub fn refuse_work_history_request(&mut self, request: &WorkHistoryRequest, issue: String) {
        if self.work_history.pending.as_ref() == Some(request) {
            self.work_history.loading = false;
            self.work_history.issue = Some(issue);
        }
    }

    fn reserve_work_history(
        &mut self,
        target: WorkHistoryPosition,
        direction: WorkHistoryDirection,
    ) -> Option<WorkHistoryRequest> {
        if self.board().is_none() || self.work_history.awaiting_board || self.work_history.loading {
            return None;
        }
        let request = WorkHistoryRequest {
            revision: self.work_history.revision,
            before: target.before,
            query: self.work_history.query.clone(),
            completed_before: self.work_history.completed_before,
            offset: target.offset,
            direction,
        };
        self.work_history.loading = true;
        self.work_history.issue = None;
        self.work_history.pending = Some(request.clone());
        Some(request)
    }

    pub(super) fn evict_work_history(&mut self, awaiting_board: bool) {
        self.work_history.invalidate(awaiting_board);
        if let Some(board) = self.board.value.as_mut() {
            board.tasks.retain(|task| {
                task.workflow.body.status == "pending" && task.workflow.body.completion.is_none()
            });
        }
    }

    /// Invalidate terminal rows until the corresponding open-board refresh lands.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn invalidate_work_history(&mut self) {
        if self.work_history.active() {
            self.evict_work_history(true);
        }
    }

    /// Replace the visible terminal page with one exact correlated result.
    pub fn apply_work_history(
        &mut self,
        request: &WorkHistoryRequest,
        admitted: &Admitted<Response<BoardAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope()
            || request.revision != self.work_history.revision
            || self.work_history.pending.as_ref() != Some(request)
        {
            return;
        }
        self.work_history.loading = false;
        let answer = match &admitted.get().outcome {
            Outcome::Reply(reply) => &reply.body,
            Outcome::Refusal(refusal) => {
                let diagnostic = refusal.diagnostic();
                self.work_history.issue = Some(
                    diagnostic
                        .map_or("board::history-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                );
                return;
            }
            Outcome::Fault(fault) => {
                self.work_history.issue = Some(fault.reason.clone());
                return;
            }
        };
        let Some(page) = answer.history.as_ref() else {
            self.work_history.issue = Some("board::history-metadata-missing".to_owned());
            return;
        };
        let result_end = request
            .offset
            .saturating_add(u64::try_from(answer.tasks.len()).unwrap_or(u64::MAX));
        let target_valid = match request.direction {
            WorkHistoryDirection::Current => request.before.is_none() && request.offset == 0,
            WorkHistoryDirection::Older => {
                request.before == self.work_history.next
                    && request.offset == self.work_history.last_result()
            }
            WorkHistoryDirection::Newer => self.work_history.newer.last().is_some_and(|target| {
                target.before == request.before && target.offset == request.offset
            }),
        };
        if !target_valid
            || request.query != self.work_history.query
            || request.completed_before != self.work_history.completed_before
            || page.before != request.before
            || answer.tasks.len() > usize::from(page.limit)
            || result_end > page.total
            || answer.tasks.iter().any(|task| {
                task.workflow.body.status == "pending" && task.workflow.body.completion.is_none()
            })
        {
            self.work_history.issue = Some("board::history-answer-invalid".to_owned());
            return;
        }
        let Some(board) = self.board.value.as_mut() else {
            self.work_history.issue = Some("board::history-open-board-missing".to_owned());
            return;
        };
        board.tasks.retain(|task| {
            task.workflow.body.status == "pending" && task.workflow.body.completion.is_none()
        });
        board.tasks.extend(answer.tasks.iter().cloned());
        match request.direction {
            WorkHistoryDirection::Current => self.work_history.newer.clear(),
            WorkHistoryDirection::Older => {
                self.work_history.newer.push(WorkHistoryPosition {
                    before: self.work_history.before.clone(),
                    offset: self.work_history.offset,
                });
            }
            WorkHistoryDirection::Newer => {
                self.work_history.newer.pop();
            }
        }
        self.work_history.before.clone_from(&request.before);
        self.work_history.offset = request.offset;
        self.work_history.loaded = answer.tasks.len();
        self.work_history.total = Some(page.total);
        self.work_history.next.clone_from(&page.next);
        self.work_history.pending = None;
        self.work_history.issue = None;
    }
}
