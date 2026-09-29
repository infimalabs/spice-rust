#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use wire::intent::TaskTimeline;
#[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
use wire::intent::TaskTimelineAnswer;
use wire::intent::{TASK_TIMELINE_LANE_LIMIT, TaskTimelineEntry};
use wire::{CanonicalUuid, Seq};
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use wire::{Outcome, Response};

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use crate::socket::Admitted;

/// Canonical member set whose workflow history one team Dash renders.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskTimelineKey {
    lanes: Vec<CanonicalUuid>,
}

impl TaskTimelineKey {
    #[must_use]
    pub fn new(lanes: impl IntoIterator<Item = CanonicalUuid>) -> Option<Self> {
        let mut lanes = lanes.into_iter().collect::<Vec<_>>();
        lanes.sort_unstable();
        lanes.dedup();
        (!lanes.is_empty() && lanes.len() <= TASK_TIMELINE_LANE_LIMIT).then_some(Self { lanes })
    }

    #[must_use]
    pub fn lanes(&self) -> &[CanonicalUuid] {
        &self.lanes
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    fn intent(&self, before: Option<Seq>) -> TaskTimeline {
        TaskTimeline {
            lanes: self.lanes.clone(),
            before,
            limit: TASK_TIMELINE_PAGE_LIMIT,
        }
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
const TASK_TIMELINE_PAGE_LIMIT: u16 = 50;

#[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
/// One correlated newest or older-page request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskTimelineRequest {
    key: TaskTimelineKey,
    before: Option<Seq>,
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
impl TaskTimelineRequest {
    #[must_use]
    pub(crate) fn intent(&self) -> TaskTimeline {
        self.key.intent(self.before)
    }
}

/// Retained team workflow chronology, including every page the reader reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTimelineState {
    entries: Vec<TaskTimelineEntry>,
    next: Option<Seq>,
    head_stale: bool,
    loading_head: bool,
    loading_older: bool,
    issue: Option<String>,
}

impl Default for TaskTimelineState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            next: None,
            head_stale: true,
            loading_head: false,
            loading_older: false,
            issue: None,
        }
    }
}

impl TaskTimelineState {
    #[must_use]
    pub fn entries(&self) -> &[TaskTimelineEntry] {
        &self.entries
    }

    #[must_use]
    pub const fn next(&self) -> Option<Seq> {
        self.next
    }

    #[must_use]
    pub const fn loading(&self) -> bool {
        self.loading_head || self.loading_older
    }

    #[must_use]
    pub const fn loading_older(&self) -> bool {
        self.loading_older
    }

    #[must_use]
    pub fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }

    #[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
    fn reserve_head(&mut self, key: TaskTimelineKey) -> Option<TaskTimelineRequest> {
        if !self.head_stale || self.loading_head {
            return None;
        }
        self.loading_head = true;
        self.issue = None;
        Some(TaskTimelineRequest { key, before: None })
    }

    #[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
    fn reserve_older(&mut self, key: TaskTimelineKey) -> Option<TaskTimelineRequest> {
        let before = self.next?;
        if self.loading_older {
            return None;
        }
        self.loading_older = true;
        self.issue = None;
        Some(TaskTimelineRequest {
            key,
            before: Some(before),
        })
    }

    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    fn fail(&mut self, request: &TaskTimelineRequest, issue: String) {
        if request.before.is_some() {
            self.loading_older = false;
        } else {
            self.loading_head = false;
            self.head_stale = false;
        }
        self.issue = Some(issue);
    }

    #[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
    fn apply(&mut self, request: &TaskTimelineRequest, answer: TaskTimelineAnswer) {
        let retained = !self.entries.is_empty();
        let retained_next = self.next;
        self.entries.reserve(answer.entries.len());
        for entry in answer.entries {
            match self
                .entries
                .binary_search_by(|probe| probe.position.cmp(&entry.position).reverse())
            {
                Ok(index) => self.entries[index] = entry,
                Err(index) => self.entries.insert(index, entry),
            }
        }
        if request.before.is_some() {
            self.next = answer.next;
            self.loading_older = false;
        } else {
            // A refreshed head may push rows out of its fixed-size page. Keep
            // the old frontier so those retained rows continue to bridge the
            // refreshed head to the first already-loaded older page.
            self.next = retained.then_some(retained_next).flatten().or(answer.next);
            self.loading_head = false;
            self.head_stale = false;
        }
        self.issue = None;
    }

    fn invalidate_head(&mut self) {
        self.head_stale = true;
    }
}
impl super::CanvasRuntime {
    /// One team's retained newest-first workflow chronology.
    #[must_use]
    pub fn task_timeline(&self, key: &TaskTimelineKey) -> Option<&TaskTimelineState> {
        self.task_timelines.get(key)
    }

