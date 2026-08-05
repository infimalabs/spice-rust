//! Deterministic release notes from one complete canonical evidence document.

use std::collections::{BTreeMap, BTreeSet};

use wire::manifest::{
    RELEASE_NOTES_VERSION, ReleaseNotesInput, ReleaseNotesInputTarget, ReleaseTarget,
};
use wire::{Digest, Domain, ReleaseNotes, ReleaseNotesTarget};

const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

/// Admit one exact release input and derive its stable release-note document.
///
/// The input bytes must already be the canonical current WIRE encoding. No
/// repository, journal, environment value, or other ambient source is read.
///
/// # Errors
///
/// Refuses malformed or noncanonical bytes, another schema version, anything
/// other than both supported targets in declaration order, reused identities,
/// and evidence whose explicit target, payload, or source bindings disagree.
pub fn release_notes(bytes: &[u8]) -> Result<ReleaseNotes, NotesError> {
    let input: ReleaseNotesInput =
        wire::decode(bytes).map_err(|error| NotesError::Decode(error.to_string()))?;
    let canonical = wire::encode(&input).map_err(|error| NotesError::Encode(error.to_string()))?;
    if canonical != bytes {
        return Err(NotesError::NonCanonical);
    }
    derive(input, canonical)
}

fn derive(input: ReleaseNotesInput, canonical: Vec<u8>) -> Result<ReleaseNotes, NotesError> {
    admit(&input)?;
    Ok(ReleaseNotes {
        version: RELEASE_NOTES_VERSION,
        identity: Digest::compute(Domain::DistReleaseNotes, &canonical),
        source: input.source,
        targets: input
            .targets
            .into_iter()
            .map(|target| ReleaseNotesTarget {
                target: target.target,
                payload: target.payload,
                proof_receipt: target.proof.receipt,
                gate_receipt: target.gate.receipt,
            })
            .collect(),
    })
}

fn admit(input: &ReleaseNotesInput) -> Result<(), NotesError> {
    if input.version != RELEASE_NOTES_VERSION {
        return Err(NotesError::Version(input.version));
    }
    if input.targets.len() != TARGETS.len() {
        return Err(NotesError::TargetCount(input.targets.len()));
    }

    let mut targets = BTreeSet::new();
    for target in &input.targets {
        if !targets.insert(target.target) {
            return Err(NotesError::DuplicateTarget(target.target));
        }
    }

    let mut identities = BTreeMap::new();
    remember(&mut identities, input.source.clone(), "source")?;
    for (index, (target, expected)) in input.targets.iter().zip(TARGETS).enumerate() {
        admit_target(target, expected, &input.source, index)?;
        remember(
            &mut identities,
            target.payload.clone(),
            format!("target[{index}].payload"),
        )?;
        remember(
            &mut identities,
            target.proof.receipt.clone(),
            format!("target[{index}].proof.receipt"),
        )?;
        remember(
            &mut identities,
            target.gate.receipt.clone(),
            format!("target[{index}].gate.receipt"),
        )?;
    }
    Ok(())
}

fn admit_target(
    entry: &ReleaseNotesInputTarget,
    expected: ReleaseTarget,
    source: &Digest,
    index: usize,
) -> Result<(), NotesError> {
    if entry.target != expected {
        return Err(NotesError::TargetOrder {
            index,
            expected,
            observed: entry.target,
        });
    }
    if entry.proof.target != entry.target {
        return Err(NotesError::ProofTarget {
            index,
            entry: entry.target,
            proof: entry.proof.target,
        });
    }
    if entry.proof.payload != entry.payload {
        return Err(NotesError::ProofPayload { index });
    }
    if entry.gate.target != entry.target {
        return Err(NotesError::GateTarget {
            index,
            entry: entry.target,
            gate: entry.gate.target,
        });
    }
    if entry.gate.source != *source {
        return Err(NotesError::GateSource { index });
    }
    Ok(())
}

fn remember(
    identities: &mut BTreeMap<Digest, String>,
    identity: Digest,
    role: impl Into<String>,
) -> Result<(), NotesError> {
    let role = role.into();
    if let Some(first) = identities.insert(identity.clone(), role.clone()) {
        Err(NotesError::IdentityReuse {
            identity,
            first,
            second: role,
        })
    } else {
        Ok(())
    }
}

/// Why a release input could not become release notes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NotesError {
    #[error("release-note input does not decode: {0}")]
    Decode(String),
    #[error("release-note input does not encode: {0}")]
    Encode(String),
    #[error("release-note input is not canonical current WIRE JSON")]
    NonCanonical,
    #[error(
        "release-note input version {0} is unsupported; current version is {RELEASE_NOTES_VERSION}"
    )]
    Version(u16),
    #[error("release-note input requires exactly two targets, found {0}")]
    TargetCount(usize),
    #[error("release-note input repeats target {0}")]
    DuplicateTarget(ReleaseTarget),
    #[error("release-note target {index} is {observed}, expected {expected}")]
    TargetOrder {
        index: usize,
        expected: ReleaseTarget,
        observed: ReleaseTarget,
    },
    #[error("release-note target {index} is {entry}, but its proof names {proof}")]
    ProofTarget {
        index: usize,
        entry: ReleaseTarget,
        proof: ReleaseTarget,
    },
    #[error("release-note target {index} proof names another payload")]
    ProofPayload { index: usize },
    #[error("release-note target {index} is {entry}, but its gate names {gate}")]
    GateTarget {
        index: usize,
        entry: ReleaseTarget,
        gate: ReleaseTarget,
    },
    #[error("release-note target {index} gate names another source")]
    GateSource { index: usize },
    #[error("release identity {identity} is reused by {first} and {second}")]
    IdentityReuse {
        identity: Digest,
        first: String,
        second: String,
    },
}

