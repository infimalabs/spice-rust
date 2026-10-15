//! Resident records of private background notices about open Designs.
//!
//! Each record is the durable read of who was told what; none is a room
//! publication, so none adds room attention, participation, steering, or wake.

use serde::{Deserialize, Serialize};

use super::DecisionRef;
use crate::{CanonicalUuid, Seq, SequenceCode, TaskStamp};

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

/// The resident invited one lane occupant, at most once, to a passive Design.
///
/// The exact statement and matched terms record the organic relevance
/// decision. The record is background information only: it is no room
/// publication, so it adds no room attention, participation, steering, claim,
/// or wake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignNoticed {
    /// The passive Design the occupant was invited to.
    pub design: TaskStamp,
    /// Exact Design statement head evaluated for this invitation.
    pub statement: Seq,
    /// Logical lane told.
    pub lane: CanonicalUuid,
    /// Occupant invited; each Design invites one occupant at most once.
    pub occupant: CanonicalUuid,
    /// Task the lane's claim holds.
    pub task: TaskStamp,
    /// Fact that won that claim.
    pub claim: SequenceCode,
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
