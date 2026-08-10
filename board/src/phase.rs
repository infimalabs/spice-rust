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
