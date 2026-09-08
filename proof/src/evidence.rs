//! Verification and atomic publication of detached proof evidence.

use std::error::Error;
use std::fmt;
use std::io::Write as _;
use std::path::Path;

use wire::manifest::{
    PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt, ProofStanding, ReleaseTarget,
};
use wire::{Digest, Domain, ProofEvidence};

use crate::payload::AdmittedPayload;
use crate::replay::{Battery, ReplayVerdict};

/// Evidence whose payload, replay, and receipt agree in every bound identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEvidence {
    target: ReleaseTarget,
    payload: Digest,
    receipt: Digest,
    battery: Digest,
    replay: Digest,
    environment: ProofEnvironment,
}

/// One canonical current receipt admitted before replay execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdmittedReceipt {
    value: ProofReceipt,
    digest: Digest,
}

impl AdmittedReceipt {
    pub(crate) const fn digest(&self) -> &Digest {
        &self.digest
    }

    pub(crate) const fn value(&self) -> &ProofReceipt {
        &self.value
    }

    #[cfg(test)]
    pub(crate) fn replace_digest(&mut self, digest: Digest) {
        self.digest = digest;
    }
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

    #[must_use]
    pub const fn environment(&self) -> &ProofEnvironment {
        &self.environment
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

/// Verify one detached receipt against an admitted target payload and replay.
///
/// # Errors
///
/// Payload shape and identity have already crossed [`crate::payload::admit`].
/// This step refuses noncanonical receipt bytes, a stale receipt schema, a
/// refused replay, or any receipt field that does not bind the admitted
/// payload and supplied replay exactly.
pub fn verify(
    admitted: &AdmittedPayload,
    receipt_bytes: &[u8],
    replay: &ReplayVerdict,
) -> Result<VerifiedEvidence, EvidenceError> {
    let receipt = admit_receipt(receipt_bytes)?;
    verify_admitted(admitted, &receipt, replay)
}

/// Verify one stored detached receipt against its admitted payload.
///
/// This is the durable half of [`verify`]: the proof leaf already derived and
/// published the canonical receipt from its complete replay, while the later
/// two-target manifest seal no longer has that process-local replay value. The
/// stored receipt must still bind the exact payload, current battery, selected
/// target environment, and clean standing before it can enter a release.
pub fn verify_stored(
    admitted: &AdmittedPayload,
    receipt_bytes: &[u8],
) -> Result<VerifiedEvidence, EvidenceError> {
    let receipt = admit_receipt(receipt_bytes)?;
    let value = receipt.value();
    let payload = admitted.value();
    if value.target != payload.target {
        return Err(EvidenceError::Target {
            payload: payload.target,
            receipt: value.target,
        });
    }
    if value.payload != *admitted.digest() {
        return Err(EvidenceError::PayloadDigest);
    }
    let environment_matches = match (&value.environment, payload.target) {
        (ProofEnvironment::Oci { image }, ReleaseTarget::X86_64UnknownLinuxMusl) => {
            image.as_str() == payload.pins.proof_container
        }
        (ProofEnvironment::Native { .. }, ReleaseTarget::Aarch64AppleDarwin) => true,
        _ => false,
    };
    if !environment_matches {
        return Err(EvidenceError::Environment {
            target: payload.target,
            payload_pin: payload.pins.proof_container.clone(),
            receipt: value.environment.clone(),
            replay: value.environment.clone(),
        });
    }
    if value.battery != *Battery::current().digest() {
        return Err(EvidenceError::BatteryDigest);
    }
    if value.standing != ProofStanding::Clean {
        return Err(EvidenceError::Refused);
    }
    Ok(VerifiedEvidence {
        target: payload.target,
        payload: admitted.digest().clone(),
        receipt: receipt.digest().clone(),
        battery: value.battery.clone(),
        replay: value.replay.clone(),
        environment: value.environment.clone(),
    })
}

pub(crate) fn admit_receipt(receipt_bytes: &[u8]) -> Result<AdmittedReceipt, EvidenceError> {
    let receipt: ProofReceipt = wire::decode(receipt_bytes)
        .map_err(|source| EvidenceError::DecodeReceipt(source.to_string()))?;
    let canonical_receipt =
        wire::encode(&receipt).map_err(|source| EvidenceError::Encode(source.to_string()))?;
    if canonical_receipt != receipt_bytes {
        return Err(EvidenceError::NonCanonicalReceipt);
    }
    if receipt.version != PROOF_RECEIPT_VERSION {
        return Err(EvidenceError::ReceiptVersion(receipt.version));
    }

    Ok(AdmittedReceipt {
        value: receipt,
        digest: Digest::compute(Domain::ProofReceiptV3, &canonical_receipt),
    })
}

pub(crate) fn verify_admitted(
    admitted: &AdmittedPayload,
    admitted_receipt: &AdmittedReceipt,
    replay: &ReplayVerdict,
) -> Result<VerifiedEvidence, EvidenceError> {
    let receipt = &admitted_receipt.value;
    let payload = admitted.value();
    let payload_digest = admitted.digest();
    if receipt.target != payload.target {
        return Err(EvidenceError::Target {
            payload: payload.target,
            receipt: receipt.target,
        });
    }
    if replay.target() != payload.target {
        return Err(EvidenceError::ReplayTarget {
            payload: payload.target,
            replay: replay.target(),
        });
    }
    if receipt.payload != *payload_digest {
        return Err(EvidenceError::PayloadDigest);
    }
    let target_environment_matches = match (&receipt.environment, payload.target) {
        (ProofEnvironment::Oci { image }, ReleaseTarget::X86_64UnknownLinuxMusl) => {
            image.as_str() == payload.pins.proof_container
        }
        (ProofEnvironment::Native { .. }, ReleaseTarget::Aarch64AppleDarwin) => true,
        (ProofEnvironment::Native { .. }, ReleaseTarget::X86_64UnknownLinuxMusl)
        | (ProofEnvironment::Oci { .. }, ReleaseTarget::Aarch64AppleDarwin) => false,
    };
    if !target_environment_matches || receipt.environment != *replay.environment() {
        return Err(EvidenceError::Environment {
            target: payload.target,
            payload_pin: payload.pins.proof_container.clone(),
            receipt: receipt.environment.clone(),
            replay: replay.environment().clone(),
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

    let rederived_receipt = replay
        .receipt(payload, payload_digest.clone())
        .map_err(|_| EvidenceError::ReplayTarget {
            payload: payload.target,
            replay: replay.target(),
        })?;
    let rederived_bytes = wire::encode(&rederived_receipt)
        .map_err(|source| EvidenceError::Encode(source.to_string()))?;
    if admitted_receipt.digest != Digest::compute(Domain::ProofReceiptV3, &rederived_bytes) {
        return Err(EvidenceError::ReceiptDigest);
    }

    Ok(VerifiedEvidence {
        target: payload.target,
        payload: payload_digest.clone(),
        receipt: admitted_receipt.digest.clone(),
        battery: receipt.battery.clone(),
        replay: receipt.replay.clone(),
        environment: receipt.environment.clone(),
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
    Ok(Digest::compute(Domain::ProofReceiptV3, &bytes))
}

/// Why payload, replay, and receipt did not form one proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceError {
    DecodeReceipt(String),
    Encode(String),
    NonCanonicalReceipt,
    ReceiptVersion(u16),
    Target {
        payload: ReleaseTarget,
        receipt: ReleaseTarget,
    },
    ReplayTarget {
        payload: ReleaseTarget,
        replay: ReleaseTarget,
    },
    PayloadDigest,
    Environment {
        target: ReleaseTarget,
        payload_pin: String,
        receipt: ProofEnvironment,
        replay: ProofEnvironment,
    },
    BatteryDigest,
    ReplayDigest,
    Standing,
    Refused,
    ReceiptDigest,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecodeReceipt(source) => {
                write!(formatter, "proof receipt does not decode: {source}")
            }
            Self::Encode(source) => write!(formatter, "proof evidence does not encode: {source}"),
            Self::NonCanonicalReceipt => {
                formatter.write_str("proof receipt bytes are not canonical")
            }
            Self::ReceiptVersion(version) => {
                write!(formatter, "proof receipt version {version} is not current")
            }
            Self::Target { payload, receipt } => write!(
                formatter,
                "proof target {receipt} does not match payload target {payload}"
            ),
            Self::ReplayTarget { payload, replay } => write!(
                formatter,
                "proof replay target {replay} does not match payload target {payload}"
            ),
            Self::PayloadDigest => {
                formatter.write_str("proof receipt does not bind the supplied target payload")
            }
            Self::Environment {
                target,
                payload_pin,
                receipt,
                replay,
            } => write!(
                formatter,
                concat!(
                    "proof environment disagrees for {}: ",
                    "payload pin={:?} receipt={:?} replay={:?}"
                ),
                target, payload_pin, receipt, replay
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
            Self::ReceiptDigest => formatter
                .write_str("stored proof receipt identity disagrees with re-derived evidence"),
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

    use wire::manifest::{
        BuildPins, InventoryEntry, ProofEnvironment, ReleaseTarget, TargetPayload,
    };

    use super::{EvidenceError, publish, verify};
    use crate::payload::{AdmittedPayload, admit};
    use crate::replay::{Battery, ReplayVerdict};
    use wire::{Digest, Domain, ProofEvidence};

    #[test]
    fn current_payload_replay_and_receipt_verify_as_one_identity_chain() {
        let payload = payload();
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let admitted = admit(&payload_bytes).expect("payload admits before replay");
        let replay = clean_replay(&payload);
        let receipt = replay
            .receipt(
                &payload,
                Digest::compute(Domain::DistTargetPayload, &payload_bytes),
            )
            .expect("matching replay and payload targets");
        let receipt_bytes = wire::encode(&receipt).expect("receipt encodes");
        let verified = verify(&admitted, &receipt_bytes, &replay).expect("evidence verifies");

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
        let admitted = admit(&payload_bytes).expect("payload admits before replay");
        let replay = clean_replay(&payload);
        let receipt = replay
            .receipt(
                &payload,
                Digest::compute(Domain::DistTargetPayload, b"another payload"),
            )
            .expect("matching replay and payload targets");
        assert_eq!(
            verify(
                &admitted,
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
        let admitted = admit(&payload_bytes).expect("payload admits before replay");
        let replay = clean_replay(&payload);
        let receipt = replay
            .receipt(
                &payload,
                Digest::compute(Domain::DistTargetPayload, &payload_bytes),
            )
            .expect("matching replay and payload targets");
        let other = |domain| Digest::compute(domain, b"other evidence");

        let mut stale_version = receipt.clone();
        stale_version.version = 1;
        assert_refuses(
            &admitted,
            &stale_version,
            &replay,
            EvidenceError::ReceiptVersion(1),
        );

        let mut wrong_target = receipt.clone();
        wrong_target.target = ReleaseTarget::Aarch64AppleDarwin;
        assert_refuses(
            &admitted,
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
            &admitted,
            &wrong_battery,
            &replay,
            EvidenceError::BatteryDigest,
        );

        let mut wrong_replay = receipt.clone();
        wrong_replay.replay = other(Domain::ProofReplayV3);
        assert_refuses(
            &admitted,
            &wrong_replay,
            &replay,
            EvidenceError::ReplayDigest,
        );

        let mut wrong_environment = receipt;
        wrong_environment.environment =
            ProofEnvironment::oci(format!("registry.example/other@sha256:{}", "5".repeat(64)))
                .expect("fixture image is pinned");
        assert!(matches!(
            verify(
                &admitted,
                &wire::encode(&wrong_environment).expect("receipt encodes"),
                &replay,
            ),
            Err(EvidenceError::Environment { .. })
        ));
    }

    #[test]
    fn each_release_target_accepts_only_its_attested_environment_variant() {
        let native = ProofEnvironment::native(Digest::compute(
            Domain::ProofNativeRunner,
            b"approved runner bytes",
        ));
        let darwin_payload = payload_for(ReleaseTarget::Aarch64AppleDarwin);
        let darwin_bytes = wire::encode(&darwin_payload).expect("Darwin payload encodes");
        let darwin_admitted = admit(&darwin_bytes).expect("Darwin payload admits");
        let darwin_replay = clean_replay_in(&darwin_payload, native.clone());
        let darwin_receipt = darwin_replay
            .receipt(
                &darwin_payload,
                Digest::compute(Domain::DistTargetPayload, &darwin_bytes),
            )
            .expect("matching Darwin target");
        assert!(
            verify(
                &darwin_admitted,
                &wire::encode(&darwin_receipt).expect("Darwin receipt encodes"),
                &darwin_replay,
            )
            .is_ok(),
            "Darwin accepts a digest-identified native runner"
        );

        let darwin_oci = clean_replay_in(
            &darwin_payload,
            ProofEnvironment::oci(&darwin_payload.pins.proof_container)
                .expect("fixture image is pinned"),
        );
        let darwin_oci_receipt = darwin_oci
            .receipt(
                &darwin_payload,
                Digest::compute(Domain::DistTargetPayload, &darwin_bytes),
            )
            .expect("matching Darwin target");
        assert!(matches!(
            verify(
                &darwin_admitted,
                &wire::encode(&darwin_oci_receipt).expect("receipt encodes"),
                &darwin_oci,
            ),
            Err(EvidenceError::Environment { .. })
        ));

        let musl_payload = payload();
        let musl_bytes = wire::encode(&musl_payload).expect("musl payload encodes");
        let musl_admitted = admit(&musl_bytes).expect("musl payload admits");
        let musl_native = clean_replay_in(&musl_payload, native);
        let musl_native_receipt = musl_native
            .receipt(
                &musl_payload,
                Digest::compute(Domain::DistTargetPayload, &musl_bytes),
            )
            .expect("matching musl target");
        assert!(matches!(
            verify(
                &musl_admitted,
                &wire::encode(&musl_native_receipt).expect("receipt encodes"),
                &musl_native,
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
        let admitted = admit(&payload_bytes).expect("payload admits before replay");
        let replay = clean_replay(&payload);
        let receipt = replay
            .receipt(
                &payload,
                Digest::compute(Domain::DistTargetPayload, &payload_bytes),
            )
            .expect("matching replay and payload targets");
        let receipt_bytes = wire::encode(&receipt).expect("receipt encodes");
        let _verified =
            verify(&admitted, &receipt_bytes, &replay).expect("evidence verifies before publish");
        let destination = root.join("receipt.json");

        let digest = publish(&receipt, &destination).expect("receipt publishes");
        let published = fs::read(&destination).expect("published receipt");

        assert_eq!(digest, Digest::compute(Domain::ProofReceiptV3, &published));
        assert_eq!(
            wire::decode::<wire::manifest::ProofReceipt>(&published)
                .expect("published receipt decodes"),
            receipt
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    fn payload() -> TargetPayload {
        payload_for(ReleaseTarget::X86_64UnknownLinuxMusl)
    }

    fn payload_for(target: ReleaseTarget) -> TargetPayload {
        let environment = format!("registry.example/proof@sha256:{}", "4".repeat(64));
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: environment,
        };
        let app = Digest::compute(Domain::DistArtifact, b"app generation");
        let native = Digest::compute(Domain::DistArtifact, b"native");
        let mut inventory = wire::manifest::CANVAS_APP_FILES
            .iter()
            .map(|file| InventoryEntry {
                path: format!("app/{file}"),
                mode: 0o644,
                bytes: file.len() as u64,
                digest: Digest::compute(Domain::DistArtifact, file.as_bytes()),
            })
            .collect::<Vec<_>>();
        inventory.push(InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: 6,
            digest: native.clone(),
        });
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        TargetPayload {
            version: wire::manifest::TARGET_PAYLOAD_VERSION,
            target,
            source: Digest::compute(Domain::DistArtifact, b"source"),
            app,
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
        clean_replay_in(
            payload,
            ProofEnvironment::oci(&payload.pins.proof_container)
                .expect("fixture image is digest-pinned"),
        )
    }

    fn clean_replay_in(payload: &TargetPayload, environment: ProofEnvironment) -> ReplayVerdict {
        let battery = Battery::current();
        ReplayVerdict::fixture_clean(payload.target, environment, battery.digest().clone())
    }

    fn assert_refuses(
        admitted: &AdmittedPayload,
        receipt: &wire::manifest::ProofReceipt,
        replay: &ReplayVerdict,
        expected: EvidenceError,
    ) {
        assert_eq!(
            verify(
                admitted,
                &wire::encode(receipt).expect("receipt encodes"),
                replay,
            ),
            Err(expected)
        );
    }
}
