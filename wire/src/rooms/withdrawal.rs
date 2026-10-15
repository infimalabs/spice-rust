//! An author's withdrawal of one exact candidate it retained, before any
//! decision. The candidate stays retained; withdrawal only removes it from
//! the operator's choices.

use serde::{Deserialize, Serialize};

use super::CandidateRef;

/// The author's retraction of one exact retained candidate before any
/// decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CandidateWithdrawalWire", into = "CandidateWithdrawalWire")]
pub struct CandidateWithdrawal {
    /// Exact candidate withdrawn.
    pub candidate: CandidateRef,
    /// Nonempty author reason, such as which candidate corrects it.
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateWithdrawalWire {
    candidate: CandidateRef,
    reason: String,
}

impl TryFrom<CandidateWithdrawalWire> for CandidateWithdrawal {
    type Error = &'static str;

    fn try_from(value: CandidateWithdrawalWire) -> Result<Self, Self::Error> {
        if value.reason.trim().is_empty() {
            return Err("a candidate withdrawal requires a nonempty reason");
        }
        Ok(Self {
            candidate: value.candidate,
            reason: value.reason,
        })
    }
}

impl From<CandidateWithdrawal> for CandidateWithdrawalWire {
    fn from(value: CandidateWithdrawal) -> Self {
        Self {
            candidate: value.candidate,
            reason: value.reason,
        }
    }
}

/// Command body withdrawing one exact candidate its author retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateWithdraw {
    /// Exact candidate and the author's reason.
    pub withdrawal: CandidateWithdrawal,
}

/// Immutable fact body withdrawing one exact undecided candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateWithdrawn {
    /// Exact candidate and the author's reason.
    pub withdrawal: CandidateWithdrawal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_withdrawal_names_its_exact_candidate_and_needs_a_reason() {
        let withdrawal = |reason: &str| {
            serde_json::from_value::<CandidateWithdrawal>(serde_json::json!({
                "candidate": {"controller": "1kKRRsDR0", "candidate": "1kKRRsDR1"},
                "reason": reason,
            }))
        };
        assert_eq!(
            withdrawal("superseded").expect("withdrawal").candidate,
            CandidateRef {
                controller: "1kKRRsDR0".parse().expect("controller"),
                candidate: "1kKRRsDR1".parse().expect("candidate"),
            }
        );
        assert!(withdrawal(" ").is_err());
        assert!(
            serde_json::from_value::<CandidateWithdrawal>(serde_json::json!({
                "candidate": {"controller": "1kKRRsDR0", "candidate": "1kKRRsDR1"},
                "reason": "superseded",
                "decided": true,
            }))
            .is_err(),
            "unknown fields refuse"
        );
    }
}
