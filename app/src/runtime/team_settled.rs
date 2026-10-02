//! Pages of work one app team recently settled, read on demand.
//!
//! Each team keeps its own pages apart from the open board and from the
//! global Work archive, so neither can evict the other. An open-board refresh
//! leaves the pages alone unless a task it last showed open is no longer open:
//! only then can the team's settled work have changed.

use std::collections::BTreeSet;

use wire::intent::{BoardAnswer, BoardTask, TaskHistoryCursor};
use wire::{CanonicalUuid, CommandKind, Instant, Outcome, Response};

use super::{AppRuntime, BoardView};
use crate::socket::Admitted;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TeamSettledDirection {
    Current,
    Older,
    Newer,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TeamSettledPosition {
    before: Option<TaskHistoryCursor>,
    offset: u64,
}

/// One exact request for a page of one team's settled work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeamSettledRequest {
    pub team: CanonicalUuid,
    /// Settled-work generation the request was reserved under.
    pub revision: u64,
    /// Keyset position of an older page; absent reads the newest page.
    pub before: Option<TaskHistoryCursor>,
    offset: u64,
    direction: TeamSettledDirection,
    /// Case-insensitive text filter applied by the board authority.
    pub query: Option<String>,
    /// Inclusive UTC completion-day boundary applied by the board authority.
    pub completed_before: Option<Instant>,
}

/// One team's visible settled page and the positions leading back to newest.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TeamSettledState {
    pages: Option<BoardView>,
    before: Option<TaskHistoryCursor>,
    offset: u64,
    total: Option<u64>,
    next: Option<TaskHistoryCursor>,
    newer: Vec<TeamSettledPosition>,
    query: Option<String>,
    completed_before: Option<Instant>,
    pending: Option<TeamSettledRequest>,
    loading: bool,
    stale: bool,
    issue: Option<String>,
}

impl TeamSettledState {
    /// The currently visible settled page.
    #[must_use]
    pub const fn pages(&self) -> Option<&BoardView> {
        self.pages.as_ref()
    }

    /// Settled tasks matching the team's active finder, loaded or not.
    #[must_use]
    pub const fn total(&self) -> Option<u64> {
        self.total
    }

    /// Whether this team page is narrowed by either finder control.
    #[must_use]
    pub const fn filtered(&self) -> bool {
        self.query.is_some() || self.completed_before.is_some()
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
    pub fn first_result(&self) -> u64 {
        if self.pages.as_ref().is_none_or(|page| page.tasks.is_empty()) {
            0
        } else {
            self.offset.saturating_add(1)
        }
    }

    #[must_use]
    pub fn last_result(&self) -> u64 {
        self.offset
            .saturating_add(self.pages.as_ref().map_or(0, |page| {
                u64::try_from(page.tasks.len()).unwrap_or(u64::MAX)
            }))
    }

    #[must_use]
    pub fn has_newer(&self) -> bool {
        !self.newer.is_empty()
    }

    /// Whether an older page remains behind the visible one.
    #[must_use]
    pub const fn has_older(&self) -> bool {
        self.next.is_some()
    }
}

fn open(task: &BoardTask) -> bool {
    task.workflow.body.status == "pending" && task.workflow.body.completion.is_none()
}

impl AppRuntime {
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

    /// Reserve the newest settled page unless a current page is loaded.
    ///
    /// A refused read waits for an explicit retry rather than repeating.
    pub fn begin_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.entry(team).or_default();
        let current = state.pages.is_some() || state.issue.is_some();
        if state.loading || (current && !state.stale) {
            return None;
        }
        self.reserve_team_settled(
            team,
            TeamSettledPosition::default(),
            TeamSettledDirection::Current,
        )
    }

    /// Replace one team's finder criteria and reserve its newest matching page.
    pub fn filter_team_settled(
        &mut self,
        team: CanonicalUuid,
        query: String,
        completed_before: Option<Instant>,
    ) -> Option<TeamSettledRequest> {
        let state = self.team_settled.entry(team).or_default();
        let query = query.trim();
        state.query = (!query.is_empty()).then(|| query.to_owned());
        state.completed_before = completed_before;
        state.pages = None;
        state.before = None;
        state.offset = 0;
        state.total = None;
        state.next = None;
        state.newer.clear();
        state.pending = None;
        state.loading = false;
        state.stale = false;
        state.issue = None;
        self.begin_team_settled(team)
    }

    /// Reserve the page behind the visible settled page.
    pub fn begin_older_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.get(&team)?;
        if state.loading || state.stale {
            return None;
        }
        let target = TeamSettledPosition {
            before: Some(state.next.clone()?),
            offset: state.last_result(),
        };
        self.reserve_team_settled(team, target, TeamSettledDirection::Older)
    }

    /// Reserve the page immediately newer than the visible settled page.
    pub fn begin_newer_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.get(&team)?;
        if state.loading || state.stale {
            return None;
        }
        let target = state.newer.last()?.clone();
        self.reserve_team_settled(team, target, TeamSettledDirection::Newer)
    }

    /// Read the refused or unqueued page again under the current generation.
    pub fn retry_team_settled(&mut self, team: CanonicalUuid) -> Option<TeamSettledRequest> {
        let state = self.team_settled.get(&team)?;
        if state.loading || state.issue.is_none() || state.stale {
            return None;
        }
        let request = state.pending.as_ref()?;
        let target = TeamSettledPosition {
            before: request.before.clone(),
            offset: request.offset,
        };
        self.reserve_team_settled(team, target, request.direction)
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
        target: TeamSettledPosition,
        direction: TeamSettledDirection,
    ) -> Option<TeamSettledRequest> {
        if !self.offers(CommandKind::BoardRead) {
            return None;
        }
        let revision = self.team_settled_revision;
        let state = self.team_settled.entry(team).or_default();
        let request = TeamSettledRequest {
            team,
            revision,
            before: target.before,
            offset: target.offset,
            direction,
            query: state.query.clone(),
            completed_before: state.completed_before,
        };
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

    /// Apply one correlated settled page, replacing the visible page.
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
        if request.revision != self.team_settled_revision || state.pending.as_ref() != Some(request)
        {
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
        let result_end = request
            .offset
            .saturating_add(u64::try_from(answer.tasks.len()).unwrap_or(u64::MAX));
        let target_valid = match request.direction {
            TeamSettledDirection::Current => request.before.is_none() && request.offset == 0,
            TeamSettledDirection::Older => {
                request.before == state.next && request.offset == state.last_result()
            }
            TeamSettledDirection::Newer => state.newer.last().is_some_and(|target| {
                target.before == request.before && target.offset == request.offset
            }),
        };
        if !target_valid
            || page.before != request.before
            || answer.tasks.len() > usize::from(page.limit)
            || result_end > page.total
            || answer.tasks.iter().any(open)
        {
            state.issue = Some("board::history-answer-invalid".to_owned());
            return;
        }
        match request.direction {
            TeamSettledDirection::Current => state.newer.clear(),
            TeamSettledDirection::Older => state.newer.push(TeamSettledPosition {
                before: state.before.clone(),
                offset: state.offset,
            }),
            TeamSettledDirection::Newer => {
                state.newer.pop();
            }
        }
        state.pages = Some(BoardView::from(answer));
        state.before.clone_from(&request.before);
        state.offset = request.offset;
        state.total = Some(page.total);
        state.next.clone_from(&page.next);
        state.pending = None;
        state.stale = false;
        state.issue = None;
    }
}
