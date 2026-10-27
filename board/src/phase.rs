use std::fmt;

use wire::{Instant, Phase, TaskHandle, WorkflowHead};

use crate::claim::Active;
use crate::task::Entry;

/// Exclusive ownership opportunity after one phase advances.
///
/// This is the same short settling interval automatic lifecycle uses before it
/// seats newly ready work. Task birth keeps its existing scheduler treatment;
/// only transition ownership consults this bound.
pub const HANDOFF_SECONDS: i64 = 3;

/// How long allocation holds a nudged Design for the lane its nudge messaged,
/// long enough for a vacant lane to wake and capture it.
pub const NUDGE_HOLD_SECONDS: i64 = 120;

/// One guarded workflow change ready for the journal transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    task: TaskHandle,
    kind: TransitionKind,
    next: WorkflowHead,
}

impl Transition {
    /// Guard an advance to the immediately following declared phase.
    pub fn advance(
        entry: &Entry,
        active: &Active<'_>,
        target: Phase,
    ) -> Result<Self, PhaseRefusal> {
        bind_claim(entry, active)?;
        let workflow = entry.workflow();
        if !workflow.status.is_pending() {
            return Err(PhaseRefusal::Completed);
        }

        let from = workflow
            .flow
            .iter()
            .position(|phase| phase == &workflow.phase)
            .expect("Entry admits only a current phase inside its flow");
        let to = workflow
            .flow
            .iter()
            .position(|phase| phase == &target)
            .ok_or_else(|| PhaseRefusal::TargetOutsideFlow(target.clone()))?;
        if to != from + 1 {
            return Err(PhaseRefusal::Illegal {
                from: workflow.phase.clone(),
                to: target,
            });
        }

        let mut next = workflow.clone();
        let previous = next.phase.clone();
        next.phase = target.clone();
        Ok(Self {
            task: entry.handle().clone(),
            kind: TransitionKind::Advance {
                from: previous,
                to: target,
            },
            next,
        })
    }

    /// Guard completion from the final declared phase.
    ///
    /// A withdrawn task is settled by the same clause that settles a completed
    /// one: withdrawal stamps `completion`, so no separate status check is
    /// needed here and none is reachable anyway. Withdrawal also frees the
    /// claim, and possession is admitted before this runs.
    pub fn complete(entry: &Entry, active: &Active<'_>, at: Instant) -> Result<Self, PhaseRefusal> {
        bind_claim(entry, active)?;
        let workflow = entry.workflow();
        if !workflow.status.is_pending() {
            return Err(PhaseRefusal::Completed);
        }
        if workflow.flow.last() != Some(&workflow.phase) {
            return Err(PhaseRefusal::NotFinal(workflow.phase.clone()));
        }
        Ok(settle(entry, workflow, at))
    }

    /// Guard a Plan's no-construction disposition from its current phase.
    ///
    /// Nothing remains to execute, so the workflow passes every later phase
    /// before its first later review, which still verifies the disposition; a
    /// flow with no later review settles. No execution phase ever holds a Plan
    /// that constructs nothing.
    pub fn dispose(entry: &Entry, active: &Active<'_>, at: Instant) -> Result<Self, PhaseRefusal> {
        bind_claim(entry, active)?;
        let workflow = entry.workflow();
        if !workflow.status.is_pending() {
            return Err(PhaseRefusal::Completed);
        }
        let from = workflow
            .flow
            .iter()
            .position(|phase| phase == &workflow.phase)
            .expect("Entry admits only a current phase inside its flow");
        let Some(review) = workflow.flow[from + 1..]
            .iter()
            .find(|phase| phase.as_str() == "review")
        else {
            return Ok(settle(entry, workflow, at));
        };
        let mut next = workflow.clone();
        next.phase = review.clone();
        Ok(Self {
            task: entry.handle().clone(),
            kind: TransitionKind::Advance {
                from: workflow.phase.clone(),
                to: review.clone(),
            },
            next,
        })
    }

    /// Task whose workflow head changes.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }

    /// Kind of guarded change.
    #[must_use]
    pub const fn kind(&self) -> &TransitionKind {
        &self.kind
    }

    /// Replacement head to commit with the transition fact.
    #[must_use]
    pub const fn next(&self) -> &WorkflowHead {
        &self.next
    }
}

/// Closed workflow transitions board can propose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionKind {
    Advance { from: Phase, to: Phase },
    Complete { phase: Phase },
}

/// Why no workflow transition was produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhaseRefusal {
    ClaimForAnotherTask,
    TargetOutsideFlow(Phase),
    Illegal { from: Phase, to: Phase },
    NotFinal(Phase),
    Completed,
}

