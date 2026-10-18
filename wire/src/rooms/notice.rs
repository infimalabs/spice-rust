//! Resident records of session-targeted requests for open passive Designs.
//!
//! Each record durably chooses one request. It is not a room publication; it
//! keeps the asked lane listening to the Design's room until that lane leaves,
//! and adds no participation, steering, claim, assignment, or wake.

use serde::{Deserialize, Serialize};

use super::DecisionRef;
use crate::{CanonicalUuid, Digest, Name, Seq, SequenceCode, TaskStamp};

/// Most bytes of completion validation retained for Design request matching.
pub const COMPLETION_VALIDATION_BYTES: usize = 4 * 1024;

/// Most matched terms one Design awareness record carries.
pub const DESIGN_NOTICE_TERMS: usize = 12;

/// Most matched terms any surface shows for one notice, however many its
/// record keeps.
pub const DESIGN_NOTICE_SHOWN_TERMS: usize = 6;

/// The matched terms a surface shows: each distinct term once, in record
/// order, at most [`DESIGN_NOTICE_SHOWN_TERMS`].
#[must_use]
pub fn shown_notice_terms(terms: &[String]) -> Vec<&str> {
    let mut shown: Vec<&str> = Vec::with_capacity(DESIGN_NOTICE_SHOWN_TERMS);
    for term in terms {
        if shown.len() == DESIGN_NOTICE_SHOWN_TERMS {
            break;
        }
        if !shown.contains(&term.as_str()) {
            shown.push(term);
        }
    }
    shown
}

/// One exact continuing vendor session invited to a passive Design.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignSession {
    /// Vendor whose declaration admitted the identity.
    pub vendor: Name,
    /// Exact resumable vendor session identity.
    pub identity: String,
}

/// Exact settled conversation source used by an opportunity or as support.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignConversationEvidence {
    /// Durable transcript record containing the assistant-authored prose.
    pub locus: crate::body::TranscriptLocus,
    /// Identity of the exact cleaned prose retained by the session profile.
    pub digest: Digest,
}

/// Event at which one session profile was evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignNoticeOpportunity {
    /// A successful task completion settled.
    TaskCompletion { task: TaskStamp, completed: Seq },
    /// A retained task selection under this exact claim. `terminal` names the
    /// terminal task operation when one preceded the selection record; its
    /// absence does not pretend that the task completed.
    TaskClaim {
        task: TaskStamp,
        claim: SequenceCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<Seq>,
    },
    /// One assistant-authored conversation message settled.
    Conversation { source: DesignConversationEvidence },
    /// The selected Design was filed or revised.
    DesignStatement,
    /// A bounded profile warm finished, optionally through its newest journal
    /// fact when the profile has task history.
    ProfileReady { horizon: Option<Seq> },
    /// The same continuing session resumed at this declaration.
    SessionResumed { declaration: Seq },
}

/// Exact retained card that supported one Design match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignNoticeEvidence {
    /// Successful task evidence at the statement observed with its completion.
    Task {
        task: TaskStamp,
        statement: Seq,
        completed: Seq,
    },
    /// Task evidence retained under an exact claim. `terminal` is present only
    /// when a terminal task operation preceded the selection record; the claim
    /// remains the witness when no completion did.
    TaskClaim {
        task: TaskStamp,
        statement: Seq,
        claim: SequenceCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<Seq>,
    },
    /// Assistant-authored conversation evidence.
    Conversation { source: DesignConversationEvidence },
}

/// One continuing session was invited to contribute to a passive Design.
///
/// The opportunity and supporting evidence are independent: a new completion
/// can select a Design because of an older task or conversation card. The
/// record is request state only. It grants no participation, steering, claim,
/// assignment, permission to work, or wake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignNoticed {
    /// Passive Design whose room accepts the answer.
    pub design: TaskStamp,
    /// Exact Design statement head evaluated for this request.
    pub statement: Seq,
    /// Logical lane carrying the continuing session when selected.
    pub lane: CanonicalUuid,
    /// Occupant carrying the continuing session when selected.
    pub occupant: CanonicalUuid,
    /// Continuing vendor session that owns the invitation.
    pub session: DesignSession,
    /// Newest compaction boundary of this session before selection. `None`
    /// names the original un-compacted context.
    pub compaction: Option<Seq>,
    /// Event that gave the resident an opportunity to evaluate the profile.
    pub opportunity: DesignNoticeOpportunity,
    /// Retained card whose view selected the Design.
    pub evidence: DesignNoticeEvidence,
    /// Matched terms in ascending order.
    pub terms: Vec<String>,
}

/// The resident handed an active Design's holder the operator's newest return
/// decision as its next input.
///
/// One record consumes that decision: no later claimant is told it again, and
/// only a newer return becomes input. It authorizes and mutates nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignReturnDelivered {
    /// The return decision delivered; its controller is the Design.
    pub decision: DecisionRef,
    /// Logical lane holding the Design.
    pub lane: CanonicalUuid,
    /// Occupant told.
    pub occupant: CanonicalUuid,
    /// Fact that won the claim the delivery served.
    pub claim: SequenceCode,
}

#[cfg(test)]
mod tests {
    use super::{DESIGN_NOTICE_SHOWN_TERMS, shown_notice_terms};

    #[test]
    fn shown_terms_are_distinct_and_capped_in_record_order() {
        let terms = [
            "beta", "alpha", "beta", "gamma", "alpha", "delta", "eps", "zeta", "eta",
        ]
        .map(str::to_owned);
        assert_eq!(
            shown_notice_terms(&terms),
            ["beta", "alpha", "gamma", "delta", "eps", "zeta"]
        );
        let many = (0..20).map(|n| format!("term{n:02}")).collect::<Vec<_>>();
        assert_eq!(shown_notice_terms(&many).len(), DESIGN_NOTICE_SHOWN_TERMS);
        assert!(shown_notice_terms(&[]).is_empty());
    }
}
