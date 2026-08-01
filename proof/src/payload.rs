//! Admission of one canonical distribution target before proof execution.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use wire::manifest::{InventoryEntry, ProofImage, TargetPayload};
use wire::{Digest, Domain};

const PAYLOAD_VERSION: u16 = 1;
const ARTIFACTS: [Artifact; 3] = [
    Artifact {
        path: "app/app.wasm",
        mode: 0o644,
        digest: DigestField::App,
    },
    Artifact {
        path: "bin/spice",
        mode: 0o755,
        digest: DigestField::Native,
    },
    Artifact {
        path: "embed/app.bin",
        mode: 0o644,
        digest: DigestField::EmbeddedApp,
    },
];

/// A canonical, internally coherent target payload admitted for proof work.
///
/// Construction is private so runner and evidence APIs can require this type
/// without retaining another payload-validation path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedPayload {
    value: TargetPayload,
    digest: Digest,
}

impl AdmittedPayload {
    /// The exact typed payload admitted from canonical bytes.
    #[must_use]
    pub const fn value(&self) -> &TargetPayload {
        &self.value
    }

    /// Domain-tagged identity of the exact admitted payload bytes.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Admit one target payload before any sealed runner or receipt publisher.
///
/// # Errors
///
/// Refuses malformed or noncanonical bytes, a stale schema, an unpinned proof
/// environment, inconsistent subordinate digests, or any inventory other than
/// the three canonical artifacts in their exact order, modes, and identities.
pub fn admit(bytes: &[u8]) -> Result<AdmittedPayload, PayloadError> {
    let payload: TargetPayload =
        wire::decode(bytes).map_err(|source| PayloadError::Decode(source.to_string()))?;
    let canonical =
        wire::encode(&payload).map_err(|source| PayloadError::Encode(source.to_string()))?;
    if canonical != bytes {
        return Err(PayloadError::NonCanonical);
    }
    if payload.version != PAYLOAD_VERSION {
        return Err(PayloadError::Version(payload.version));
    }
    if !ProofImage::admits(&payload.pins.proof_container) {
        return Err(PayloadError::UnpinnedEnvironment(
            payload.pins.proof_container.clone(),
        ));
    }

    let pins =
        wire::encode(&payload.pins).map_err(|source| PayloadError::Encode(source.to_string()))?;
    if payload.pins_digest != Digest::compute(Domain::DistPins, &pins) {
        return Err(PayloadError::PinsDigest);
    }
    let inventory = wire::encode(&payload.inventory)
        .map_err(|source| PayloadError::Encode(source.to_string()))?;
    if payload.inventory_digest != Digest::compute(Domain::DistInventory, &inventory) {
        return Err(PayloadError::InventoryDigest);
    }

    verify_inventory(&payload)?;

    Ok(AdmittedPayload {
        value: payload,
        digest: Digest::compute(Domain::DistTargetPayload, &canonical),
    })
}

fn verify_inventory(payload: &TargetPayload) -> Result<(), PayloadError> {
    let mut paths = BTreeSet::new();
    for entry in &payload.inventory {
        if !paths.insert(entry.path.as_str()) {
            return Err(PayloadError::DuplicateArtifact(entry.path.clone()));
        }
    }

    for artifact in ARTIFACTS {
        if !paths.contains(artifact.path) {
            return Err(PayloadError::MissingArtifact(artifact.path));
        }
    }
    if let Some(entry) = payload
        .inventory
        .iter()
        .find(|entry| !ARTIFACTS.iter().any(|artifact| artifact.path == entry.path))
    {
        return Err(PayloadError::ExtraArtifact(entry.path.clone()));
    }

    for (index, (entry, artifact)) in payload.inventory.iter().zip(ARTIFACTS).enumerate() {
        if entry.path != artifact.path {
            return Err(PayloadError::ReorderedArtifact {
                index,
                expected: artifact.path,
                observed: entry.path.clone(),
            });
        }
        verify_artifact(payload, entry, artifact)?;
    }
    Ok(())
}

fn verify_artifact(
    payload: &TargetPayload,
    entry: &InventoryEntry,
    artifact: Artifact,
) -> Result<(), PayloadError> {
    if entry.mode != artifact.mode {
        return Err(PayloadError::ArtifactMode {
            path: artifact.path,
            expected: artifact.mode,
            observed: entry.mode,
        });
    }
    if entry.digest != *artifact.digest.from(payload) {
        return Err(PayloadError::ArtifactDigest(artifact.path));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Artifact {
    path: &'static str,
    mode: u32,
    digest: DigestField,
}

#[derive(Clone, Copy)]
enum DigestField {
    App,
    Native,
    EmbeddedApp,
}

impl DigestField {
    const fn from(self, payload: &TargetPayload) -> &Digest {
        match self {
            Self::App => &payload.app,
            Self::Native => &payload.native,
            Self::EmbeddedApp => &payload.embedded_app,
        }
    }
}

/// Why target payload bytes were not admitted for proof work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadError {
    /// Bytes do not decode as the current target payload shape.
    Decode(String),
    /// The decoded payload could not be encoded canonically.
    Encode(String),
    /// Bytes decode but differ from their canonical encoding.
    NonCanonical,
    /// The payload schema is not current.
    Version(u16),
    /// The proof environment is not pinned by an OCI digest.
    UnpinnedEnvironment(String),
    /// The declared pins digest disagrees with the pins document.
    PinsDigest,
    /// The declared inventory digest disagrees with the inventory document.
    InventoryDigest,
    /// One canonical artifact is absent.
    MissingArtifact(&'static str),
    /// An artifact outside the canonical inventory is present.
    ExtraArtifact(String),
    /// One artifact path occurs more than once.
    DuplicateArtifact(String),
    /// The canonical artifacts are not in strict order.
    ReorderedArtifact {
        /// Zero-based inventory position.
        index: usize,
        /// Canonical path required at that position.
        expected: &'static str,
        /// Path observed at that position.
        observed: String,
    },
    /// One canonical artifact has the wrong published mode.
    ArtifactMode {
        /// Canonical artifact path.
        path: &'static str,
        /// Required mode.
        expected: u32,
        /// Declared mode.
        observed: u32,
    },
    /// One inventory digest disagrees with the payload field it names.
    ArtifactDigest(&'static str),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(source) => write!(formatter, "target payload does not decode: {source}"),
            Self::Encode(source) => write!(formatter, "target payload does not encode: {source}"),
            Self::NonCanonical => formatter.write_str("target payload bytes are not canonical"),
            Self::Version(version) => {
                write!(formatter, "target payload version {version} is not current")
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
            Self::MissingArtifact(path) => {
                write!(formatter, "target payload inventory is missing {path}")
            }
            Self::ExtraArtifact(path) => {
                write!(
                    formatter,
                    "target payload inventory has extra artifact {path:?}"
                )
            }
            Self::DuplicateArtifact(path) => {
                write!(
                    formatter,
                    "target payload inventory repeats artifact {path:?}"
                )
            }
            Self::ReorderedArtifact {
                index,
                expected,
                observed,
            } => write!(
                formatter,
                "target payload artifact {index} is {observed:?}, expected {expected:?}"
            ),
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
        }
    }
}

impl Error for PayloadError {}

#[cfg(test)]
mod tests {
    use wire::manifest::{BuildPins, InventoryEntry, ReleaseTarget, TargetPayload};
    use wire::{Digest, Domain};

    use super::{PayloadError, admit};

    #[test]
    fn canonical_payload_admits_with_its_exact_domain_identity() {
        let payload = payload();
        let bytes = encode(&payload);

        let admitted = admit(&bytes).expect("canonical payload admits");

        assert_eq!(admitted.value(), &payload);
        assert_eq!(
            admitted.digest(),
            &Digest::compute(Domain::DistTargetPayload, &bytes)
        );
        assert_eq!(wire::encode(admitted.value()).expect("re-encode"), bytes);
    }

    #[test]
    fn malformed_noncanonical_stale_and_unpinned_payloads_refuse_before_admission() {
        let mut runner_started = false;
        let mut publisher_reached = false;
        let malformed = admit(b"not-json").map(|_| {
            runner_started = true;
            publisher_reached = true;
        });
        assert!(matches!(malformed, Err(PayloadError::Decode(_))));
        assert!(!runner_started);
        assert!(!publisher_reached);

        let mut noncanonical = encode(&payload());
        noncanonical.push(b'\n');
        assert_refuses_before_proof(&noncanonical, PayloadError::NonCanonical);

        let mut stale = payload();
        stale.version = 2;
        assert_refuses_before_proof(&encode(&stale), PayloadError::Version(2));

        let mut unpinned = payload();
        unpinned.pins.proof_container = "registry.example/proof:latest".to_owned();
        assert_refuses_before_proof(
            &encode(&unpinned),
            PayloadError::UnpinnedEnvironment("registry.example/proof:latest".to_owned()),
        );
    }

    #[test]
    fn subordinate_pin_and_inventory_identities_are_recomputed() {
        let mut wrong_pins = payload();
        wrong_pins.pins.rust = "rustc another".to_owned();
        assert_refuses_before_proof(&encode(&wrong_pins), PayloadError::PinsDigest);

        let mut wrong_inventory = payload();
        wrong_inventory.inventory[0].bytes += 1;
        assert_refuses_before_proof(&encode(&wrong_inventory), PayloadError::InventoryDigest);
    }

    #[test]
    fn missing_extra_duplicate_and_reordered_inventory_refuse_apart() {
        let mut missing = payload();
        missing.inventory.remove(0);
        refresh_inventory(&mut missing);
        assert_refuses_before_proof(
            &encode(&missing),
            PayloadError::MissingArtifact("app/app.wasm"),
        );

        let mut extra = payload();
        extra.inventory.push(InventoryEntry {
            path: "extra/debug.txt".to_owned(),
            mode: 0o644,
            bytes: 0,
            digest: Digest::compute(Domain::DistArtifact, b"extra"),
        });
        refresh_inventory(&mut extra);
        assert_refuses_before_proof(
            &encode(&extra),
            PayloadError::ExtraArtifact("extra/debug.txt".to_owned()),
        );

        let mut duplicate = payload();
        duplicate.inventory.push(duplicate.inventory[0].clone());
        refresh_inventory(&mut duplicate);
        assert_refuses_before_proof(
            &encode(&duplicate),
            PayloadError::DuplicateArtifact("app/app.wasm".to_owned()),
        );

        let mut reordered = payload();
        reordered.inventory.swap(0, 1);
        refresh_inventory(&mut reordered);
        assert_refuses_before_proof(
            &encode(&reordered),
            PayloadError::ReorderedArtifact {
                index: 0,
                expected: "app/app.wasm",
                observed: "bin/spice".to_owned(),
            },
        );
    }

    #[test]
    fn every_canonical_path_has_one_mode_and_one_named_digest() {
        for (index, path, expected) in [
            (0, "app/app.wasm", 0o644),
            (1, "bin/spice", 0o755),
            (2, "embed/app.bin", 0o644),
        ] {
            let mut wrong_mode = payload();
            wrong_mode.inventory[index].mode = 0;
            refresh_inventory(&mut wrong_mode);
            assert_refuses_before_proof(
                &encode(&wrong_mode),
                PayloadError::ArtifactMode {
                    path,
                    expected,
                    observed: 0,
                },
            );

            let mut wrong_digest = payload();
            match index {
                0 => wrong_digest.app = Digest::compute(Domain::DistArtifact, b"other app"),
                1 => wrong_digest.native = Digest::compute(Domain::DistArtifact, b"other native"),
                2 => {
                    wrong_digest.embedded_app =
                        Digest::compute(Domain::DistArtifact, b"other embedded")
                }
                _ => unreachable!("fixture has exactly three artifacts"),
            }
            assert_refuses_before_proof(&encode(&wrong_digest), PayloadError::ArtifactDigest(path));
        }
    }

    fn payload() -> TargetPayload {
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!("registry.example/proof@sha256:{}", "4".repeat(64)),
        };
        let app = Digest::compute(Domain::DistArtifact, b"app");
        let native = Digest::compute(Domain::DistArtifact, b"native");
        let embedded = Digest::compute(Domain::DistArtifact, b"embedded");
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

    fn encode(payload: &TargetPayload) -> Vec<u8> {
        wire::encode(payload).expect("payload encodes")
    }

    fn refresh_inventory(payload: &mut TargetPayload) {
        payload.inventory_digest = Digest::compute(
            Domain::DistInventory,
            &wire::encode(&payload.inventory).expect("inventory encodes"),
        );
    }

    fn assert_refuses_before_proof(bytes: &[u8], expected: PayloadError) {
        let mut runner_started = false;
        let mut publisher_reached = false;
        let refused = admit(bytes).map(|_| {
            runner_started = true;
            publisher_reached = true;
        });

        assert_eq!(refused, Err(expected));
        assert!(!runner_started, "a refused payload reached the runner");
        assert!(
            !publisher_reached,
            "a refused payload reached receipt publication"
        );
    }
}
