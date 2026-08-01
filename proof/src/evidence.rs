//! Verification and atomic publication of detached proof evidence.

use std::error::Error;
use std::fmt;
use std::io::Write as _;
use std::path::Path;

use wire::manifest::{ProofReceipt, ProofStanding, ReleaseTarget, TargetPayload};
use wire::{Digest, Domain, ProofEvidence};

use crate::container::is_digest_pinned;
use crate::replay::{Battery, ReplayVerdict};

const PAYLOAD_VERSION: u16 = 1;
const RECEIPT_VERSION: u16 = 1;

/// Evidence whose payload, replay, and receipt agree in every bound identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEvidence {
    target: ReleaseTarget,
    payload: Digest,
    receipt: Digest,
    battery: Digest,
    replay: Digest,
}

impl VerifiedEvidence {
    #[must_use]
    pub const fn target(&self) -> ReleaseTarget {
        self.target
    }

    #[must_use]
    pub const fn payload_digest(&self) -> &Digest {
        &self.payload
    }

    #[must_use]
    pub const fn receipt_digest(&self) -> &Digest {
        &self.receipt
    }

    #[must_use]
    pub const fn battery_digest(&self) -> &Digest {
        &self.battery
    }

    #[must_use]
    pub const fn replay_digest(&self) -> &Digest {
        &self.replay
    }
}

impl From<VerifiedEvidence> for ProofEvidence {
    fn from(verified: VerifiedEvidence) -> Self {
        Self {
            target: verified.target,
            payload: verified.payload,
            receipt: verified.receipt,
            battery: verified.battery,
            replay: verified.replay,
        }
    }
}

/// Verify the current target payload and detached receipt against one replay.
///
/// # Errors
///
/// Refuses noncanonical wire bytes, stale schemas, internal payload digest
/// disagreements, incomplete artifact inventories, refused replays, or any
/// receipt field that does not bind the supplied payload and replay exactly.
pub fn verify(
    payload_bytes: &[u8],
    receipt_bytes: &[u8],
    replay: &ReplayVerdict,
) -> Result<VerifiedEvidence, EvidenceError> {
    let payload: TargetPayload = wire::decode(payload_bytes)
        .map_err(|source| EvidenceError::DecodePayload(source.to_string()))?;
    let receipt: ProofReceipt = wire::decode(receipt_bytes)
        .map_err(|source| EvidenceError::DecodeReceipt(source.to_string()))?;
    let canonical_payload =
        wire::encode(&payload).map_err(|source| EvidenceError::Encode(source.to_string()))?;
    let canonical_receipt =
        wire::encode(&receipt).map_err(|source| EvidenceError::Encode(source.to_string()))?;
    if canonical_payload != payload_bytes {
        return Err(EvidenceError::NonCanonicalPayload);
    }
    if canonical_receipt != receipt_bytes {
        return Err(EvidenceError::NonCanonicalReceipt);
    }
    if payload.version != PAYLOAD_VERSION {
        return Err(EvidenceError::PayloadVersion(payload.version));
    }
    if receipt.version != RECEIPT_VERSION {
        return Err(EvidenceError::ReceiptVersion(receipt.version));
    }
    if !is_digest_pinned(&payload.pins.proof_container) {
        return Err(EvidenceError::UnpinnedEnvironment(
            payload.pins.proof_container.clone(),
        ));
    }

    let pins_bytes =
        wire::encode(&payload.pins).map_err(|source| EvidenceError::Encode(source.to_string()))?;
    let pins_digest = Digest::compute(Domain::DistPins, &pins_bytes);
    if payload.pins_digest != pins_digest {
        return Err(EvidenceError::PinsDigest);
    }
    let inventory_bytes = wire::encode(&payload.inventory)
        .map_err(|source| EvidenceError::Encode(source.to_string()))?;
    let inventory_digest = Digest::compute(Domain::DistInventory, &inventory_bytes);
    if payload.inventory_digest != inventory_digest {
        return Err(EvidenceError::InventoryDigest);
    }
    for pair in payload.inventory.windows(2) {
        if pair[0].path >= pair[1].path {
            return Err(EvidenceError::InventoryOrder(pair[1].path.clone()));
        }
    }
    verify_artifact(&payload, "app/app.wasm", 0o644, &payload.app)?;
    verify_artifact(&payload, "embed/app.bin", 0o644, &payload.embedded_app)?;
    verify_artifact(&payload, "bin/spice", 0o755, &payload.native)?;
    if payload.inventory.len() != 3 {
        return Err(EvidenceError::InventoryLength(payload.inventory.len()));
    }

    let payload_digest = Digest::compute(Domain::DistTargetPayload, &canonical_payload);
    if receipt.target != payload.target {
        return Err(EvidenceError::Target {
            payload: payload.target,
            receipt: receipt.target,
        });
    }
    if receipt.payload != payload_digest {
        return Err(EvidenceError::PayloadDigest);
    }
    if receipt.environment != payload.pins.proof_container
        || receipt.environment != replay.environment()
    {
        return Err(EvidenceError::Environment {
            payload: payload.pins.proof_container,
            receipt: receipt.environment,
            replay: replay.environment().to_owned(),
        });
    }
    if receipt.battery != *replay.battery_digest()
        || receipt.battery != *Battery::current().digest()
    {
        return Err(EvidenceError::BatteryDigest);
    }
    if receipt.replay != *replay.replay_digest() {
        return Err(EvidenceError::ReplayDigest);
    }
    if receipt.standing != replay.standing() {
        return Err(EvidenceError::Standing);
    }
    if receipt.standing != ProofStanding::Clean {
        return Err(EvidenceError::Refused);
    }

    let receipt_digest = Digest::compute(Domain::ProofReceipt, &canonical_receipt);
    Ok(VerifiedEvidence {
        target: payload.target,
        payload: payload_digest,
        receipt: receipt_digest,
        battery: receipt.battery,
        replay: receipt.replay,
    })
}

