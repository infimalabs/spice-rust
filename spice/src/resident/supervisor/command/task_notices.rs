//! Private task-claim notices for one exact occupant: a stale claim
//! generation, a resident-minted assignment and its withdrawal once the claim
//! ends, and a lapse. Each is a keyless background aside under the task's
//! purpose, so a later notice for the same task replaces an earlier one still
//! queued at omp's next step boundary.

use std::io::Write;
use std::sync::PoisonError;

use serde::{Deserialize, Serialize};
use wire::CanonicalUuid;

use super::super::super::RuntimePaths;
use super::{Driver, Kick, send};

/// One private task-authority reminder, scoped to the claim's exact occupant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct TaskGenerationNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: String,
    pub(in crate::resident) claim: u64,
    pub(in crate::resident) generation: u64,
    pub(in crate::resident) change: String,
    pub(in crate::resident) editor: String,
    pub(in crate::resident) position: u64,
}

/// One private proactive-assignment notice, scoped to the claim's exact occupant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct TaskAssignmentNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: String,
    pub(in crate::resident) claim: u64,
    pub(in crate::resident) phase: String,
}

/// One ended claim, scoped to the exact occupant that held it when it ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct TaskAssignmentEnd {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: String,
    pub(in crate::resident) claim: u64,
}

impl TaskAssignmentNotice {
    fn text(&self) -> String {
        format!(
            "<system-notice>\nTask {} was assigned to this lane in phase {}. Continue \
             this task now; run `spice task show {} --full` for its contract before asking \
             the allocator for work. This is a private assignment notice; no \
             acknowledgment key or visible ACK/NAK is required.\n</system-notice>",
            self.task, self.phase, self.task,
        )
    }
}

/// One private lapse notice for the lane that held the expired claim, scoped to
/// that lane's current occupant: a restart may have re-seated the lane since
/// the claim was won.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct ClaimLapseNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) task: String,
    pub(in crate::resident) expiry: wire::Instant,
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Replace a stale-claim reminder without interrupting current work.
    pub(super) fn deliver_task_generation(&self, notice: &TaskGenerationNotice) {
        let previous = {
            let mut delivered = self
                .task_generation
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref().is_some_and(|(task, claim, generation)| {
                task == &notice.task && *claim == notice.claim && *generation >= notice.generation
            }) {
                return;
            }
            delivered.replace((notice.task.clone(), notice.claim, notice.generation))
        };
        let message = format!(
            "<system-notice>\nTask {} changed after this claim last acknowledged it: \
             generation {} is {} by {} at position {}. Run `spice task next` \
             to read and accept the current statement before continuing. Completion and review remain \
             refused until task next succeeds. This is a private authority reminder; \
             no acknowledgment key or visible ACK/NAK is required.\n</system-notice>",
            notice.task, notice.generation, notice.change, notice.editor, notice.position,
        );
        if self
            .background(&format!("task:{}", notice.task), &message)
            .is_err()
        {
            let mut delivered = self
                .task_generation
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref().is_some_and(|(task, claim, generation)| {
                task == &notice.task && *claim == notice.claim && *generation == notice.generation
            }) {
                *delivered = previous;
            }
        }
    }

    /// Tell this exact occupant about one resident-minted assignment.
    pub(super) fn deliver_task_assignment(&self, notice: &TaskAssignmentNotice) {
        let previous = {
            let mut delivered = self
                .task_assignment
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered
                .as_ref()
                .is_some_and(|told| told.task == notice.task && told.claim == notice.claim)
            {
                return;
            }
            delivered.replace(notice.clone())
        };
        if self
            .background(&format!("task:{}", notice.task), &notice.text())
            .is_err()
        {
            let mut delivered = self
                .task_assignment
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref() == Some(notice) {
                *delivered = previous;
            }
        }
    }

    /// Withdraw this occupant's assignment notice for a claim that has ended,
    /// if omp still queues it. The notice stays recorded as delivered, so a
    /// later observation of the same claim never sends it again; any other
    /// notice queued under the task's purpose is left as it stands.
    pub(super) fn withdraw_task_assignment(&self, ended: &TaskAssignmentEnd) {
        let told = self
            .task_assignment
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(told) = told.filter(|told| told.task == ended.task && told.claim == ended.claim)
        else {
            return;
        };
        let text = told.text();
        let _ = self.update_background(&format!("task:{}", told.task), |queued| {
            if queued == text {
                String::new()
            } else {
                queued.to_owned()
            }
        });
    }

    /// Tell this occupant that a claim its lane held lapsed, once per lapse.
    pub(super) fn deliver_claim_lapse(&self, notice: &ClaimLapseNotice) {
        let previous = {
            let mut delivered = self
                .claim_lapse
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref() == Some(notice) {
                return;
            }
            delivered.replace(notice.clone())
        };
        let message = format!(
            "<system-notice>\nThe claim this lane held on task {} lapsed: its lease expired at \
             {} and the resident released it. This lane no longer holds that task; stop \
             working on it. Commits made for it and not yet published are loose and cannot \
             complete it; run `spice task show {}` for its current holder before asking the \
             allocator for work. This is a private lapse notice; no acknowledgment key or \
             visible ACK/NAK is required.\n</system-notice>",
            notice.task, notice.expiry, notice.task,
        );
        if self
            .background(&format!("task:{}", notice.task), &message)
            .is_err()
        {
            let mut delivered = self
                .claim_lapse
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if delivered.as_ref() == Some(notice) {
                *delivered = previous;
            }
        }
    }
}

/// Privately notify the exact claimant occupant that its task authority moved.
pub(in crate::resident) fn task_generation(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: TaskGenerationNotice,
) {
    send(paths, lane, &Kick::TaskGeneration(notice));
}

/// Privately notify the exact occupant about a resident-minted assignment.
pub(in crate::resident) fn task_assignment(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: TaskAssignmentNotice,
) {
    send(paths, lane, &Kick::TaskAssignment(notice));
}

/// Privately tell the exact occupant that a claim it held has ended.
pub(in crate::resident) fn task_assignment_ended(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    ended: TaskAssignmentEnd,
) {
    send(paths, lane, &Kick::TaskAssignmentEnded(ended));
}

/// Privately tell the lane that held a lapsed claim, at its current occupant.
pub(in crate::resident) fn claim_lapse(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: ClaimLapseNotice,
) {
    send(paths, lane, &Kick::ClaimLapse(notice));
}
