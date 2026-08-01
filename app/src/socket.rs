//! The browser's end of the socket: one current opening and one frame door.
//!
//! The app has no independent schema clock. It is compiled from the same wire
//! crate as the resident that serves it, so an opening carries
//! [`WireVersion::CURRENT`] and every inbound frame is admitted against it.
//! Nothing here decides authority; admission only decides whether bytes are
//! allowed to reach browser state.

use serde::de::DeserializeOwned;
use wire::{CanonicalUuid, Fault, FaultClass, Hello, Response, WireVersion};

/// Reasons this door names when it turns a frame away.
mod reason {
    /// The bytes did not decode as the current DTO.
    pub const UNDECODABLE: &str = "frame::undecodable";
    /// The frame carried a version this build does not speak.
    pub const WIRE: &str = "frame::wire-mismatch";
    /// The frame answered a generation this session does not address.
    pub const EPOCH: &str = "frame::epoch-mismatch";
    /// The discovery file now presents a different capability.
    pub const CAPABILITY: &str = "session::capability-replaced";
}

/// One capability-scoped connection to one resident generation.
///
/// A session is the smallest thing that can be stale. Each `spice up` replaces
/// endpoint, capability, and epoch together, so a session never outlives the
/// generation it opened against and never silently adopts a new capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    capability: CanonicalUuid,
    epoch: CanonicalUuid,
}

impl Session {
    /// Open a session against one resident generation.
    #[must_use]
    pub const fn open(capability: CanonicalUuid, epoch: CanonicalUuid) -> Self {
        Self { capability, epoch }
    }

    /// The generation this session addresses.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// The opening frame, always at the version this build speaks.
    ///
    /// The capability crosses only inside this payload. It never reaches a
    /// URL, query string, fragment, or cookie, so browser history and logs
    /// cannot leak it.
    #[must_use]
    pub const fn hello(&self) -> Hello {
        Hello {
            wire: WireVersion::CURRENT,
            capability: self.capability,
            epoch: self.epoch,
        }
    }

    /// Admit a capability offered by a later discovery read.
    ///
    /// A replaced capability means a new resident generation, not a refreshed
    /// credential for this one. Reporting it as a fault keeps the app from
    /// reusing state that belongs to a generation that no longer exists.
    ///
    /// # Errors
    ///
    /// Returns a protocol fault when `presented` is not this session's
    /// capability.
    pub fn admit_capability(&self, presented: CanonicalUuid) -> Result<(), Fault> {
        if presented == self.capability {
            return Ok(());
        }
        Err(Fault {
            class: FaultClass::Protocol,
            reason: reason::CAPABILITY.to_owned(),
            detail: "discovery presented a capability this session did not open with; \
                     open a new session rather than reusing this one"
                .to_owned(),
        })
    }

    /// Decode one inbound frame and admit it against this session.
    ///
    /// Undecodable bytes, a version this build does not speak, and an epoch
    /// that has moved are all protocol faults. They are reported rather than
    /// repaired: there is no upcast path and no second decoder.
    ///
    /// # Errors
    ///
    /// Returns a protocol fault naming which of the three admissions failed.
    pub fn admit_frame<B: DeserializeOwned>(&self, bytes: &[u8]) -> Result<Response<B>, Fault> {
        let frame: Response<B> = wire::decode(bytes).map_err(|error| Fault {
            class: FaultClass::Protocol,
            reason: reason::UNDECODABLE.to_owned(),
            detail: error.to_string(),
        })?;
        if !frame.v.is_current() {
            return Err(Fault {
                class: FaultClass::Protocol,
                reason: reason::WIRE.to_owned(),
                detail: format!(
                    "frame carried wire {}; this build speaks {}",
                    frame.v,
                    WireVersion::CURRENT
                ),
            });
        }
        if frame.epoch != self.epoch {
            return Err(Fault {
                class: FaultClass::Protocol,
                reason: reason::EPOCH.to_owned(),
                detail: format!(
                    "frame answered epoch {}; this session addresses {}",
                    frame.epoch, self.epoch
                ),
            });
        }
        Ok(frame)
    }
}