/// Publish one current receipt atomically and return its canonical identity.
///
/// # Errors
///
/// Returns the encoding or durable publication failure. No alternate receipt
/// format or partial destination is left behind.
pub fn publish(receipt: &ProofReceipt, destination: &Path) -> Result<Digest, EvidencePublishError> {
    let bytes = wire::encode(receipt).map_err(|source| EvidencePublishError::Encode {
        source: source.to_string(),
    })?;
    atomic::Destination::new(destination)
        .map_err(EvidencePublishError::Publish)?
        .publish(|staged| staged.write_all(&bytes))
        .map_err(EvidencePublishError::Publish)?;
    Ok(Digest::compute(Domain::ProofReceipt, &bytes))
}

fn verify_artifact(
    payload: &TargetPayload,
    path: &'static str,
    mode: u32,
    digest: &Digest,
) -> Result<(), EvidenceError> {
    let Some(entry) = payload.inventory.iter().find(|entry| entry.path == path) else {
        return Err(EvidenceError::MissingArtifact(path));
    };
    if entry.mode != mode {
        return Err(EvidenceError::ArtifactMode {
            path,
            expected: mode,
            observed: entry.mode,
        });
    }
    if &entry.digest != digest {
        return Err(EvidenceError::ArtifactDigest(path));
    }
    Ok(())
}

