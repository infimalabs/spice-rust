//! The sole candidate-to-fact transition.

use wire::{Fact, body};

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

    /// Mint a fact only for a confirmed violation.
    ///
    /// A candidate hit has no fact conversion, and clearing consumes the
    /// judgment without producing one. This keeps an unjudged or harmless
    /// trigger outside the journal path by construction.
    #[must_use]
    pub fn into_fact(self) -> Option<Fact<body::TasteConfirmed>> {
        (self.decision == Decision::Confirm).then(|| {
            Fact::new(body::TasteConfirmed {
                taste: self.hit.taste().to_string(),
                locus: self.hit.locus().to_string(),
            })
        })
    }
}
