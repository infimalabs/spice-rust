use std::collections::{BTreeMap, btree_map::Entry};
use std::error::Error;
use std::fmt;

use journal::FactSnapshot;
use wire::body::{ConsentAsked, ConsentDenied, ConsentGranted, ConsentQueueCause, ConsentQueued};
use wire::intent::{
    ConsentBudgetAnswer, ConsentPendingAnswer, ConsentPolicyAnswer, ConsentPolicyDecision,
    ConsentQueueAnswer, ConsentRuleAnswer,
};
use wire::{CanonicalUuid, FactKind, Name, Origin, Refusal, Seq, Witness};

use crate::{Answer, OPERATOR, Policy};

/// Durable materialization of pending consent and renewal budgets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Queue {
    requests: BTreeMap<CanonicalUuid, RequestState>,
    budgets: BTreeMap<String, BudgetState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BudgetState {
    bound: u32,
    remaining: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RequestState {
    Asked {
        lane: CanonicalUuid,
        operation: String,
        fact: Seq,
        origin: Origin,
    },
    Pending(PendingRequest),
    Terminal,
}

/// One request still waiting on the operator, including its causal fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRequest {
    request: CanonicalUuid,
    lane: CanonicalUuid,
    operation: String,
    rule: String,
    cause: ConsentQueueCause,
    fact: Seq,
    origin: Origin,
}

impl PendingRequest {
    /// Request identity.
    #[must_use]
    pub const fn request(&self) -> CanonicalUuid {
        self.request
    }

    /// Lane that raised the request.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// Rule or posture that queued the request.
    #[must_use]
    pub fn rule(&self) -> &str {
        &self.rule
    }

    /// Request-bound queue cause.
    #[must_use]
    pub const fn cause(&self) -> ConsentQueueCause {
        self.cause
    }

    /// Journal fact that recorded this queue entry.
    #[must_use]
    pub const fn fact(&self) -> Seq {
        self.fact
    }

    /// Origin inherited by the terminal ruling.
    #[must_use]
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
}

impl Queue {
    /// Whether one fact kind can alter consent standing.
    #[must_use]
    pub const fn observes(kind: FactKind) -> bool {
        matches!(
            kind,
            FactKind::ConsentAsked
                | FactKind::ConsentQueued
                | FactKind::ConsentGranted
                | FactKind::ConsentDenied
        )
    }

    /// Rebuild consent state from one complete journal snapshot.
    ///
    /// # Errors
    ///
    /// Returns a semantic replay error when consent facts do not form one
    /// asked, optionally queued, then terminal chain per request identity.
    pub fn from_snapshot(snapshot: &FactSnapshot, policy: &Policy) -> Result<Self, QueueError> {
        let mut queue = Self {
            requests: BTreeMap::new(),
            budgets: policy
                .rules()
                .iter()
                .filter_map(|rule| {
                    rule.budget().map(|bound| {
                        (
                            rule.name().to_owned(),
                            BudgetState {
                                bound,
                                remaining: bound,
                            },
                        )
                    })
                })
                .collect(),
        };
        queue.advance(snapshot.facts())?;
        Ok(queue)
    }

