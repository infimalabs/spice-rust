//! The sole candidate-to-fact transition.

use wire::{Fact, FactKind, body};

use crate::Hit;

/// Human or model judgment over one candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Confirm,
    Clear,
}

/// One candidate with an explicit decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Judgment {
    hit: Hit,
    decision: Decision,
}

/// The two facts an explicit taste judgment may mint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JudgmentFact {
    /// The prose exhibited the disfavored shape.
    Confirmed(Fact<body::TasteConfirmed>),
    /// The prose only quoted, cited, or analyzed the candidate.
    Cleared(Fact<body::TasteCleared>),
}

impl JudgmentFact {
    /// Discriminant registered for the judgment's typed body.
    #[must_use]
    pub const fn kind(&self) -> FactKind {
        match self {
            Self::Confirmed(fact) => fact.kind(),
            Self::Cleared(fact) => fact.kind(),
        }
    }
}

/// Bind one candidate to the decision made with its surrounding context.
#[must_use]
pub const fn judge(hit: Hit, decision: Decision) -> Judgment {
    Judgment { hit, decision }
}

impl Judgment {
    /// Decision made over the candidate.
    #[must_use]
    pub const fn decision(&self) -> Decision {
        self.decision
    }

    /// Candidate and context the decision used.
    #[must_use]
    pub const fn hit(&self) -> &Hit {
        &self.hit
    }

    /// Mint the one fact selected by this explicit judgment.
    ///
    /// A candidate hit has no fact conversion. Judging is therefore the only
    /// door to either the confirmation or clearance journal path.
    #[must_use]
    pub fn into_fact(self) -> JudgmentFact {
        let taste = self.hit.taste().to_string();
        let locus = self.hit.locus().to_string();
        match self.decision {
            Decision::Confirm => {
                JudgmentFact::Confirmed(Fact::new(body::TasteConfirmed { taste, locus }))
            }
            Decision::Clear => {
                JudgmentFact::Cleared(Fact::new(body::TasteCleared { taste, locus }))
            }
        }
    }
}
