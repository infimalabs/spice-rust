//! Capability registration and server-side principal derivation.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, RwLock};

/// Closed capability-to-principal map for one resident generation.
#[derive(Clone, Default)]
pub struct Capabilities {
    principals: Arc<RwLock<BTreeMap<wire::CanonicalUuid, wire::Actor>>>,
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Capabilities")
            .field(
                "principals",
                &self
                    .principals
                    .read()
                    .map_or(0, |principals| principals.len()),
            )
            .finish()
    }
}

impl Capabilities {
    /// Register an operator or lane capability exactly once.
    pub fn insert(
        &self,
        capability: wire::CanonicalUuid,
        actor: wire::Actor,
    ) -> Result<(), CapabilityError> {
        if matches!(actor, wire::Actor::Spice) {
            return Err(CapabilityError::ResidentIsNotInteractive);
        }
        let mut principals = self
            .principals
            .write()
            .map_err(|_| CapabilityError::Unavailable)?;
        match principals.entry(capability) {
            Entry::Vacant(entry) => {
                entry.insert(actor);
                Ok(())
            }
            Entry::Occupied(_) => Err(CapabilityError::Duplicate),
        }
    }

    /// Remove one exact capability mapping during a failed or retired seat.
    pub fn remove(&self, capability: wire::CanonicalUuid, actor: &wire::Actor) -> bool {
        let Ok(mut principals) = self.principals.write() else {
            return false;
        };
        if principals.get(&capability) != Some(actor) {
            return false;
        }
        principals.remove(&capability);
        true
    }

    /// Admit a current hello and derive its principal server-side.
    pub fn admit(
        &self,
        hello: &wire::Hello,
        epoch: wire::CanonicalUuid,
    ) -> Result<wire::Accepted, OpeningError> {
        hello.admit(epoch).map_err(|rejected| match rejected {
            wire::Rejected::Wire(refusal) => OpeningError::Refused(refusal),
            wire::Rejected::Epoch(fault) => OpeningError::Fault(fault),
        })?;
        let actor = self
            .principals
            .read()
            .map_err(|_| {
                OpeningError::Fault(wire::Fault {
                    class: wire::FaultClass::Environment,
                    reason: "canvas::capability-unavailable".to_owned(),
                    detail: "resident capability registry is unavailable".to_owned(),
                })
            })?
            .get(&hello.capability)
            .cloned()
            .ok_or_else(|| {
                OpeningError::Refused(wire::Refusal {
                    reason: "hello::capability-unknown".to_owned(),
                    witness: None,
                    expected: None,
                    observed: None,
                    remedy:
                        "read the current owner-only canvas discovery file and open a new session"
                            .to_owned(),
                })
            })?;
        Ok(wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor,
            epoch,
        })
    }

    pub(super) fn is_operator(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals.read().ok().is_some_and(|principals| {
            principals
                .get(&capability)
                .is_some_and(|actor| matches!(actor, wire::Actor::Operator))
        })
    }

    pub(super) fn matches(&self, capability: wire::CanonicalUuid, actor: &wire::Actor) -> bool {
        self.principals
            .read()
            .ok()
            .is_some_and(|principals| principals.get(&capability) == Some(actor))
    }

    pub(super) fn contains(&self, capability: wire::CanonicalUuid) -> bool {
        self.principals
            .read()
            .ok()
            .is_some_and(|principals| principals.contains_key(&capability))
    }

    pub(super) fn operator(&self) -> Option<wire::CanonicalUuid> {
        let principals = self.principals.read().ok()?;
        let mut operators = principals.iter().filter_map(|(capability, actor)| {
            matches!(actor, wire::Actor::Operator).then_some(*capability)
        });
        let operator = operators.next()?;
        operators.next().is_none().then_some(operator)
    }
}

/// Capability registration was structurally invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    Duplicate,
    ResidentIsNotInteractive,
    NotOperator,
    /// The shared registry could not be acquired.
    Unavailable,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicate => "canvas capability is already registered",
            Self::ResidentIsNotInteractive => "resident actor cannot own an interactive capability",
            Self::NotOperator => "canvas discovery requires an operator capability",
            Self::Unavailable => "canvas capability registry is unavailable",
        })
    }
}

impl Error for CapabilityError {}

/// Opening failed before a capability-scoped session existed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpeningError {
    Refused(wire::Refusal),
    Fault(wire::Fault),
}

impl fmt::Display for OpeningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => {
                write!(formatter, "socket opening refused: {}", refusal.reason)
            }
            Self::Fault(fault) => write!(formatter, "socket opening faulted: {}", fault.reason),
        }
    }
}

impl Error for OpeningError {}
