//! Candidate decision DTOs and their retained compatibility shapes.

use serde::{Deserialize, Serialize};

use super::{CandidateRef, DecisionRef, RoomSource, all_unique};
use crate::TaskStamp;

/// One output's verdict in a candidate decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputVerdict {
    /// Release this output.
    Accepted,
    /// Keep this output gated.
    Rejected,
}

/// The operator's exact verdict and optional comment for one candidate output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDecision {
    /// Candidate output this verdict addresses.
    pub output: TaskStamp,
    /// Whether this output is released.
    pub verdict: OutputVerdict,
    /// Optional operator comment about this output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Exact operator disposition of one immutable retained candidate version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DecisionOutcome {
    /// Authorize exactly this nonempty subset of the candidate's outputs.
    ///
    /// Retained for decisions recorded before output-by-output review.
    Selected {
        /// Exact retained candidate version being selected.
        candidate: CandidateRef,
        /// Unique output subset this decision releases.
        selected_outputs: Vec<TaskStamp>,
    },
    /// Return this candidate version for further preparation.
    ///
    /// Retained for decisions recorded before output-by-output review.
    Returned {
        /// Exact retained candidate version being returned.
        candidate: CandidateRef,
        /// Nonempty operator reason for returning it.
        reason: String,
    },
    /// Record one explicit verdict for every output.
    Resolved {
        /// Exact retained candidate version being resolved.
        candidate: CandidateRef,
        /// Unique, complete output verdicts in candidate order.
        outputs: Vec<OutputDecision>,
        /// Optional comment applying to the complete candidate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    /// Collapse the Design through its conclusion authority, on this
    /// candidate's premise.
    Collapse {
        /// Exact retained candidate version whose premise did not hold.
        candidate: CandidateRef,
        /// Nonempty operator reason for collapsing it.
        reason: String,
    },
}

impl DecisionOutcome {
    /// Exact retained candidate version this decision addresses.
    #[must_use]
    pub fn candidate(&self) -> &CandidateRef {
        match self {
            Self::Selected { candidate, .. }
            | Self::Returned { candidate, .. }
            | Self::Resolved { candidate, .. }
            | Self::Collapse { candidate, .. } => candidate,
        }
    }

    /// Outputs this decision authorizes, in recorded order.
    pub fn accepted_outputs(&self) -> impl Iterator<Item = &TaskStamp> {
        match self {
            Self::Selected {
                selected_outputs, ..
            } => EitherOutputs::Selected(selected_outputs.iter()),
            Self::Resolved { outputs, .. } => EitherOutputs::Resolved(outputs.iter()),
            Self::Returned { .. } | Self::Collapse { .. } => EitherOutputs::None,
        }
    }

    /// Whether this decision carries feedback for the Design's next holder.
    ///
    /// Accepted outputs with no comments release work without creating another
    /// input. A rejection or either comment does.
    #[must_use]
    pub fn carries_feedback(&self) -> bool {
        match self {
            Self::Returned { .. } => true,
            Self::Resolved {
                outputs, comment, ..
            } => {
                comment.is_some()
                    || outputs.iter().any(|output| {
                        output.verdict == OutputVerdict::Rejected || output.comment.is_some()
                    })
            }
            Self::Selected { .. } | Self::Collapse { .. } => false,
        }
    }
}

enum EitherOutputs<'a> {
    Selected(std::slice::Iter<'a, TaskStamp>),
    Resolved(std::slice::Iter<'a, OutputDecision>),
    None,
}

impl<'a> Iterator for EitherOutputs<'a> {
    type Item = &'a TaskStamp;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Selected(outputs) => outputs.next(),
            Self::Resolved(outputs) => outputs.find_map(|output| {
                (output.verdict == OutputVerdict::Accepted).then_some(&output.output)
            }),
            Self::None => None,
        }
    }
}

/// One source-backed operator decision under a controlling task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PreparationDecisionWire", into = "PreparationDecisionWire")]
pub struct PreparationDecision {
    /// Stable decision identity.
    pub decision: DecisionRef,
    /// Exact authority owning the decision basis.
    pub source: RoomSource,
    /// Selection, return, output resolution, or collapse over one exact
    /// candidate version.
    pub outcome: DecisionOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparationDecisionWire {
    decision: DecisionRef,
    source: RoomSource,
    outcome: DecisionOutcome,
}

impl TryFrom<PreparationDecisionWire> for PreparationDecision {
    type Error = &'static str;

    fn try_from(value: PreparationDecisionWire) -> Result<Self, Self::Error> {
        if value.outcome.candidate().controller != value.decision.controller {
            return Err("decision and candidate must share one controlling task");
        }
        match &value.outcome {
            DecisionOutcome::Selected {
                selected_outputs, ..
            } if selected_outputs.is_empty() || !all_unique(selected_outputs) => {
                return Err("a selection must name a nonempty unique output set");
            }
            DecisionOutcome::Resolved {
                outputs, comment, ..
            } => {
                if outputs.is_empty()
                    || outputs.iter().enumerate().any(|(index, output)| {
                        outputs[index + 1..]
                            .iter()
                            .any(|other| other.output == output.output)
                    })
                {
                    return Err("an output resolution must name a nonempty unique output set");
                }
                if outputs
                    .iter()
                    .any(|output| invalid_optional_comment(output.comment.as_deref()))
                    || invalid_optional_comment(comment.as_deref())
                {
                    return Err("decision comments must be nonempty and trimmed when present");
                }
            }
            DecisionOutcome::Returned { reason, .. } | DecisionOutcome::Collapse { reason, .. }
                if reason.trim().is_empty() =>
            {
                return Err("a return or collapse decision requires a nonempty reason");
            }
            _ => {}
        }
        Ok(Self {
            decision: value.decision,
            source: value.source,
            outcome: value.outcome,
        })
    }
}

fn invalid_optional_comment(comment: Option<&str>) -> bool {
    comment.is_some_and(|comment| comment.is_empty() || comment.trim() != comment)
}

impl From<PreparationDecision> for PreparationDecisionWire {
    fn from(value: PreparationDecision) -> Self {
        Self {
            decision: value.decision,
            source: value.source,
            outcome: value.outcome,
        }
    }
}

/// Command body recording one preparation decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationDecide {
    /// Source-backed decision to record.
    pub decision: PreparationDecision,
}

/// Immutable fact body recording one preparation decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationDecided {
    /// Source-backed decision that was recorded.
    pub decision: PreparationDecision,
}
