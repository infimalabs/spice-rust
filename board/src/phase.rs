use std::fmt;

use wire::{Instant, Phase, TaskHandle, WorkflowHead};

use crate::claim::Active;
use crate::task::Entry;

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
        if workflow.status == "completed" || workflow.completion.is_some() {
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
        if workflow.status == "completed" || workflow.completion.is_some() {
            return Err(PhaseRefusal::Completed);
        }
        if workflow.flow.last() != Some(&workflow.phase) {
            return Err(PhaseRefusal::NotFinal(workflow.phase.clone()));
        }

        let mut next = workflow.clone();
        next.status = "completed".to_owned();
        next.completion = Some(at);
        next.assignment = None;
        Ok(Self {
            task: entry.handle().clone(),
            kind: TransitionKind::Complete {
                phase: workflow.phase.clone(),
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
/// answers `None` rather than guessing at what it wants. The three declared
/// stations differ in what they may touch, which is the whole reason a lane
/// must read its phase before it acts: plan moves board state, todo moves
/// repository files, and review returns a verdict on work already integrated.
#[must_use]
pub fn deliverable(phase: &Phase) -> Option<&'static str> {
    match phase.as_str() {
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

/// The review deliverable when the allocator hands an actor its own review.
///
/// Self-review is ranked last, never excluded, so a quiet board hands it out
/// rather than stalling.
pub const SELF_REVIEW_DELIVERABLE: &str = "a verdict is the deliverable: the allocator \
     assigned your own review as the last resort because no peer work is eligible, so this \
     assignment authorizes and requires you to review it now; do not free or defer it merely \
     because you authored the implementation. Inspect the integrated diff, check its evidence, \
     and record a clean or finding review";

/// What a lane is told when it wins back a row it freed.
///
/// A freed row ranks last rather than being excluded, so it returns once the
/// lane has nothing else. Freeing it again would only repeat the loop; the
/// lasting answer is to state what the row waits on.
#[must_use]
pub fn released_reminder(task: &wire::TaskHandle) -> String {
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
