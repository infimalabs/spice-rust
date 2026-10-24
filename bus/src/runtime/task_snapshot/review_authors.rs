//! Who completed the phase each finding review judged, and who judged it,
//! advanced once per commit so delivering a finding never walks task history.
//! Completions and reviews do not carry the claim occupant, so authorship
//! follows each task's active claim as claims are won, carried, rebound, and
//! released.

use std::collections::HashMap;

use journal::crossing::body::{ClaimCarried, ClaimLapsed, ClaimRebound, TaskReviewed, Verdict};
use journal::crossing::{Actor, AnyFact, CanonicalUuid, Envelope, FactKind, Seq, TaskStamp};

use super::persistent_index::PersistentIndex;
use super::{Indexes, TaskFacts};

/// One claim's lane and the occupant seated on it when the claim was taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimAuthor {
    pub lane: CanonicalUuid,
    pub occupant: CanonicalUuid,
}

/// Who completed the phase one finding review judged, and who judged it.
/// Fixed once the review is recorded, because history is append-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindingAuthors {
    pub author: ClaimAuthor,
    pub reviewer: ClaimAuthor,
}

/// Authority history that leaves a finding review's authors undefined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorshipFault {
    /// The judged completion had no active claim before it.
    CompletionUnclaimed,
    /// The judged completion's actor is not its active claim's lane.
    CompletionMismatched,
    /// The review had no completed phase before it.
    ReviewUncompleted,
    /// The review had no active reviewer claim before it.
    ReviewUnclaimed,
}

/// Each task's active and completed phase authors, and every finding review's
/// authors keyed by its durable position.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ReviewAuthors {
    /// Each task's active claim author; `None` once that claim is released.
    active: PersistentIndex<TaskStamp, Option<ClaimAuthor>>,
    /// Each task's newest completed phase author, or why it has none.
    completed: PersistentIndex<TaskStamp, Result<ClaimAuthor, AuthorshipFault>>,
    findings: PersistentIndex<Seq, Result<FindingAuthors, AuthorshipFault>>,
}

impl ReviewAuthors {
    fn active(&self, task: &TaskStamp) -> Option<ClaimAuthor> {
        self.active.get(task).copied().flatten()
    }

    /// `occupant` on `lane` now holds `task`'s active claim.
    pub(super) fn claim(&mut self, task: TaskStamp, lane: CanonicalUuid, occupant: CanonicalUuid) {
        self.active = self.active.pushed(HashMap::from([(
            task,
            Some(ClaimAuthor { lane, occupant }),
        )]));
    }

    /// `task`'s active claim ended without completing its phase.
    pub(super) fn release(&mut self, task: &TaskStamp) {
        if self.active(task).is_some() {
            self.active = self.active.pushed(HashMap::from([(task.clone(), None)]));
        }
    }

    /// `actor` completed `task`'s phase: its active claim author becomes the
    /// author any later finding on the task judges.
    pub(super) fn complete(&mut self, actor: &Actor, task: &TaskStamp) {
        let author = match self.active(task) {
            None => Err(AuthorshipFault::CompletionUnclaimed),
            Some(author) if *actor != Actor::Lane(author.lane) => {
                Err(AuthorshipFault::CompletionMismatched)
            }
            Some(author) => Ok(author),
        };
        self.release(task);
        self.completed = self
            .completed
            .pushed(HashMap::from([(task.clone(), author)]));
    }

    /// The review at `seq` ends its reviewer's claim; a finding records the
    /// completed author it judged and that reviewer.
    fn review(&mut self, seq: Seq, task: &TaskStamp, verdict: Verdict) {
        let reviewer = self.active(task);
        self.release(task);
        if verdict != Verdict::Finding {
            return;
        }
        let authors = match (self.completed.get(task).copied(), reviewer) {
            (None, _) => Err(AuthorshipFault::ReviewUncompleted),
            (Some(Err(fault)), _) => Err(fault),
            (Some(Ok(_)), None) => Err(AuthorshipFault::ReviewUnclaimed),
            (Some(Ok(author)), Some(reviewer)) => Ok(FindingAuthors { author, reviewer }),
        };
        self.findings = self.findings.pushed(HashMap::from([(seq, authors)]));
    }
}

impl Indexes {
    /// Advance phase authorship by a fact only it reads: a carried, rebound,
    /// or lapsed claim, or a review. Won and freed claims and completions
    /// reach it from `observe_task_authority`, which decodes them once for
    /// every index.
    pub(super) fn observe_review_authors(&mut self, row: &Envelope<AnyFact>) {
        let authors = &mut self.review_authors;
        match row.fact.kind() {
            FactKind::ClaimCarried => {
                if let Some(carried) = row.fact.read::<ClaimCarried>() {
                    authors.claim(carried.task, carried.lane, carried.occupant);
                }
            }
            FactKind::ClaimRebound => {
                if let Some(rebound) = row.fact.read::<ClaimRebound>() {
                    authors.claim(rebound.task, rebound.lane, rebound.occupant);
                }
            }
            FactKind::ClaimLapsed => {
                if let Some(lapsed) = row.fact.read::<ClaimLapsed>() {
                    authors.release(&lapsed.task);
                }
            }
            FactKind::TaskReviewed => {
                if let Some(reviewed) = row.fact.read::<TaskReviewed>() {
                    authors.review(row.seq, &reviewed.task, reviewed.verdict);
                }
            }
            _ => {}
        }
    }
}

impl TaskFacts {
    /// The authors of the finding review at `review`, or the authority fault
    /// that leaves them undefined; `None` when no finding review is there.
    #[must_use]
    pub fn finding_authors(&self, review: Seq) -> Option<Result<FindingAuthors, AuthorshipFault>> {
        self.indexes.review_authors.findings.get(&review).copied()
    }
}

#[cfg(test)]
mod tests;
