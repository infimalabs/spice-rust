use serde::{Deserialize, Serialize};

use super::is_zero;
use crate::envelope::Actor;
use crate::ident::Instant;
use crate::witness::Phase;

/// Live review state carried by a pending workflow.
///
/// An assignment owns its reviewer. No parallel optional field can disagree
/// with this state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowReview {
    /// No reviewer is assigned.
    Unassigned,
    /// One reviewer is assigned.
    Assigned {
        /// Principal assigned to review.
        reviewer: Actor,
    },
    /// Review completed cleanly before a later phase.
    Clean,
}

impl WorkflowReview {
    /// Assigned reviewer, when this is an assigned review.
    #[must_use]
    pub const fn reviewer(&self) -> Option<&Actor> {
        match self {
            Self::Assigned { reviewer } => Some(reviewer),
            Self::Unassigned | Self::Clean => None,
        }
    }

    /// Stable wire and operator-facing review spelling.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::Assigned { .. } => "assigned",
            Self::Clean => "clean",
        }
    }
}

/// Review state retained by a positive terminal workflow.
///
/// A completed workflow cannot remain assigned, so its narrower review type
/// excludes that live-only state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowCompletionReview {
    /// Completion did not follow a clean review.
    Unassigned,
    /// Completion followed a clean review.
    Clean,
}

impl WorkflowCompletionReview {
    /// Stable wire and operator-facing review spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::Clean => "clean",
        }
    }
}

/// Why a workflow settled without positively completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowNegative {
    /// The operator closed the task or a Design collapsed.
    Closed,
    /// The task was withdrawn.
    Deleted,
}

/// A terminal workflow's positive or negative conclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowConclusion {
    /// The task completed positively.
    Positive {
        /// Review state retained by the canonical wire document.
        review: WorkflowCompletionReview,
    },
    /// The task settled without approving what it controls.
    Negative(WorkflowNegative),
}

/// One task's live or terminal workflow state.
///
/// The live review owns any reviewer. The terminal state owns its instant and
/// conclusion. A completion instant on pending work, a terminal workflow
/// without one, and an assignment outside assigned review are therefore not
/// representable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowStatus {
    /// Work remains live.
    Pending(WorkflowReview),
    /// Work reached a terminal conclusion.
    Settled {
        /// Instant at which the workflow settled.
        at: Instant,
        /// Whether settlement positively completed the task.
        conclusion: WorkflowConclusion,
    },
}

impl WorkflowStatus {
    /// An unassigned live workflow.
    #[must_use]
    pub const fn pending() -> Self {
        Self::Pending(WorkflowReview::Unassigned)
    }

    /// A live workflow assigned to one reviewer.
    #[must_use]
    pub const fn assigned(reviewer: Actor) -> Self {
        Self::Pending(WorkflowReview::Assigned { reviewer })
    }

    /// A positive completion that did not follow clean review.
    #[must_use]
    pub const fn completed(at: Instant) -> Self {
        Self::Settled {
            at,
            conclusion: WorkflowConclusion::Positive {
                review: WorkflowCompletionReview::Unassigned,
            },
        }
    }

    /// A positive completion following clean review.
    #[must_use]
    pub const fn reviewed(at: Instant) -> Self {
        Self::Settled {
            at,
            conclusion: WorkflowConclusion::Positive {
                review: WorkflowCompletionReview::Clean,
            },
        }
    }

    /// A negative operator closure or collapsed Design.
    #[must_use]
    pub const fn closed(at: Instant) -> Self {
        Self::Settled {
            at,
            conclusion: WorkflowConclusion::Negative(WorkflowNegative::Closed),
        }
    }

    /// A withdrawn task.
    #[must_use]
    pub const fn deleted(at: Instant) -> Self {
        Self::Settled {
            at,
            conclusion: WorkflowConclusion::Negative(WorkflowNegative::Deleted),
        }
    }