    /// Change token that re-arms mounted team Dash demand reads.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) const fn task_timeline_revision(&self) -> u64 {
        self.task_timeline_revision
    }

    /// Reserve the newest page when this member set has no current head.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn reserve_task_timeline_head(
        &mut self,
        key: TaskTimelineKey,
    ) -> Option<TaskTimelineRequest> {
        reserve_head(&mut self.task_timelines, key)
    }

    /// Reserve the next exclusive older-page cursor for one team.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn reserve_task_timeline_older(
        &mut self,
        key: TaskTimelineKey,
    ) -> Option<TaskTimelineRequest> {
        reserve_older(&mut self.task_timelines, key)
    }

    /// Apply one correlated timeline page without crossing capability scope.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn apply_task_timeline(
        &mut self,
        request: &TaskTimelineRequest,
        admitted: &Admitted<Response<TaskTimelineAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let answer = match &admitted.get().outcome {
            Outcome::Reply(reply) => Ok(reply.body.clone()),
            Outcome::Refusal(refusal) => {
                let diagnostic = refusal.diagnostic();
                Err(diagnostic
                    .map_or("task::timeline-refused", wire::RefusalDiagnostic::reason)
                    .to_owned())
            }
            Outcome::Fault(fault) => Err(format!("{}: {}", fault.reason, fault.detail)),
        };
        apply_answer(&mut self.task_timelines, request, answer);
        self.task_timeline_revision = self.task_timeline_revision.saturating_add(1);
    }

    /// Settle a request the browser could not place on its action queue.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    pub(crate) fn refuse_task_timeline_request(
        &mut self,
        request: &TaskTimelineRequest,
        issue: String,
    ) {
        apply_answer(&mut self.task_timelines, request, Err(issue));
        self.task_timeline_revision = self.task_timeline_revision.saturating_add(1);
    }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(super) fn answer_matches(request: &TaskTimelineRequest, answer: &TaskTimelineAnswer) -> bool {
    answer.lanes == request.key.lanes
        && answer
            .entries
            .windows(2)
            .all(|pair| pair[0].position > pair[1].position)
        && answer.entries.iter().all(|entry| {
            request.key.lanes.binary_search(&entry.lane).is_ok()
                && request.before.is_none_or(|before| entry.position < before)
        })
        && match answer.next {
            Some(next) => answer
                .entries
                .last()
                .is_some_and(|entry| entry.position == next),
            None => true,
        }
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(super) fn reserve_head(
    states: &mut std::collections::BTreeMap<TaskTimelineKey, TaskTimelineState>,
    key: TaskTimelineKey,
) -> Option<TaskTimelineRequest> {
    states.entry(key.clone()).or_default().reserve_head(key)
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(super) fn reserve_older(
    states: &mut std::collections::BTreeMap<TaskTimelineKey, TaskTimelineState>,
    key: TaskTimelineKey,
) -> Option<TaskTimelineRequest> {
    states.get_mut(&key)?.reserve_older(key)
}

pub(super) fn invalidate_all(
    states: &mut std::collections::BTreeMap<TaskTimelineKey, TaskTimelineState>,
) {
    states
        .values_mut()
        .for_each(TaskTimelineState::invalidate_head);
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub(super) fn apply_answer(
    states: &mut std::collections::BTreeMap<TaskTimelineKey, TaskTimelineState>,
    request: &TaskTimelineRequest,
    answer: Result<TaskTimelineAnswer, String>,
) {
    let state = states.entry(request.key.clone()).or_default();
    match answer {
        Ok(answer) if answer_matches(request, &answer) => state.apply(request, answer),
        Ok(_) => state.fail(
            request,
            "the timeline answer did not match its member set, cursor, or ordering".to_owned(),
        ),
        Err(issue) => state.fail(request, issue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(value: u64) -> Seq {
        Seq::new(value).expect("fixture sequence is positive")
    }

    fn entry(lane: CanonicalUuid, position: u64) -> TaskTimelineEntry {
        TaskTimelineEntry {
            position: seq(position),
            at: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            lane,
            task: "1kKRRsDP".parse().expect("fixture task stamp"),
            handle: "SPICE-1kKRRsDP".parse().expect("fixture task handle"),
            title: format!("Transition {position}"),
            event: wire::intent::TaskTimelineEvent::Edited {},
        }
    }

    fn answer(lane: CanonicalUuid, positions: &[u64], next: Option<u64>) -> TaskTimelineAnswer {
        TaskTimelineAnswer {
            snapshot: seq(20),
            lanes: vec![lane],
            entries: positions
                .iter()
                .map(|position| entry(lane, *position))
                .collect(),
            next: next.map(seq),
        }
    }

    #[test]
    fn refreshed_head_keeps_displaced_rows_and_loaded_older_frontier() {
        let lane = "6f5ae476-efa6-471b-9bed-0f6994c3dcfa"
            .parse()
            .expect("fixture lane");
        let key = TaskTimelineKey::new([lane]).expect("one-lane timeline key");
        let mut state = TaskTimelineState::default();

        let head = state
            .reserve_head(key.clone())
            .expect("initial head request");
        state.apply(&head, answer(lane, &[10, 9], Some(9)));
        let older = state
            .reserve_older(key.clone())
            .expect("older page request");
        state.apply(&older, answer(lane, &[8, 7], Some(7)));

        state.invalidate_head();
        let refreshed = state.reserve_head(key).expect("stale head request");
        state.apply(&refreshed, answer(lane, &[12, 11], Some(11)));

        assert_eq!(
            state
                .entries()
                .iter()
                .map(|entry| entry.position)
                .collect::<Vec<_>>(),
            [seq(12), seq(11), seq(10), seq(9), seq(8), seq(7)]
        );
        assert_eq!(state.next(), Some(seq(7)));
    }
}