    /// Advance this queue from a contiguous suffix of already validated facts.
    ///
    /// Non-consent facts are ignored without decoding. Callers can therefore
    /// apply one committed delta while retaining all prior request and budget
    /// state.
    pub fn advance<'a>(
        &mut self,
        facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    ) -> Result<(), QueueError> {
        for envelope in facts {
            match envelope.fact.kind() {
                FactKind::ConsentAsked => {
                    let asked = decode::<ConsentAsked>(envelope)?;
                    self.ask(asked, envelope.seq, envelope.origin.clone())?;
                }
                FactKind::ConsentQueued => {
                    let queued = decode::<ConsentQueued>(envelope)?;
                    self.pend(
                        queued,
                        envelope.seq,
                        envelope.origin.clone(),
                        envelope.cause,
                    )?;
                }
                FactKind::ConsentGranted => {
                    let granted = decode::<ConsentGranted>(envelope)?;
                    self.grant(granted, envelope.seq, envelope.cause, &envelope.origin)?;
                }
                FactKind::ConsentDenied => {
                    let denied = decode::<ConsentDenied>(envelope)?;
                    self.deny(denied, envelope.seq, envelope.cause, &envelope.origin)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Find one request that is still pending.
    #[must_use]
    pub fn pending(&self, request: CanonicalUuid) -> Option<&PendingRequest> {
        match self.requests.get(&request) {
            Some(RequestState::Pending(pending)) => Some(pending),
            Some(RequestState::Asked { .. } | RequestState::Terminal) | None => None,
        }
    }

    /// Render the current queue and budgets as their wire answer.
    ///
    /// # Errors
    ///
    /// Returns an error if a durable queued rule is not a valid witness name.
    pub fn answer(&self, policy: &Policy) -> Result<ConsentQueueAnswer, QueueError> {
        let pending = self
            .requests
            .values()
            .filter_map(|state| match state {
                RequestState::Pending(pending) => Some(pending),
                RequestState::Asked { .. } | RequestState::Terminal => None,
            })
            .map(|pending| self.pending_answer(pending))
            .collect::<Result<Vec<_>, _>>()?;
        let budgets = policy
            .rules()
            .iter()
            .filter_map(|rule| {
                rule.budget().map(|bound| {
                    let state = self.budget(rule.name(), bound);
                    ConsentBudgetAnswer {
                        rule: rule
                            .name()
                            .parse()
                            .expect("an admitted policy rule carries a Name"),
                        bound: state.bound,
                        remaining: state.remaining,
                    }
                })
            })
            .collect();
        Ok(ConsentQueueAnswer { pending, budgets })
    }

    /// Render tracked policy with the current budget state.
    #[must_use]
    pub fn policy_answer(&self, policy: &Policy) -> ConsentPolicyAnswer {
        ConsentPolicyAnswer {
            posture: policy_decision(policy.posture()),
            rules: policy
                .rules()
                .iter()
                .map(|rule| ConsentRuleAnswer {
                    name: rule
                        .name()
                        .parse()
                        .expect("an admitted policy rule carries a Name"),
                    tool: rule.tool().to_owned(),
                    answer: policy_decision(rule.answer()),
                    budget: rule.budget(),
                    remaining: rule
                        .budget()
                        .map(|bound| self.budget(rule.name(), bound).remaining),
                })
                .collect(),
        }
    }

    fn ask(&mut self, asked: ConsentAsked, seq: Seq, origin: Origin) -> Result<(), QueueError> {
        match self.requests.entry(asked.request) {
            Entry::Vacant(entry) => {
                entry.insert(RequestState::Asked {
                    lane: asked.lane,
                    operation: asked.operation,
                    fact: seq,
                    origin,
                });
                Ok(())
            }
            Entry::Occupied(_) => Err(invalid(
                seq,
                asked.request,
                "request was asked more than once",
            )),
        }
    }

    fn pend(
        &mut self,
        queued: ConsentQueued,
        seq: Seq,
        origin: Origin,
        cause: Option<Seq>,
    ) -> Result<(), QueueError> {
        let Some(state) = self.requests.get_mut(&queued.request) else {
            return Err(invalid(seq, queued.request, "queue has no preceding ask"));
        };
        let RequestState::Asked {
            lane,
            operation,
            fact,
            origin: asked_origin,
        } = state
        else {
            return Err(invalid(
                seq,
                queued.request,
                "request was queued after a disposition",
            ));
        };
        if *lane != queued.lane {
            return Err(invalid(
                seq,
                queued.request,
                "queue lane differs from its ask",
            ));
        }
        if cause != Some(*fact) {
            return Err(invalid(seq, queued.request, "queue cause is not its ask"));
        }
        if asked_origin.as_str() != origin.as_str() {
            return Err(invalid(
                seq,
                queued.request,
                "queue origin differs from its ask",
            ));
        }
        if let ConsentQueueCause::BudgetExhausted { bound } = queued.cause {
            self.budgets.insert(
                queued.rule.clone(),
                BudgetState {
                    bound,
                    remaining: 0,
                },
            );
        }
        *state = RequestState::Pending(PendingRequest {
            request: queued.request,
            lane: queued.lane,
            operation: std::mem::take(operation),
            rule: queued.rule,
            cause: queued.cause,
            fact: seq,
            origin,
        });
        Ok(())
    }

    fn grant(
        &mut self,
        granted: ConsentGranted,
        seq: Seq,
        cause: Option<Seq>,
        origin: &Origin,
    ) -> Result<(), QueueError> {
        let previous = self.terminal(granted.request, granted.lane, seq, cause, origin)?;
        if granted.rule == OPERATOR {
            if granted.budget.is_some() {
                return Err(invalid(
                    seq,
                    granted.request,
                    "operator grant carries a rule budget",
                ));
            }
            if let RequestState::Pending(pending) = previous
                && let ConsentQueueCause::BudgetExhausted { bound } = pending.cause
            {
                self.budgets.insert(
                    pending.rule,
                    BudgetState {
                        bound,
                        remaining: bound,
                    },
                );
            }
        } else if let Some(remaining) = granted.budget
            && let Some(budget) = self.budgets.get_mut(&granted.rule)
        {
            budget.remaining = remaining.min(budget.bound);
        }
        Ok(())
    }

    fn deny(
        &mut self,
        denied: ConsentDenied,
        seq: Seq,
        cause: Option<Seq>,
        origin: &Origin,
    ) -> Result<(), QueueError> {
        self.terminal(denied.request, denied.lane, seq, cause, origin)?;
        Ok(())
    }

    fn terminal(
        &mut self,
        request: CanonicalUuid,
        lane: CanonicalUuid,
        seq: Seq,
        cause: Option<Seq>,
        origin: &Origin,
    ) -> Result<RequestState, QueueError> {
        let Some(state) = self.requests.get_mut(&request) else {
            return Err(invalid(
                seq,
                request,
                "terminal ruling has no preceding ask",
            ));
        };
        let (state_lane, state_fact, state_origin) = match &*state {
            RequestState::Asked {
                lane, fact, origin, ..
            } => (*lane, *fact, origin),
            RequestState::Pending(pending) => (pending.lane, pending.fact, &pending.origin),
            RequestState::Terminal => {
                return Err(invalid(
                    seq,
                    request,
                    "request has more than one terminal ruling",
                ));
            }
        };
        if state_lane != lane {
            return Err(invalid(seq, request, "terminal lane differs from its ask"));
        }
        if cause != Some(state_fact) {
            return Err(invalid(
                seq,
                request,
                "terminal cause is not its current request disposition",
            ));
        }
        if state_origin != origin {
            return Err(invalid(
                seq,
                request,
                "terminal origin differs from its request",
            ));
        }
        Ok(std::mem::replace(state, RequestState::Terminal))
    }

    fn pending_answer(&self, pending: &PendingRequest) -> Result<ConsentPendingAnswer, QueueError> {
        let rule = pending.rule.parse::<Name>().map_err(|_| QueueError {
            seq: pending.fact,
            detail: format!(
                "queued consent rule {:?} is not a witness name",
                pending.rule
            ),
        })?;
        let budget = match pending.cause {
            ConsentQueueCause::Policy {} => None,
            ConsentQueueCause::BudgetExhausted { bound } => {
                let state = self.budget(&pending.rule, bound);
                Some(ConsentBudgetAnswer {
                    rule: rule.clone(),
                    bound: state.bound,
                    remaining: state.remaining,
                })
            }
        };
        Ok(ConsentPendingAnswer {
            request: pending.request,
            lane: pending.lane,
            operation: pending.operation.clone(),
            rule,
            cause: pending.cause,
            budget,
        })
    }

    fn budget(&self, rule: &str, bound: u32) -> BudgetState {
        self.budgets.get(rule).map_or(
            BudgetState {
                bound,
                remaining: bound,
            },
            |state| BudgetState {
                bound,
                remaining: state.remaining.min(bound),
            },
        )
    }
}

/// Build the exact witnesses for one terminal operator answer.
///
/// # Errors
///
/// Refuses non-terminal answers and malformed durable rule names.
pub fn decision_witnesses(
    pending: &ConsentPendingAnswer,
    answer: wire::Answer,
) -> Result<Vec<Witness>, Refusal> {
    if !matches!(answer, wire::Answer::Granted | wire::Answer::Denied) {
        return Err(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "consent::ruling-invalid",
                Some(pending.request.to_string()),
                Some("granted or denied".to_owned()),
                Some(format!("{answer:?}").to_lowercase()),
            )
            .with_detail("choose allow or deny for the queued request"),
        ));
    }
    let mut witnesses = vec![
        Witness::Consent {
            request: pending.request,
            answer: wire::Answer::Queued,
        },
        Witness::Rule {
            name: pending.rule.clone(),
        },
    ];
    if answer == wire::Answer::Granted
        && let Some(budget) = &pending.budget
    {
        witnesses.push(Witness::Budget {
            rule: budget.rule.clone(),
            remaining: budget.remaining,
        });
    }
    Ok(witnesses)
}

fn policy_decision(answer: Answer) -> ConsentPolicyDecision {
    match answer {
        Answer::Allow => ConsentPolicyDecision::Allow,
        Answer::Deny => ConsentPolicyDecision::Deny,
        Answer::Queue => ConsentPolicyDecision::Queue,
    }
}

fn decode<B: serde::de::DeserializeOwned>(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Result<B, QueueError> {
    let bytes = wire::encode(envelope.fact.body()).map_err(|error| QueueError {
        seq: envelope.seq,
        detail: format!("fact body cannot be encoded: {error}"),
    })?;
    wire::decode(&bytes).map_err(|error| QueueError {
        seq: envelope.seq,
        detail: format!("registered fact body cannot be decoded: {error}"),
    })
}

fn invalid(seq: Seq, request: CanonicalUuid, detail: &str) -> QueueError {
    QueueError {
        seq,
        detail: format!("request {request}: {detail}"),
    }
}

/// A consent fact stream that cannot materialize one deterministic queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueError {
    seq: Seq,
    detail: String,
}

impl fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "consent queue is invalid at seq {}: {}",
            self.seq, self.detail
        )
    }
}

