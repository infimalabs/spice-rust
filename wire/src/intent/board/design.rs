//! Design standing and the passive Design invitation a session owes.

use serde::{Deserialize, Serialize};

/// One passive Design invitation owed to a continuing vendor session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignRequest {
    /// Design whose room accepts the answer.
    pub design: crate::TaskHandle,
    /// Lane carrying the continuing session.
    pub lane: crate::CanonicalUuid,
    /// Continuing session that owns the invitation.
    pub session: crate::rooms::DesignSession,
    /// Event at which the session profile was evaluated.
    pub opportunity: crate::rooms::DesignNoticeOpportunity,
    /// Exact retained card that supported the match.
    pub evidence: crate::rooms::DesignNoticeEvidence,
    /// The supporting card's bounded authored excerpt.
    pub excerpt: String,
    /// Current Design title.
    pub title: String,
    /// Current Design statement revision.
    pub statement: crate::SequenceCode,
    /// One-line Design context.
    pub pitch: String,
    /// In-order prefix of whole open questions bounded for delivery.
    pub questions: Vec<String>,
    /// Distinctive shared terms that selected this Design.
    pub terms: Vec<String>,
    /// Working-group stance whose relevance floor admitted the match, when the
    /// record kept it; Explore admits on a slightly wider cast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<crate::rooms::WorkingGroupStance>,
    /// The question items asked, the best-scoring first and the rest in
    /// statement order; empty asks about the whole Design.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<DesignRequestAsk>,
}

/// One question item a Design request asks about, at its statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignRequestAsk {
    /// The item's 1-based position.
    pub question: crate::rooms::RoomQuestion,
    /// The item verbatim.
    pub text: String,
    /// Where the item stood when asked; contested asks a lane already in
    /// its series for a strength-2 re-charge.
    pub state: crate::rooms::DesignItemState,
    /// Charged posts the item holds.
    pub charges: u64,
    /// Distinct lanes those posts came from, as retained.
    pub lanes: usize,
}

/// Whether the current heads describe an open Design.
///
/// This is the canonical constant-time rule used by authority and presentation
/// surfaces. A Design-intent task in an intake phase is ordinary claimed work,
/// not an open Design.
#[must_use]
pub fn is_open_design(intent: Option<crate::TaskIntent>, workflow: &crate::WorkflowHead) -> bool {
    intent.is_some_and(crate::TaskIntent::is_design)
        && workflow.status.is_pending()
        && workflow.phase.as_str() == "design"
}

/// The parent that released this construction root: a root built under a
/// Plan or Design is born depending on its immediate parent, and a Design
/// decision removes that one edge when it accepts the output. The returned
/// parent is the controller that now waits on the root until it settles.
/// Corrections carry provenance without ever holding the edge and are not
/// construction roots, and a self-owned Plan or Design releases nothing.
#[must_use]
pub fn released_construction_root<'a>(
    born: &crate::TaskStamp,
    dependencies: &[crate::TaskStamp],
    preparation: Option<&'a crate::TaskPreparation>,
) -> Option<&'a crate::TaskStamp> {
    let preparation = preparation?;
    (preparation.construction == *born
        && preparation.parent != *born
        && !dependencies.contains(&preparation.parent))
    .then_some(&preparation.parent)
}

/// One Design's lifecycle standing. An open Design is passive while a Passive
/// working group holds it and active otherwise; only the operator concludes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignStanding {
    /// Open in a Passive working group: no path grants it a claim.
    Passive,
    /// Open outside every working group or in an Explore or Converge one:
    /// lanes may claim it or be nudged onto it.
    Active,
    /// The operator concluded it done.
    Done,
    /// The operator concluded it collapsed. A retained record concluded
    /// before collapse recorded a reason carries none.
    Collapsed {
        /// Why its premise did not hold.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The operator withdrew it.
    Deleted,
}

impl DesignStanding {
    /// Derive the standing every board, task, group, room, and browser read
    /// shows from current heads, so terminal settlement outranks any retained
    /// group membership; `None` for a task without Design intent.
    #[must_use]
    pub fn of(
        intent: Option<crate::TaskIntent>,
        workflow: &crate::WorkflowHead,
        active: bool,
        collapse_reason: Option<&str>,
    ) -> Option<Self> {
        if !intent.is_some_and(crate::TaskIntent::is_design) {
            return None;
        }
        Some(match workflow.status {
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Negative(crate::WorkflowNegative::Deleted),
                ..
            } => Self::Deleted,
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Positive { .. },
                ..
            } => Self::Done,
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Negative(crate::WorkflowNegative::Closed),
                ..
            } => Self::Collapsed {
                reason: collapse_reason.map(str::to_owned),
            },
            crate::WorkflowStatus::Pending(_) if !is_open_design(intent, workflow) => return None,
            crate::WorkflowStatus::Pending(_) if active => Self::Active,
            crate::WorkflowStatus::Pending(_) => Self::Passive,
        })
    }

    /// Whether the Design is still open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Passive | Self::Active)
    }

    /// Stable machine key.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::Done => "done",
            Self::Collapsed { .. } => "collapsed",
            Self::Deleted => "deleted",
        }
    }
}

impl std::fmt::Display for DesignStanding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passive => formatter.write_str("open passive (in a Passive working group)"),
            Self::Active => formatter.write_str("open active"),
            Self::Done => formatter.write_str("concluded done"),
            Self::Collapsed {
                reason: Some(reason),
            } => write!(formatter, "concluded collapsed: {reason}"),
            Self::Collapsed { reason: None } => {
                formatter.write_str("concluded collapsed (no reason recorded)")
            }
            Self::Deleted => formatter.write_str("deleted"),
        }
    }
}
