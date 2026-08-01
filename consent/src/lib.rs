//! The pre-action actuator: vendor asks, consent decides.
//!
//! A request arrives as [`wire::body::ConsentAsked`] and leaves as exactly one
//! [`Outcome`], which carries the fact that records the decision. The three
//! modules are the three things a decision needs: [`policy`] states which named
//! rule answers, [`budget`] bounds how many times a rule may grant before an
//! operator renews it, and [`pend`] holds what the operator must answer while
//! the lane keeps doing reversible work.
//!
//! No policy is decided anywhere else. `vendor` states the request and this
//! crate answers it, so the seat an operator configures is the seat that acts.

#![forbid(unsafe_code)]

pub mod budget;
pub mod pend;
pub mod policy;

pub use budget::{Budgets, Spend};
pub use pend::{Held, Pending, Reversibility};
pub use policy::{Answer, Authority, Decision, POSTURE, Policy, PolicyError, Rule};

use wire::body::{ConsentAsked, ConsentDenied, ConsentGranted, ConsentQueued};

/// The one disposition a request received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A rule or the posture allowed the request.
    Granted(ConsentGranted),
    /// A rule or the posture refused the request.
    Denied(ConsentDenied),
    /// The request waits for the operator.
    Queued(ConsentQueued),
}

impl Outcome {
    /// The fact kind this outcome records.
    #[must_use]
    pub const fn kind(&self) -> wire::FactKind {
        match self {
            Self::Granted(_) => wire::FactKind::ConsentGranted,
            Self::Denied(_) => wire::FactKind::ConsentDenied,
            Self::Queued(_) => wire::FactKind::ConsentQueued,
        }
    }

    /// The loud refusal a denied request owes its caller.
    ///
    /// Only a denial refuses. A grant happened and a queued request has not
    /// been answered yet, so neither is a refusal and neither may pretend to
    /// be one.
    #[must_use]
    pub fn refusal(&self) -> Option<wire::Refusal> {
        match self {
            Self::Granted(_) | Self::Queued(_) => None,
            Self::Denied(denied) => Some(wire::Refusal {
                reason: "consent denied the operation".to_owned(),
                witness: Some(denied.rule.clone()),
                expected: None,
                observed: None,
                remedy: format!(
                    "the `{}` rule denies this operation; ask the operator to decide it directly, or change that rule",
                    denied.rule
                ),
            }),
        }
    }
}

/// The seat that decides whether a requested action proceeds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Actuator {
    policy: Policy,
    budgets: Budgets,
    pending: Pending,
}

impl Actuator {
    /// Seat an actuator under one rule table.
    #[must_use]
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            budgets: Budgets::new(),
            pending: Pending::new(),
        }
    }

    /// Decide one request, recording the rule that answered it.
    ///
    /// A rule that grants under a budget grants until its bound and then
    /// queues, so the bound is reached by queueing for renewal rather than by
    /// quietly granting past it.
    pub fn decide(&mut self, asked: &ConsentAsked, reversibility: Reversibility) -> Outcome {
        let decision = self.policy.decide(&asked.operation);
        let rule = decision.authority().name().to_owned();
        match decision.answer() {
            Answer::Deny => Outcome::Denied(ConsentDenied {
                request: asked.request,
                lane: asked.lane,
                rule,
            }),
            Answer::Queue => Outcome::Queued(self.pending.hold(asked, reversibility)),
            Answer::Allow => match decision.bound() {
                None => Outcome::Granted(ConsentGranted {
                    request: asked.request,
                    lane: asked.lane,
                    rule,
                    budget: None,
                }),
                Some(bound) => match self.budgets.spend(&rule, bound) {
                    Spend::Granted { remaining } => Outcome::Granted(ConsentGranted {
                        request: asked.request,
                        lane: asked.lane,
                        rule,
                        budget: Some(remaining),
                    }),
                    Spend::Exhausted => Outcome::Queued(self.pending.hold(asked, reversibility)),
                },
            },
        }
    }

    /// Whether work of this reversibility proceeds while requests are held.
    #[must_use]
    pub fn proceeds(&self, reversibility: Reversibility) -> bool {
        self.pending.proceeds(reversibility)
    }

    /// The rule table this seat decides by.
    #[must_use]
    pub const fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Grants spent per rule since each rule's last renewal.
    #[must_use]
    pub const fn budgets(&self) -> &Budgets {
        &self.budgets
    }

    /// Requests waiting on the operator.
    #[must_use]
    pub const fn pending(&self) -> &Pending {
        &self.pending
    }

    /// Renew a rule's budget after an operator grants the renewal.
    pub fn renew(&mut self, rule: &str) {
        self.budgets.renew(rule);
    }

    /// Release a held request once the operator has answered it.
    pub fn release(&mut self, request: wire::CanonicalUuid) -> Option<Held> {
        self.pending.release(request)
    }
}

#[cfg(test)]
mod tests;
