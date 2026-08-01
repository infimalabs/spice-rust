//! The pre-action actuator: vendor asks, consent decides.
//!
//! A request arrives as [`wire::body::ConsentAsked`] and leaves as exactly one
//! [`Outcome`], which carries the fact that records the decision. The three
//! modules are the three things a decision needs: [`policy`] states which named
//! rule answers, [`budget`] bounds how many times a rule may grant before an
//! operator renews it, and [`pend`] holds what the operator must answer while
//! the lane keeps doing reversible work, then records the answer once given.
//!
//! No policy is decided anywhere else. `vendor` states the request and this
//! crate answers it, so the seat an operator configures is the seat that acts.

#![forbid(unsafe_code)]

pub mod budget;
pub mod pend;
pub mod policy;

pub use budget::{Budgets, Spend};
pub use pend::{Held, Pending, Reversibility, Ruling};
pub use policy::{Answer, Authority, Decision, OPERATOR, POSTURE, Policy, PolicyError, Rule};

use wire::body::{ConsentAsked, ConsentDenied, ConsentGranted, ConsentQueueCause, ConsentQueued};

/// The one disposition a request received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A rule, the posture, or the operator allowed the request.
    Granted(ConsentGranted),
    /// A rule, the posture, or the operator refused the request.
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
                remedy: if denied.rule == OPERATOR {
                    "the operator refused this request; it never proceeds under this identity, so ask again only as a new request".to_owned()
                } else {
                    format!(
                        "the `{}` rule denies this operation; ask the operator to decide it directly, or change that rule",
                        denied.rule
                    )
                },
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
            Answer::Queue => Outcome::Queued(self.pending.hold(
                asked,
                reversibility,
                rule,
                ConsentQueueCause::Policy {},
            )),
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
                    Spend::Exhausted => Outcome::Queued(self.pending.hold(
                        asked,
                        reversibility,
                        rule,
                        ConsentQueueCause::BudgetExhausted { bound },
                    )),
                },
            },
        }
    }

    /// Whether this request's action proceeds while requests are held.
    #[must_use]
    pub fn proceeds(&self, request: wire::CanonicalUuid, reversibility: Reversibility) -> bool {
        self.pending.proceeds(request, reversibility)
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

    /// Renew the budget named by one exact exhausted request and release it.
    ///
    /// # Errors
    ///
    /// Returns a loud refusal when the request is not held or was queued by
    /// policy rather than by an exhausted budget.
    pub fn renew(&mut self, request: wire::CanonicalUuid) -> Result<Held, wire::Refusal> {
        let Some(held) = self.pending.get(request) else {
            return Err(wire::Refusal {
                reason: "consent::request-not-held".to_owned(),
                witness: Some(request.to_string()),
                expected: Some("a held budget-exhausted request".to_owned()),
                observed: Some("no held request".to_owned()),
                remedy: "re-read the consent queue and renew a request that is still held"
                    .to_owned(),
            });
        };
        match held.cause() {
            ConsentQueueCause::Policy {} => {
                return Err(wire::Refusal {
                    reason: "consent::renewal-not-required".to_owned(),
                    witness: Some(held.rule().to_owned()),
                    expected: Some("budget-exhausted queue".to_owned()),
                    observed: Some("policy queue".to_owned()),
                    remedy: format!(
                        "decide request {request} directly; the `{}` rule did not exhaust a budget",
                        held.rule()
                    ),
                });
            }
            ConsentQueueCause::BudgetExhausted { .. } => {}
        }

        let rule = held.rule().to_owned();
        self.budgets.renew(&rule);
        // Renewing is the operator granting the renewal the exhausted budget
        // queued for, so the request leaves the queue allowed rather than
        // refused and its action proceeds.
        Ok(self
            .pending
            .release(request, Ruling::Allow)
            .expect("the inspected request remains held until this release"))
    }

    /// Record the operator's answer to a held request and state the fact.
    ///
    /// The answer is the request's terminal disposition, so it reads as an
    /// [`Outcome`] exactly like the answer a rule gives: a grant opens the
    /// action the queue was holding, and a refusal closes it for good. A
    /// request nothing holds has no answer to record and yields nothing.
    pub fn release(&mut self, request: wire::CanonicalUuid, ruling: Ruling) -> Option<Outcome> {
        let held = self.pending.release(request, ruling)?;
        let rule = Authority::Operator.name().to_owned();
        Some(match ruling {
            Ruling::Allow => Outcome::Granted(ConsentGranted {
                request,
                lane: held.lane(),
                rule,
                budget: None,
            }),
            Ruling::Deny => Outcome::Denied(ConsentDenied {
                request,
                lane: held.lane(),
                rule,
            }),
        })
    }
}

#[cfg(test)]
mod tests;