impl Error for QueueError {}

#[cfg(test)]
mod tests {
    use journal::FactSnapshot;
    use wire::body::{ConsentAsked, ConsentGranted, ConsentQueueCause, ConsentQueued};
    use wire::{Actor, AnyFact, Envelope, Fact, Instant, Origin, Seq, WireVersion};

    use super::Queue;
    use crate::{Answer, Policy};

    #[test]
    fn every_queue_and_terminal_fact_is_caused_by_its_current_request_fact() {
        let policy = Policy::new(Answer::Queue, Vec::new()).expect("queue posture is valid");
        let request = uuid(1);
        let lane = uuid(2);
        let asked = envelope(
            1,
            None,
            "request:one",
            Fact::new(ConsentAsked {
                request,
                lane,
                operation: "deploy:one".to_owned(),
            }),
        );
        let queued = envelope(
            2,
            None,
            "request:one",
            Fact::new(ConsentQueued {
                request,
                lane,
                rule: "posture".to_owned(),
                cause: ConsentQueueCause::Policy {},
            }),
        );
        let snapshot = FactSnapshot::from_facts(vec![asked.clone(), queued])
            .expect("contiguous malformed fixture");
        assert!(
            Queue::from_snapshot(&snapshot, &policy)
                .expect_err("a queue without its ask cause must fail")
                .to_string()
                .contains("queue cause is not its ask")
        );

        let queued = envelope(
            2,
            Some(1),
            "request:one",
            Fact::new(ConsentQueued {
                request,
                lane,
                rule: "posture".to_owned(),
                cause: ConsentQueueCause::Policy {},
            }),
        );
        let granted = envelope(
            3,
            Some(1),
            "request:one",
            Fact::new(ConsentGranted {
                request,
                lane,
                rule: "operator".to_owned(),
                budget: None,
            }),
        );
        let snapshot = FactSnapshot::from_facts(vec![asked, queued, granted])
            .expect("contiguous malformed fixture");
        assert!(
            Queue::from_snapshot(&snapshot, &policy)
                .expect_err("a terminal ruling must point at its queue fact")
                .to_string()
                .contains("terminal cause is not its current request disposition")
        );
    }

