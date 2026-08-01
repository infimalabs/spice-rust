use std::collections::BTreeMap;

use wire::CanonicalUuid;
use wire::body::{ConsentAsked, ConsentQueued};

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
    pub fn hold(&mut self, asked: &ConsentAsked, reversibility: Reversibility) -> ConsentQueued {
        self.held.insert(
            asked.request,
            Held {
                lane: asked.lane,
                operation: asked.operation.clone(),
                reversibility,
            },
        );
        ConsentQueued {
            request: asked.request,
            lane: asked.lane,
        }
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

    /// Whether work of this reversibility proceeds while requests are held.
    ///
    /// Reversible work always proceeds, including while the queue is full,
    /// which is the whole point of queueing rather than denying. An
    /// irreversible action proceeds only once nothing is held, because there
    /// is no undo to fall back on if the operator later refuses.
    #[must_use]
    pub fn proceeds(&self, reversibility: Reversibility) -> bool {
        match reversibility {
            Reversibility::Reversible => true,
            Reversibility::Irreversible => self.held.is_empty(),
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
