//! Pages of work one Canvas team recently settled, read on demand.
//!
//! Each team keeps its own pages apart from the open board and from the
//! global Work archive, so neither can evict the other. An open-board refresh
//! leaves the pages alone unless a task it last showed open is no longer open:
//! only then can the team's settled work have changed.

use std::collections::BTreeSet;

use wire::intent::{BoardAnswer, BoardTask, TaskHistoryCursor};
use wire::{CanonicalUuid, CommandKind, Outcome, Response};

use super::{BoardView, CanvasRuntime};
use crate::socket::Admitted;

/// One exact request for a page of one team's settled work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamSettledRequest {
    pub team: CanonicalUuid,
    /// Settled-work generation the request was reserved under.
    pub revision: u64,
    /// Keyset position of an older page; absent reads the newest page.
    pub before: Option<TaskHistoryCursor>,
}

/// One team's loaded settled pages, newest first.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TeamSettledState {
    pages: Option<BoardView>,
    total: Option<u64>,
    next: Option<TaskHistoryCursor>,
    pending: Option<TeamSettledRequest>,
    loading: bool,
    stale: bool,
    issue: Option<String>,
}

impl TeamSettledState {
    /// Every settled task loaded so far, as one board the Work projection reads.
    #[must_use]
    pub const fn pages(&self) -> Option<&BoardView> {
        self.pages.as_ref()
    }

    /// Settled tasks the team has in all, loaded or not.
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

    /// Whether an older page remains behind the loaded ones.
    #[must_use]
    pub const fn has_older(&self) -> bool {
        self.next.is_some()
    }
}

fn open(task: &BoardTask) -> bool {
    task.workflow.body.status == "pending" && task.workflow.body.completion.is_none()
}

impl CanvasRuntime {
    /// Settled pages loaded for one team, if it was ever shown.
    #[must_use]
    pub fn team_settled(&self, team: CanonicalUuid) -> Option<&TeamSettledState> {
        self.team_settled.get(&team)
    }

    /// Generation that advances whenever loaded settled pages go stale.
    #[must_use]
    pub const fn team_settled_revision(&self) -> u64 {
        self.team_settled_revision
    }

    /// Reserve the newest settled page unless current pages are loaded.
    ///
    /// A refused read waits for an explicit retry rather than repeating.
    pub fn begin_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.entry(team).or_default();
        let current = state.pages.is_some() || state.issue.is_some();
        if state.loading || (current && !state.stale) {
            return None;
        }
        self.reserve_team_settled(team, None)
    }

    /// Reserve the page behind the oldest loaded settled task.
    pub fn begin_older_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.get(&team)?;
        if state.loading || state.stale {
            return None;
        }
        let before = state.next.clone()?;
        self.reserve_team_settled(team, Some(before))
    }

    /// Read the refused or unqueued page again under the current generation.
    pub fn retry_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.get(&team)?;
        if state.loading || state.issue.is_none() {
            return None;
        }
        let before = state
            .pending
            .as_ref()
            .filter(|_| !state.stale)
            .and_then(|pending| pending.before.clone());
        self.reserve_team_settled(team, before)
    }

    /// Restore one request to retryable state when browser admission fails.
    pub fn refuse_team_settled_request(&mut self, request: &TeamSettledRequest, issue: String) {
        if let Some(state) = self.team_settled.get_mut(&request.team)
            && state.pending.as_ref() == Some(request)
        {
            state.loading = false;
            state.issue = Some(issue);
        }
    }

    fn reserve_team_settled(
        &mut self,
        team: CanonicalUuid,
        before: Option<TaskHistoryCursor>,
    ) -> Option<TeamSettledRequest> {
        if !self.offers(CommandKind::BoardRead) {
            return None;
        }
        let request = TeamSettledRequest {
            team,
            revision: self.team_settled_revision,
            before,
        };
        let state = self.team_settled.entry(team).or_default();
        state.loading = true;
        state.issue = None;
        state.pending = Some(request.clone());
        Some(request)
    }

    /// Mark every team's pages stale when `answer` no longer shows open a task
    /// the previous open board did, abandoning reads already in flight.
    pub(super) fn settle_team_work(&mut self, answer: &BoardAnswer) {
        let Some(previous) = self.board.value.as_ref() else {
            return;
        };
        let current = answer
            .tasks
            .iter()
            .filter(|task| open(task))
            .map(|task| &task.statement.body.born)
            .collect::<BTreeSet<_>>();
        let settled = previous
            .tasks
            .iter()
            .filter(|task| open(task))
            .any(|task| !current.contains(&task.statement.body.born));
        if settled {
            self.stale_team_settled();
        }
    }

    /// Re-read every team's pages the next time they are shown, abandoning
    /// reads already in flight.
    pub(super) fn stale_team_settled(&mut self) {
        self.team_settled_revision = self.team_settled_revision.saturating_add(1);
        for state in self.team_settled.values_mut() {
            state.stale = true;
            state.pending = None;
            state.loading = false;
        }
    }

    /// Apply one correlated settled page: the newest replaces every loaded
    /// page, an older one extends them.
    pub fn apply_team_settled(
        &mut self,
        request: &TeamSettledRequest,
        admitted: &Admitted<Response<BoardAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let Some(state) = self.team_settled.get_mut(&request.team) else {
            return;
        };
        if state.pending.as_ref() != Some(request) {
            return;
        }
        state.loading = false;
        let answer = match &admitted.get().outcome {
            Outcome::Reply(reply) => &reply.body,
            Outcome::Refusal(refusal) => {
                state.issue = Some(
                    refusal
                        .diagnostic()
                        .map_or("board::history-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                );
                return;
            }
            Outcome::Fault(fault) => {
                state.issue = Some(fault.reason.clone());
                return;
            }
        };
        let Some(page) = answer.history.as_ref() else {
            state.issue = Some("board::history-metadata-missing".to_owned());
            return;
        };
        if page.before != request.before
            || answer.tasks.len() > usize::from(page.limit)
            || answer.tasks.iter().any(open)
        {
            state.issue = Some("board::history-answer-invalid".to_owned());
            return;
        }
        match (&request.before, state.pages.as_mut()) {
            (Some(_), Some(pages)) => pages.tasks.extend(answer.tasks.iter().cloned()),
            _ => state.pages = Some(BoardView::from(answer)),
        }
        state.total = Some(page.total);
        state.next.clone_from(&page.next);
        state.pending = None;
        state.stale = false;
        state.issue = None;
    }
}
