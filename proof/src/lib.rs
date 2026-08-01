//! Sealed appliance verification.
//!
//! [`layout`] resolves the one payload and receipt, [`container`] establishes
//! the network and cache boundary, [`replay`] runs the complete generated
//! battery inside it, and [`evidence`] binds that replay to the receipt-free
//! target payload before publishing its detached receipt atomically.

#![forbid(unsafe_code)]

mod challenge;
pub mod config;
pub mod container;
pub mod environment;
pub mod evidence;
pub mod layout;
pub mod native;
pub mod payload;
pub mod replay;

pub use config::{Config, DEFAULT_IMAGE, ExecutablePath, Image};
pub use container::{Container, ContainerError, Limits, OciRuntime};
pub use environment::{EnvironmentError, SealedEnvironment, UnsupportedHost};
pub use evidence::{EvidenceError, EvidencePublishError, VerifiedEvidence, publish, verify};
pub use layout::{Input, InputError, Layout, LayoutError, ReceiptInput};
pub use native::{NativeError, NativeRunner};
pub use payload::{AdmittedPayload, PayloadError, admit};
pub use replay::{
    Battery, Check, CheckKind, CheckStanding, CheckVerdict, ReceiptError, ReplayError,
    ReplayVerdict, replay,
};