/// The one deliverable a declared phase authorizes.
///
/// A flow is an open vocabulary, so a phase this repository does not spell
/// answers `None` rather than guessing at what it wants. The declared
/// stations differ in what they may touch, which is the whole reason a lane
/// must read its phase before it acts: design and plan move board state, todo
/// moves repository files, and review returns a verdict on work already
/// integrated.
#[must_use]
pub fn deliverable(phase: &Phase) -> Option<&'static str> {
    match phase.as_str() {
        "design" => Some(
            "board state is the deliverable: record exactly one `preparation propose` \
             recommendation justified by the evidence; that command releases the claim \
             atomically; only the operator concludes a Design, and lanes never mark it done",
        ),
        "plan" => Some(
            "board state is the deliverable: make the execution contract explicit or \
             decompose it into accepted children; do not edit or commit repository files",
        ),
        "todo" => Some(
            "repository work is the deliverable: implement the acceptance this task \
             carries, prove it, and commit while holding this claim",
        ),
        "review" => Some(
            "a verdict is the deliverable: inspect the integrated diff, check its \
             evidence, and record a clean or finding review",
        ),
        _ => None,
    }
}

/// Deliverable override when an independent phase returns to its prior author.
///
/// Independent handoff is ranked last, never excluded, so a quiet board can
/// still progress without weakening the preference for another owner.
pub const SELF_HANDOFF_DELIVERABLE: &str = "the current phase deliverable remains authoritative: \
     the allocator assigned this independent phase back to its prior author as the last resort \
     because no peer work is eligible. This assignment authorizes and requires you to complete \
     the phase now; do not free or defer it merely because you authored the prior phase";

/// What a lane is told when it wins back a row it freed.
///
/// A freed row ranks last rather than being excluded, so it returns once the
/// lane has nothing else. Freeing it again would only repeat the loop; the
/// lasting answer is to state what the row waits on. An open Design never
/// waits that way: its pass cycle already says why it returned, so a pass
/// that ended in a proposal points at the decision and input since, and an
/// abandoned pass is finished with one proposal.
#[must_use]
pub fn released_reminder(entry: &Entry) -> String {
    let task = entry.handle();
    if wire::intent::is_open_design(entry.statement().intent, entry.workflow()) {
        return match entry.design().end {
            Some(wire::rooms::DesignPassEnd::Proposed) => format!(
                "your earlier pass of {task} ended in a proposal; this pass answers the \
                 operator's decision and the input recorded since, read through \
                 `spice preparation show {task}`, and ends with one `spice preparation propose`"
            ),
            Some(wire::rooms::DesignPassEnd::Abandoned) | None => format!(
                "your earlier pass of {task} ended without a proposal; finish this pass with \
                 one `spice preparation propose`, recommending conclude or collapse when no \
                 further work is warranted"
            ),
        };
    }
    format!(
        "you freed {task} earlier; if it still cannot proceed, record why instead of \
         freeing it again: \
         `spice task depends {task} AFTER...` for a prerequisite task, \
         `spice task require {task} --journal-schema SCHEMA | --trust-grant DIGEST | \
         --appliance-after VERSION` for the environment, or `spice task defer {task} UNTIL`"
    )
}

fn bind_claim(entry: &Entry, active: &Active<'_>) -> Result<(), PhaseRefusal> {
    if active.handle() == entry.handle() {
        Ok(())
    } else {
        Err(PhaseRefusal::ClaimForAnotherTask)
    }
}

/// Settle a pending workflow positively from its current phase, carrying its
/// live review state into the completion.
fn settle(entry: &Entry, workflow: &WorkflowHead, at: Instant) -> Transition {
    let review = match workflow.status.review() {
        Some(wire::WorkflowReview::Clean) => wire::WorkflowCompletionReview::Clean,
        Some(wire::WorkflowReview::Unassigned | wire::WorkflowReview::Assigned { .. }) => {
            wire::WorkflowCompletionReview::Unassigned
        }
        None => unreachable!("pending workflow has a live review"),
    };
    let mut next = workflow.clone();
    next.status = wire::WorkflowStatus::Settled {
        at,
        conclusion: wire::WorkflowConclusion::Positive { review },
    };
    Transition {
        task: entry.handle().clone(),
        kind: TransitionKind::Complete {
            phase: workflow.phase.clone(),
        },
        next,
    }
}

impl fmt::Display for PhaseRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClaimForAnotherTask => {
                formatter.write_str("active claim belongs to another task")
            }
            Self::TargetOutsideFlow(phase) => {
                write!(formatter, "target phase {phase} is outside the flow")
            }
            Self::Illegal { from, to } => {
                write!(formatter, "workflow cannot transition from {from} to {to}")
            }
            Self::NotFinal(phase) => write!(formatter, "phase {phase} is not the final station"),
            Self::Completed => formatter.write_str("workflow is already completed"),
        }
    }
}

impl std::error::Error for PhaseRefusal {}