    #[test]
    fn a_committed_suffix_advances_to_the_same_queue_as_restart_replay() {
        let policy = Policy::new(Answer::Queue, Vec::new()).expect("queue posture is valid");
        let request = uuid(1);
        let lane = uuid(2);
        let asked = envelope(
            1,
            None,
            "request:one",
            Fact::new(ConsentAsked {
                request,
                lane,
                operation: "deploy:one".to_owned(),
            }),
        );
        let queued = envelope(
            2,
            Some(1),
            "request:one",
            Fact::new(ConsentQueued {
                request,
                lane,
                rule: "posture".to_owned(),
                cause: ConsentQueueCause::Policy {},
            }),
        );
        let granted = envelope(
            3,
            Some(2),
            "request:one",
            Fact::new(ConsentGranted {
                request,
                lane,
                rule: "operator".to_owned(),
                budget: None,
            }),
        );
        let restarted = Queue::from_snapshot(
            &FactSnapshot::from_facts(vec![asked.clone(), queued.clone(), granted.clone()])
                .expect("restart fixture is contiguous"),
            &policy,
        )
        .expect("restart queue replays");
        let mut incremental = Queue::from_snapshot(
            &FactSnapshot::from_facts(vec![asked, queued]).expect("prefix is contiguous"),
            &policy,
        )
        .expect("prefix queue replays");
        incremental
            .advance([&granted])
            .expect("one committed suffix advances");
        assert_eq!(incremental, restarted);
    }

    fn envelope<B: wire::Body + serde::Serialize>(
        seq: u64,
        cause: Option<u64>,
        origin: &str,
        fact: Fact<B>,
    ) -> Envelope<AnyFact> {
        let fact = wire::decode(&wire::encode(&fact).expect("typed fact encodes"))
            .expect("typed fact decodes as opaque");
        Envelope {
            v: WireVersion::CURRENT,
            seq: Seq::new(seq).expect("nonzero sequence"),
            at: "2030-04-12T18:44:09.271Z"
                .parse::<Instant>()
                .expect("fixture instant"),
            actor: Actor::Spice,
            origin: origin.parse::<Origin>().expect("fixture origin"),
            cause: cause.map(|value| Seq::new(value).expect("nonzero cause")),
            fact,
        }
    }

    fn uuid(last: u8) -> wire::CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }
}