/// Why payload, replay, and receipt did not form one proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceError {
    DecodePayload(String),
    DecodeReceipt(String),
    Encode(String),
    NonCanonicalPayload,
    NonCanonicalReceipt,
    PayloadVersion(u16),
    ReceiptVersion(u16),
    UnpinnedEnvironment(String),
    PinsDigest,
    InventoryDigest,
    InventoryOrder(String),
    InventoryLength(usize),
    MissingArtifact(&'static str),
    ArtifactMode {
        path: &'static str,
        expected: u32,
        observed: u32,
    },
    ArtifactDigest(&'static str),
    Target {
        payload: ReleaseTarget,
        receipt: ReleaseTarget,
    },
    PayloadDigest,
    Environment {
        payload: String,
        receipt: String,
        replay: String,
    },
    BatteryDigest,
    ReplayDigest,
    Standing,
    Refused,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecodePayload(source) => {
                write!(formatter, "target payload does not decode: {source}")
            }
            Self::DecodeReceipt(source) => {
                write!(formatter, "proof receipt does not decode: {source}")
            }
            Self::Encode(source) => write!(formatter, "proof evidence does not encode: {source}"),
            Self::NonCanonicalPayload => {
                formatter.write_str("target payload bytes are not canonical")
            }
            Self::NonCanonicalReceipt => {
                formatter.write_str("proof receipt bytes are not canonical")
            }
            Self::PayloadVersion(version) => {
                write!(formatter, "target payload version {version} is not current")
            }
            Self::ReceiptVersion(version) => {
                write!(formatter, "proof receipt version {version} is not current")
            }
            Self::UnpinnedEnvironment(environment) => write!(
                formatter,
                "target payload proof environment is not digest-pinned: {environment:?}"
            ),
            Self::PinsDigest => {
                formatter.write_str("target payload pin digest disagrees with its pins")
            }
            Self::InventoryDigest => {
                formatter.write_str("target payload inventory digest disagrees with its inventory")
            }
            Self::InventoryOrder(path) => write!(
                formatter,
                "target payload inventory is not strictly ordered at {path:?}"
            ),
            Self::InventoryLength(length) => write!(
                formatter,
                "target payload inventory contains {length} entries instead of 3"
            ),
            Self::MissingArtifact(path) => {
                write!(formatter, "target payload inventory is missing {path}")
            }
            Self::ArtifactMode {
                path,
                expected,
                observed,
            } => write!(
                formatter,
                "target payload artifact {path} has mode {observed:04o}, expected {expected:04o}"
            ),
            Self::ArtifactDigest(path) => write!(
                formatter,
                "target payload artifact {path} disagrees with its named digest"
            ),
            Self::Target { payload, receipt } => write!(
                formatter,
                "proof target {receipt} does not match payload target {payload}"
            ),
            Self::PayloadDigest => {
                formatter.write_str("proof receipt does not bind the supplied target payload")
            }
            Self::Environment {
                payload,
                receipt,
                replay,
            } => write!(
                formatter,
                "proof environment disagrees: payload={payload:?} receipt={receipt:?} replay={replay:?}"
            ),
            Self::BatteryDigest => {
                formatter.write_str("proof receipt does not bind the replay battery")
            }
            Self::ReplayDigest => {
                formatter.write_str("proof receipt does not bind the replay evidence")
            }
            Self::Standing => {
                formatter.write_str("proof receipt standing disagrees with the replay")
            }
            Self::Refused => {
                formatter.write_str("refused proof evidence cannot be verified as a release proof")
            }
        }
    }
}

impl Error for EvidenceError {}

/// Why a detached receipt did not reach its destination.
#[derive(Debug)]
pub enum EvidencePublishError {
    Encode { source: String },
    Publish(atomic::Error),
}

impl fmt::Display for EvidencePublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode { source } => write!(formatter, "proof receipt encoding failed: {source}"),
            Self::Publish(source) => {
                write!(formatter, "proof receipt publication failed: {source}")
            }
        }
    }
}

