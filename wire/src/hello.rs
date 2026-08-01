//! The socket opening: one current version, one capability, one epoch.

use serde::{Deserialize, Serialize};

use crate::envelope::WireVersion;
use crate::ident::CanonicalUuid;
use crate::reply::{Fault, FaultClass, Refusal};

/// The first frame of every socket.
///
/// All three fields are required. The socket keeps a current-version-only
/// clock: there is no negotiation and no historical upcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// Socket schema version the client speaks.
    pub wire: WireVersion,
    /// Capability authenticating an operator or lane connection.
    pub capability: CanonicalUuid,
    /// Resident generation the client believes it is addressing.
    pub epoch: CanonicalUuid,
}

/// Why an opening did not begin a session.
///
/// identity-law: non-crossing - caller-local rejection before a session opens.
///
/// The two failures are deliberately different outcomes: a version the
/// resident does not speak is a refusal the client can act on, while an epoch
/// that has moved is a fault about a resident generation that no longer
/// exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    /// The client speaks a version this resident does not.
    Wire(Refusal),
    /// The client addressed a generation this resident has replaced.
    Epoch(Fault),
}

impl Hello {
    /// Admit this opening against the resident's current epoch.
    ///
    /// A wire mismatch refuses by naming both client and resident versions.
    /// An epoch mismatch faults by naming both epochs.
    ///
    /// # Errors
    ///
    /// Returns [`Rejected::Wire`] when the version is not current, and
    /// [`Rejected::Epoch`] when the addressed generation is not the resident's.
    pub fn admit(&self, resident_epoch: CanonicalUuid) -> Result<(), Rejected> {
        if !self.wire.is_current() {
            return Err(Rejected::Wire(Refusal {
                reason: "hello::wire-mismatch".to_owned(),
                witness: None,
                expected: Some(WireVersion::CURRENT.to_string()),
                observed: Some(self.wire.to_string()),
                remedy: "reconnect with a client built from this resident's wire crate".to_owned(),
            }));
        }
        if self.epoch != resident_epoch {
            return Err(Rejected::Epoch(Fault {
                class: FaultClass::Protocol,
                reason: "hello::epoch-mismatch".to_owned(),
                detail: format!(
                    "client addressed epoch {}; resident epoch is {resident_epoch}",
                    self.epoch
                ),
            }));
        }
        Ok(())
    }
}
