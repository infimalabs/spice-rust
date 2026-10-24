//! Design pass proposal decision DTOs.

use serde::{Deserialize, Serialize};

use super::{CandidateRef, DecisionRef, RoomSource};
use crate::TaskStamp;

/// One output's verdict in a candidate decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputVerdict {
    /// Release this output.
    Accepted,
    /// Close this output: the operator's veto settles it in the decision.
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

/// Exact operator disposition of one immutable Design pass proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DecisionOutcome {
    /// Return this proposal for another pass without authorizing or concluding.
    Returned {
        /// Exact proposal being returned.
        candidate: CandidateRef,
        /// Nonempty operator feedback for the next pass.
        reason: String,
    },
    /// Record one explicit verdict for every work output.
    Resolved {
        /// Exact work proposal being resolved.
        candidate: CandidateRef,
        /// Unique, complete output verdicts in proposal order.
        outputs: Vec<OutputDecision>,
        /// Optional comment applying to the complete proposal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    /// Accept an outputless conclude or collapse recommendation.
    Accepted {
        /// Exact outputless proposal being accepted.
        candidate: CandidateRef,
    },
}

impl DecisionOutcome {
    /// Exact proposal this decision addresses.
    #[must_use]
    pub fn candidate(&self) -> &CandidateRef {
        match self {
            Self::Returned { candidate, .. }
            | Self::Resolved { candidate, .. }
            | Self::Accepted { candidate } => candidate,
        }
    }

    /// Work outputs this decision authorizes, in recorded order.
    pub fn accepted_outputs(&self) -> impl Iterator<Item = &TaskStamp> {
        self.outputs_with(OutputVerdict::Accepted)
    }

    /// Work outputs this decision closes, in recorded order.
    pub fn rejected_outputs(&self) -> impl Iterator<Item = &TaskStamp> {
        self.outputs_with(OutputVerdict::Rejected)
    }

    fn outputs_with(&self, verdict: OutputVerdict) -> impl Iterator<Item = &TaskStamp> {
        match self {
            Self::Resolved { outputs, .. } => EitherOutputs::Resolved(outputs.iter(), verdict),
            Self::Returned { .. } | Self::Accepted { .. } => EitherOutputs::None,
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
            Self::Accepted { .. } => false,
        }
    }
}

enum EitherOutputs<'a> {
    Resolved(std::slice::Iter<'a, OutputDecision>, OutputVerdict),
    None,
}

impl<'a> Iterator for EitherOutputs<'a> {
    type Item = &'a TaskStamp;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Resolved(outputs, verdict) => {
                outputs.find_map(|output| (output.verdict == *verdict).then_some(&output.output))
            }
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
    /// Return, output resolution, or outputless acceptance over one exact
    /// proposal version.
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
            DecisionOutcome::Returned { reason, .. }
                if reason.is_empty() || reason.trim() != reason =>
            {
                return Err("a return decision requires a nonempty trimmed reason");
            }
            DecisionOutcome::Accepted { .. } | DecisionOutcome::Returned { .. } => {}
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
