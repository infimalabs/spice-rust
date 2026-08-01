use std::collections::BTreeMap;

use wire::CanonicalUuid;
use wire::body::{ConsentAsked, ConsentQueueCause, ConsentQueued};

/// Whether the action a request covers can be taken back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reversibility {
    /// The action can be undone, so it need not wait on a pending decision.
    Reversible,
    /// The action cannot be undone, so it waits until the operator decides.
    Irreversible,
}

/// One request held for the operator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Held {
    lane: CanonicalUuid,
    operation: String,
    reversibility: Reversibility,
    rule: String,
    cause: ConsentQueueCause,
}

impl Held {
    /// Lane the held request came from.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// Operation the operator is being asked about.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Whether the held action can be taken back.
    #[must_use]
    pub const fn reversibility(&self) -> Reversibility {
        self.reversibility
    }

    /// Rule or posture that queued this request.
    #[must_use]
    pub fn rule(&self) -> &str {
        &self.rule
    }

    /// Whether policy asked directly or a named budget requires renewal.
    #[must_use]
    pub const fn cause(&self) -> ConsentQueueCause {
        self.cause
    }
}

/// Requests pended to the operator, keyed by request identity.
///
/// Holding is not blocking. A lane with requests outstanding keeps doing
/// reversible work, and only the irreversible action a request covers waits.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pending {
    held: BTreeMap<CanonicalUuid, Held>,
}

impl Pending {
    /// A queue holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold this request for the operator and state the fact that says so.
    pub fn hold(
        &mut self,
        asked: &ConsentAsked,
        reversibility: Reversibility,
        rule: String,
        cause: ConsentQueueCause,
    ) -> ConsentQueued {
        let queued = ConsentQueued {
            request: asked.request,
            lane: asked.lane,
            rule,
            cause,
        };
        self.held.insert(
            asked.request,
            Held {
                lane: asked.lane,
                operation: asked.operation.clone(),
                reversibility,
                rule: queued.rule.clone(),
                cause,
            },
        );
        queued
    }

    /// Release a decided request, yielding what was held.
    pub fn release(&mut self, request: CanonicalUuid) -> Option<Held> {
        self.held.remove(&request)
    }

    /// Whether this request is still waiting on the operator.
    #[must_use]
    pub fn holds(&self, request: CanonicalUuid) -> bool {
        self.held.contains_key(&request)
    }

    /// The request held under this identity, if it is still pending.
    #[must_use]
    pub fn get(&self, request: CanonicalUuid) -> Option<&Held> {
        self.held.get(&request)
    }

    /// Whether this request's action proceeds while requests are held.
    ///
    /// Reversible work always proceeds, including while the queue is full,
    /// which is the whole point of queueing rather than denying. An
    /// irreversible action waits on its own held request and on no other,
    /// because there is no undo to fall back on if the operator later refuses
    /// this one. A decision pending about some other operation is not an answer
    /// about this one, and treating it as one would make the first queued
    /// request a global halt on every lane.
    #[must_use]
    pub fn proceeds(&self, request: CanonicalUuid, reversibility: Reversibility) -> bool {
        match reversibility {
            Reversibility::Reversible => true,
            Reversibility::Irreversible => !self.holds(request),
        }
    }

    /// Held requests in request order.
    pub fn iter(&self) -> impl Iterator<Item = (CanonicalUuid, &Held)> {
        self.held.iter().map(|(request, held)| (*request, held))
    }

    /// How many requests are waiting on the operator.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing is waiting on the operator.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}