#[cfg(test)]
mod tests {
    use wire::manifest::{ReleaseNotesGate, ReleaseNotesProof};

    use super::*;

    #[test]
    fn canonical_input_derives_one_byte_stable_document() {
        let input = fixture();
        let bytes = wire::encode(&input).expect("fixture encodes");

        let first = release_notes(&bytes).expect("fixture is admitted");
        let second = release_notes(&bytes).expect("the same fixture is admitted again");

        assert_eq!(first, second);
        assert_eq!(
            wire::encode(&first).expect("first notes encode"),
            wire::encode(&second).expect("second notes encode")
        );
        assert_eq!(
            first.identity,
            Digest::compute(Domain::DistReleaseNotes, &bytes)
        );
        assert_eq!(first.version, RELEASE_NOTES_VERSION);
    }

    #[test]
    fn every_admitted_input_identity_moves_the_notes_identity() {
        let original = fixture();
        let baseline = derive_fixture(&original).identity;

        let mut source = original.clone();
        source.source = digest(Domain::DistSourceCommit, "other-source");
        for target in &mut source.targets {
            target.gate.source = source.source.clone();
        }
        assert_ne!(derive_fixture(&source).identity, baseline);

        for index in 0..TARGETS.len() {
            let mut payload = original.clone();
            payload.targets[index].payload = digest(Domain::DistTargetPayload, "other-payload");
            payload.targets[index].proof.payload = payload.targets[index].payload.clone();
            assert_ne!(derive_fixture(&payload).identity, baseline);

            let mut proof = original.clone();
            proof.targets[index].proof.receipt = digest(Domain::ProofReceiptV3, "other-proof");
            assert_ne!(derive_fixture(&proof).identity, baseline);

            let mut gate = original.clone();
            gate.targets[index].gate.receipt = digest(Domain::GateReceipt, "other-gate");
            assert_ne!(derive_fixture(&gate).identity, baseline);
        }
    }

    #[test]
    fn target_shape_order_and_evidence_bindings_are_all_load_bearing() {
        let mut missing = fixture();
        missing.targets.pop();
        assert_eq!(error(&missing), NotesError::TargetCount(1));

        let mut duplicate = fixture();
        duplicate.targets[1].target = duplicate.targets[0].target;
        assert!(matches!(
            error(&duplicate),
            NotesError::DuplicateTarget(ReleaseTarget::Aarch64AppleDarwin)
        ));

        let mut unordered = fixture();
        unordered.targets.swap(0, 1);
        assert!(matches!(
            error(&unordered),
            NotesError::TargetOrder { index: 0, .. }
        ));

        let mut proof_target = fixture();
        proof_target.targets[0].proof.target = ReleaseTarget::X86_64UnknownLinuxMusl;
        assert!(matches!(
            error(&proof_target),
            NotesError::ProofTarget { index: 0, .. }
        ));

        let mut proof_payload = fixture();
        proof_payload.targets[0].proof.payload =
            digest(Domain::DistTargetPayload, "foreign-payload");
        assert_eq!(error(&proof_payload), NotesError::ProofPayload { index: 0 });

        let mut gate_target = fixture();
        gate_target.targets[1].gate.target = ReleaseTarget::Aarch64AppleDarwin;
        assert!(matches!(
            error(&gate_target),
            NotesError::GateTarget { index: 1, .. }
        ));

        let mut gate_source = fixture();
        gate_source.targets[1].gate.source = digest(Domain::DistSourceCommit, "foreign-source");
        assert_eq!(error(&gate_source), NotesError::GateSource { index: 1 });
    }

    #[test]
    fn reused_and_noncanonical_identities_never_derive_notes() {
        let mut duplicate = fixture();
        duplicate.targets[1].payload = duplicate.targets[0].payload.clone();
        duplicate.targets[1].proof.payload = duplicate.targets[0].payload.clone();
        assert!(matches!(
            error(&duplicate),
            NotesError::IdentityReuse { .. }
        ));

        let canonical = wire::encode(&fixture()).expect("fixture encodes");
        let mut second_spelling = canonical;
        second_spelling.push(b'\n');
        assert_eq!(
            release_notes(&second_spelling),
            Err(NotesError::NonCanonical)
        );
    }

    fn fixture() -> ReleaseNotesInput {
        let source = digest(Domain::DistSourceCommit, "source");
        ReleaseNotesInput {
            version: RELEASE_NOTES_VERSION,
            source: source.clone(),
            targets: TARGETS
                .into_iter()
                .enumerate()
                .map(|(index, target)| {
                    let payload = digest(Domain::DistTargetPayload, &format!("payload-{index}"));
                    ReleaseNotesInputTarget {
                        target,
                        payload: payload.clone(),
                        proof: ReleaseNotesProof {
                            target,
                            payload,
                            receipt: digest(Domain::ProofReceiptV3, &format!("proof-{index}")),
                        },
                        gate: ReleaseNotesGate {
                            target,
                            source: source.clone(),
                            receipt: digest(Domain::GateReceipt, &format!("gate-{index}")),
                        },
                    }
                })
                .collect(),
        }
    }

    fn derive_fixture(input: &ReleaseNotesInput) -> ReleaseNotes {
        release_notes(&wire::encode(input).expect("fixture encodes")).expect("fixture is admitted")
    }

    fn error(input: &ReleaseNotesInput) -> NotesError {
        release_notes(&wire::encode(input).expect("fixture encodes"))
            .expect_err("fixture must refuse")
    }

    fn digest(domain: Domain, label: &str) -> Digest {
        Digest::compute(domain, label.as_bytes())
    }
}
