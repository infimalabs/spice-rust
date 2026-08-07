//! Sealed appliance verification.
//!
//! [`layout`] resolves the one payload and receipt, [`container`] establishes
//! the network and cache boundary, [`replay`](mod@replay) runs the complete
//! generated battery inside it, and [`evidence`] binds that replay to the
//! receipt-free target payload before publishing its detached receipt
//! atomically.

#![forbid(unsafe_code)]

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

    static EXECUTABLE_FIXTURE_BARRIER: RwLock<()> = RwLock::new(());

    pub(crate) fn process_spawn_guard() -> RwLockReadGuard<'static, ()> {
        EXECUTABLE_FIXTURE_BARRIER
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn executable_write_guard() -> RwLockWriteGuard<'static, ()> {
        EXECUTABLE_FIXTURE_BARRIER
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

pub mod challenge;
pub mod config;
pub mod container;
pub mod environment;
pub mod evidence;
pub mod layout;
pub mod manifest;
pub mod native;
pub mod payload;
pub mod rederive;
pub mod replay;
pub mod seal;

pub use config::{Config, DEFAULT_IMAGE, ExecutablePath, Image};
pub use container::{Container, ContainerError, Limits, OciRuntime};
pub use environment::{EnvironmentError, SealedEnvironment, UnsupportedHost, compiled_target};
pub use evidence::{EvidenceError, EvidencePublishError, VerifiedEvidence, publish, verify};
pub use layout::{Input, InputError, Layout, LayoutError, ReceiptInput};
pub use manifest::{
    FileManifestPublisher, ManifestAdmissionError, ManifestError, ManifestInput,
    ManifestPublishError, ManifestPublisher, PublishedManifest, admit_manifest, publish_manifest,
};
pub use native::{NativeError, NativeRunner};
pub use payload::{AdmittedPayload, PayloadError, admit};
pub use rederive::{RederiveError, rederive};
pub use replay::{
    Battery, Check, CheckKind, CheckStanding, CheckVerdict, ReceiptError, ReplayError,
    ReplayVerdict, replay,
};
pub use seal::{ReplayRefusal, SealError, seal};
