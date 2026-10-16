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
    use wire::manifest::{InventoryEntry, MacSymbolMetadata, ReleaseTarget};
    use wire::{Digest, Domain};

    use crate::container::{Captured, Execution, ExecutionCompletion};

    /// The arguments a bounded process spec hands its program, as text.
    pub(crate) fn arguments(spec: &sill::Spec) -> Vec<String> {
        spec.argv
            .arguments()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    /// A process that exited with `code` after writing both streams whole.
    pub(crate) fn exited(
        code: i32,
        stdout: impl Into<Vec<u8>>,
        stderr: impl Into<Vec<u8>>,
    ) -> Execution {
        Execution {
            completion: ExecutionCompletion::Exited {
                success: code == 0,
                code: Some(code),
            },
            stdout: whole(stdout),
            stderr: whole(stderr),
        }
    }

    pub(crate) fn whole(bytes: impl Into<Vec<u8>>) -> Captured {
        Captured {
            bytes: bytes.into(),
            truncated: false,
            complete: true,
        }
    }

    /// A regular executable file that admission can inspect and digest.
    pub(crate) fn executable(path: &std::path::Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("fixture executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("make fixture executable");
        }
    }

    pub(crate) fn symbols(
        target: ReleaseTarget,
        native: &Digest,
        inventory: &mut Vec<InventoryEntry>,
    ) -> Option<MacSymbolMetadata> {
        (target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
            let metadata = MacSymbolMetadata {
                path: "symbols/spice.dSYM.json".to_owned(),
                mode: 0o644,
                bytes: 14,
                digest: Digest::compute(Domain::DistArtifact, b"symbol fixture"),
                uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
                executable: native.clone(),
            };
            inventory.push(InventoryEntry {
                path: metadata.path.clone(),
                mode: metadata.mode,
                bytes: metadata.bytes,
                digest: metadata.digest.clone(),
            });
            metadata
        })
    }
}

pub mod challenge;
pub mod config;
pub mod container;
pub mod environment;
pub mod evidence;
mod launch;
pub mod layout;
pub mod manifest;
pub mod native;
pub mod package;
pub mod payload;
pub mod rederive;
pub mod replay;
pub mod seal;

pub use config::{Config, DEFAULT_IMAGE, ExecutablePath, Image};
pub use container::{Container, ContainerError, Limits, OciRuntime};
pub use environment::{EnvironmentError, SealedEnvironment, UnsupportedHost, compiled_target};
pub use evidence::{
    EvidenceError, EvidencePublishError, VerifiedEvidence, publish, verify, verify_stored,
};
pub use layout::{Input, InputError, Layout, LayoutError, ReceiptInput};
pub use manifest::{
    FileManifestPublisher, ManifestAdmissionError, ManifestError, ManifestInput,
    ManifestPublishError, ManifestPublisher, PublishedManifest, admit_manifest,
    admit_proof_manifest, publish_manifest,
};
pub use native::{NativeError, NativeRunner};
pub use package::{
    AdmittedPackage, PACKAGE_DOCUMENT_LIMIT, PackageError, PackageInput, PackageMember,
    PackageTargetInput, PackageTree, admit_package_tree, build_package, build_package_index,
};
pub use payload::{AdmittedPayload, PayloadError, admit};
pub use rederive::{RederiveError, rederive};
pub use replay::{
    Battery, Check, CheckKind, CheckStanding, CheckVerdict, ReceiptError, ReplayError,
    ReplayVerdict, replay,
};
pub use seal::{ReplayRefusal, SealError, seal};