impl Error for EvidencePublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode { .. } => None,
            Self::Publish(source) => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::manifest::{BuildPins, InventoryEntry, ReleaseTarget, TargetPayload};

    use super::{EvidenceError, publish, verify};
    use crate::replay::{Battery, ReplayVerdict};
    use wire::{Digest, Domain, ProofEvidence};

    #[test]
    fn current_payload_replay_and_receipt_verify_as_one_identity_chain() {
        let payload = payload();
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let replay = clean_replay(&payload);
        let receipt = replay.receipt(
            &payload,
            Digest::compute(Domain::DistTargetPayload, &payload_bytes),
        );
        let receipt_bytes = wire::encode(&receipt).expect("receipt encodes");

        let verified = verify(&payload_bytes, &receipt_bytes, &replay).expect("evidence verifies");

        let result: ProofEvidence = verified.clone().into();

        assert_eq!(verified.target(), ReleaseTarget::X86_64UnknownLinuxMusl);
        assert_eq!(verified.battery_digest(), replay.battery_digest());
        assert_eq!(verified.replay_digest(), replay.replay_digest());
        assert_eq!(result.target, verified.target());
        assert_eq!(result.payload, *verified.payload_digest());
        assert_eq!(result.receipt, *verified.receipt_digest());
        assert_eq!(result.battery, *verified.battery_digest());
        assert_eq!(result.replay, *verified.replay_digest());
    }

    #[test]
    fn a_receipt_over_another_payload_refuses_by_the_mismatched_identity() {
        let payload = payload();
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let replay = clean_replay(&payload);
        let receipt = replay.receipt(
            &payload,
            Digest::compute(Domain::DistTargetPayload, b"another payload"),
        );

        assert_eq!(
            verify(
                &payload_bytes,
                &wire::encode(&receipt).expect("receipt encodes"),
                &replay,
            ),
            Err(EvidenceError::PayloadDigest)
        );
    }

    #[test]
    fn every_detached_receipt_binding_is_checked_independently() {
        let payload = payload();
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let replay = clean_replay(&payload);
        let receipt = replay.receipt(
            &payload,
            Digest::compute(Domain::DistTargetPayload, &payload_bytes),
        );
        let other = |domain| Digest::compute(domain, b"other evidence");

        let mut wrong_target = receipt.clone();
        wrong_target.target = ReleaseTarget::Aarch64AppleDarwin;
        assert_refuses(
            &payload_bytes,
            &wrong_target,
            &replay,
            EvidenceError::Target {
                payload: ReleaseTarget::X86_64UnknownLinuxMusl,
                receipt: ReleaseTarget::Aarch64AppleDarwin,
            },
        );

        let mut wrong_battery = receipt.clone();
        wrong_battery.battery = other(Domain::ProofBattery);
        assert_refuses(
            &payload_bytes,
            &wrong_battery,
            &replay,
            EvidenceError::BatteryDigest,
        );

        let mut wrong_replay = receipt.clone();
        wrong_replay.replay = other(Domain::ProofReplay);
        assert_refuses(
            &payload_bytes,
            &wrong_replay,
            &replay,
            EvidenceError::ReplayDigest,
        );

        let mut wrong_environment = receipt;
        wrong_environment.environment = format!("registry.example/other@sha256:{}", "5".repeat(64));
        assert!(matches!(
            verify(
                &payload_bytes,
                &wire::encode(&wrong_environment).expect("receipt encodes"),
                &replay,
            ),
            Err(EvidenceError::Environment { .. })
        ));
    }

    #[test]
    fn receipt_publication_is_atomic_and_returns_the_published_identity() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-proof-evidence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture directory");
        let payload = payload();
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let replay = clean_replay(&payload);
        let receipt = replay.receipt(
            &payload,
            Digest::compute(Domain::DistTargetPayload, &payload_bytes),
        );
        let destination = root.join("receipt.json");

        let digest = publish(&receipt, &destination).expect("receipt publishes");
        let published = fs::read(&destination).expect("published receipt");

        assert_eq!(digest, Digest::compute(Domain::ProofReceipt, &published));
        assert_eq!(
            wire::decode::<wire::manifest::ProofReceipt>(&published)
                .expect("published receipt decodes"),
            receipt
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    fn payload() -> TargetPayload {
        let environment = format!("registry.example/proof@sha256:{}", "4".repeat(64));
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: environment,
        };
        let app = Digest::compute(Domain::DistArtifact, b"app");
        let embedded = Digest::compute(Domain::DistArtifact, b"embedded");
        let native = Digest::compute(Domain::DistArtifact, b"native");
        let inventory = vec![
            InventoryEntry {
                path: "app/app.wasm".to_owned(),
                mode: 0o644,
                bytes: 3,
                digest: app.clone(),
            },
            InventoryEntry {
                path: "bin/spice".to_owned(),
                mode: 0o755,
                bytes: 6,
                digest: native.clone(),
            },
            InventoryEntry {
                path: "embed/app.bin".to_owned(),
                mode: 0o644,
                bytes: 8,
                digest: embedded.clone(),
            },
        ];
        TargetPayload {
            version: 1,
            target: ReleaseTarget::X86_64UnknownLinuxMusl,
            source: Digest::compute(Domain::DistArtifact, b"source"),
            app,
            embedded_app: embedded,
            native,
            pins_digest: Digest::compute(
                Domain::DistPins,
                &wire::encode(&pins).expect("pins encode"),
            ),
            inventory_digest: Digest::compute(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            pins,
            inventory,
        }
    }

    fn clean_replay(payload: &TargetPayload) -> ReplayVerdict {
        let battery = Battery::current();
        ReplayVerdict::fixture_clean(
            payload.pins.proof_container.clone(),
            battery.digest().clone(),
        )
    }

    fn assert_refuses(
        payload_bytes: &[u8],
        receipt: &wire::manifest::ProofReceipt,
        replay: &ReplayVerdict,
        expected: EvidenceError,
    ) {
        assert_eq!(
            verify(
                payload_bytes,
                &wire::encode(receipt).expect("receipt encodes"),
                replay,
            ),
            Err(expected)
        );
    }
}
