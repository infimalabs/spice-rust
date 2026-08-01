//! Sealed appliance verification.
//!
//! [`container`] establishes the network and cache boundary, [`replay`] runs
//! the complete generated battery inside it, and [`evidence`] binds that
//! replay to the receipt-free target payload before publishing its detached
//! receipt atomically.

#![forbid(unsafe_code)]

pub mod container;
pub mod evidence;
pub mod replay;

pub use container::{Container, ContainerError, Limits, OciRuntime, RuntimeKind};
pub use evidence::{EvidenceError, EvidencePublishError, VerifiedEvidence, publish, verify};
pub use replay::{
    Battery, Check, CheckKind, CheckStanding, CheckVerdict, ReplayError, ReplayVerdict, replay,
};