    /// Whether work remains live.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Whether the canonical workflow status is `completed`.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(
            self,
            Self::Settled {
                conclusion: WorkflowConclusion::Positive { .. }
                    | WorkflowConclusion::Negative(WorkflowNegative::Closed),
                ..
            }
        )
    }

    /// Whether the task was withdrawn.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        matches!(
            self,
            Self::Settled {
                conclusion: WorkflowConclusion::Negative(WorkflowNegative::Deleted),
                ..
            }
        )
    }

    /// Live review state, absent after settlement.
    #[must_use]
    pub const fn review(&self) -> Option<&WorkflowReview> {
        match self {
            Self::Pending(review) => Some(review),
            Self::Settled { .. } => None,
        }
    }

    /// Assigned reviewer, when this is live assigned review.
    #[must_use]
    pub const fn reviewer(&self) -> Option<&Actor> {
        match self.review() {
            Some(review) => review.reviewer(),
            None => None,
        }
    }

    /// Terminal instant, absent while live.
    #[must_use]
    pub const fn completion(&self) -> Option<Instant> {
        match self {
            Self::Pending(_) => None,
            Self::Settled { at, .. } => Some(*at),
        }
    }

    /// Terminal conclusion, absent while live.
    #[must_use]
    pub const fn conclusion(&self) -> Option<WorkflowConclusion> {
        match self {
            Self::Pending(_) => None,
            Self::Settled { conclusion, .. } => Some(*conclusion),
        }
    }

    /// Closed canonical spelling retained by presentation-only projections.
    #[must_use]
    pub const fn kind(&self) -> WorkflowStatusKind {
        match self {
            Self::Pending(_) => WorkflowStatusKind::Pending,
            Self::Settled {
                conclusion:
                    WorkflowConclusion::Positive { .. }
                    | WorkflowConclusion::Negative(WorkflowNegative::Closed),
                ..
            } => WorkflowStatusKind::Completed,
            Self::Settled {
                conclusion: WorkflowConclusion::Negative(WorkflowNegative::Deleted),
                ..
            } => WorkflowStatusKind::Deleted,
        }
    }

    /// Canonical wire and operator-facing status spelling.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.kind().as_str()
    }
}

impl std::fmt::Display for WorkflowStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One current task workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowHead {
    /// Ordered phases the task walks.
    pub flow: Vec<Phase>,
    /// Current phase.
    pub phase: Phase,
    /// Current live or terminal workflow state.
    pub status: WorkflowStatus,
    /// Wakeable deferral deadline.
    pub deferral: Option<Instant>,
    /// Durable task mutation generation.
    ///
    /// A task opens at zero, which is also the baseline every task written
    /// before this field existed carries, so zero is stored as absence. Every
    /// journal transaction that changes its statement, notes, deferral, or
    /// workflow advances this exactly once; claim, lease, and notification
    /// maintenance preserve it. The journal assigns it at commit, so a value
    /// a writer builds into a proposed head is not authority.
    pub generation: u64,
}

impl WorkflowHead {
    /// Whether this workflow concluded positively.
    #[must_use]
    pub const fn concluded_positively(&self) -> bool {
        matches!(
            &self.status,
            WorkflowStatus::Settled {
                conclusion: WorkflowConclusion::Positive { .. },
                ..
            }
        )
    }

    /// Stable review spelling retained by the canonical workflow document.
    #[must_use]
    pub const fn review_str(&self) -> &'static str {
        match &self.status {
            WorkflowStatus::Pending(review) => review.as_str(),
            WorkflowStatus::Settled {
                conclusion: WorkflowConclusion::Positive { review },
                ..
            } => review.as_str(),
            WorkflowStatus::Settled {
                conclusion: WorkflowConclusion::Negative(_),
                ..
            } => "closed",
        }
    }
}

/// Closed canonical status spelling used at summary wire boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatusKind {
    /// Work remains live.
    Pending,
    /// Work settled either positively or by operator closure.
    Completed,
    /// Work was withdrawn.
    Deleted,
}

impl WorkflowStatusKind {
    /// Stable wire and operator-facing spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }
}

impl std::fmt::Display for WorkflowStatusKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WorkflowReviewWire {
    Unassigned,
    Assigned,
    Clean,
    Closed,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowHeadWire {
    flow: Vec<Phase>,
    phase: Phase,
    status: WorkflowStatusKind,
    review: WorkflowReviewWire,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    assignment: Option<Actor>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    deferral: Option<Instant>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    completion: Option<Instant>,
    #[serde(default, skip_serializing_if = "is_zero")]
    generation: u64,
}

impl Serialize for WorkflowHead {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (status, review, assignment, completion) = match &self.status {
            WorkflowStatus::Pending(WorkflowReview::Unassigned) => (
                WorkflowStatusKind::Pending,
                WorkflowReviewWire::Unassigned,
                None,
                None,
            ),
            WorkflowStatus::Pending(WorkflowReview::Assigned { reviewer }) => (
                WorkflowStatusKind::Pending,
                WorkflowReviewWire::Assigned,
                Some(reviewer.clone()),
                None,
            ),
            WorkflowStatus::Pending(WorkflowReview::Clean) => (
                WorkflowStatusKind::Pending,
                WorkflowReviewWire::Clean,
                None,
                None,
            ),
            WorkflowStatus::Settled {
                at,
                conclusion:
                    WorkflowConclusion::Positive {
                        review: WorkflowCompletionReview::Unassigned,
                    },
            } => (
                WorkflowStatusKind::Completed,
                WorkflowReviewWire::Unassigned,
                None,
                Some(*at),
            ),
            WorkflowStatus::Settled {
                at,
                conclusion:
                    WorkflowConclusion::Positive {
                        review: WorkflowCompletionReview::Clean,
                    },
            } => (
                WorkflowStatusKind::Completed,
                WorkflowReviewWire::Clean,
                None,
                Some(*at),
            ),
            WorkflowStatus::Settled {
                at,
                conclusion: WorkflowConclusion::Negative(WorkflowNegative::Closed),
            } => (
                WorkflowStatusKind::Completed,
                WorkflowReviewWire::Closed,
                None,
                Some(*at),
            ),
            WorkflowStatus::Settled {
                at,
                conclusion: WorkflowConclusion::Negative(WorkflowNegative::Deleted),
            } => (
                WorkflowStatusKind::Deleted,
                WorkflowReviewWire::Closed,
                None,
                Some(*at),
            ),
        };
        WorkflowHeadWire {
            flow: self.flow.clone(),
            phase: self.phase.clone(),
            status,
            review,
            assignment,
            deferral: self.deferral,
            completion,
            generation: self.generation,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WorkflowHead {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let wire = WorkflowHeadWire::deserialize(deserializer)?;
        let status = match (wire.status, wire.review, wire.assignment, wire.completion) {
            (WorkflowStatusKind::Pending, WorkflowReviewWire::Unassigned, None, None) => {
                WorkflowStatus::pending()
            }
            (WorkflowStatusKind::Pending, WorkflowReviewWire::Assigned, Some(reviewer), None) => {
                WorkflowStatus::Pending(WorkflowReview::Assigned { reviewer })
            }
            (WorkflowStatusKind::Pending, WorkflowReviewWire::Clean, None, None) => {
                WorkflowStatus::Pending(WorkflowReview::Clean)
            }
            (WorkflowStatusKind::Completed, WorkflowReviewWire::Unassigned, None, Some(at)) => {
                WorkflowStatus::Settled {
                    at,
                    conclusion: WorkflowConclusion::Positive {
                        review: WorkflowCompletionReview::Unassigned,
                    },
                }
            }
            (WorkflowStatusKind::Completed, WorkflowReviewWire::Clean, None, Some(at)) => {
                WorkflowStatus::Settled {
                    at,
                    conclusion: WorkflowConclusion::Positive {
                        review: WorkflowCompletionReview::Clean,
                    },
                }
            }
            (WorkflowStatusKind::Completed, WorkflowReviewWire::Closed, None, Some(at)) => {
                WorkflowStatus::Settled {
                    at,
                    conclusion: WorkflowConclusion::Negative(WorkflowNegative::Closed),
                }
            }
            (WorkflowStatusKind::Deleted, WorkflowReviewWire::Closed, None, Some(at)) => {
                WorkflowStatus::Settled {
                    at,
                    conclusion: WorkflowConclusion::Negative(WorkflowNegative::Deleted),
                }
            }
            _ => {
                return Err(D::Error::custom(
                    "workflow status, review, assignment, and completion disagree",
                ));
            }
        };
        Ok(Self {
            flow: wire.flow,
            phase: wire.phase,
            status,
            deferral: wire.deferral,
            generation: wire.generation,
        })
    }
}
