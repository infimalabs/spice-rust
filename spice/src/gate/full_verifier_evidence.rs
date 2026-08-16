//! Atomic, worktree-local evidence published by the complete verifier.

use std::fs;
use std::io::{self, Write as _};

use ::gate::{
    BatteryReceipt, GateSemanticClosure, Selector, StateReceipt, VerifierCompilerEvidence,
};
use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::{PreparedBattery, RunError, prepare_strict};
use crate::repository::Repository;

mod semantic;

pub use semantic::{AdmittedSemanticVerifierEvidence, SemanticFullVerifierAdmission};

const FULL_VERIFIER_EVIDENCE_VERSION: u64 = 3;
const STATE_PUBLICATION_ATTEMPTS: usize = 4;
const AUTHORITY_PREFIX: &[u8] = b"spice/full-verifier/compiler-evidence/v1";
const PREPARED_AUTHORITY_PREFIX: &[u8] = b"spice/full-verifier/prepared-compiler-evidence/v1";

/// Canonical evidence retained only after the complete verifier admits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FullVerifierEvidence {
    version: u64,
    cargo_jobs: usize,
    battery: String,
    compiler: String,
    state: String,
    semantic: String,
}

impl FullVerifierEvidence {
    /// Bind an admitted complete battery and every completed compiler arm.
    pub fn mint(
        repository: &Repository,
        cargo_jobs: usize,
        battery: &BatteryReceipt,
        compiler: &[VerifierCompilerEvidence],
        prepared: &PreparedBattery<'_>,
    ) -> Result<Self, String> {
        if !battery.run().admits() {
            return Err("full-verifier evidence requires an admitted battery receipt".to_owned());
        }
        if &prepared.context().map_err(|error| error.to_string())? != battery.run().context() {
            return Err(
                "full-verifier evidence requires the prepared battery's exact authority context"
                    .to_owned(),
            );
        }
        if prepared.source() != battery.tree() {
            return Err(
                "full-verifier evidence preparation names a different selected source".to_owned(),
            );
        }
        let closure = GateSemanticClosure::capture(
            prepared.source().clone(),
            prepared.root(),
            prepared.paths(),
            prepared.changes(),
            prepared.generated_paths(),
        )?;
        let semantic = semantic::SemanticEvidence::mint(repository, closure, battery, compiler)?;
        let state = accept_state_with_retry(prepared, battery)?;
        let semantic = String::from_utf8(
            semantic
                .encode()
                .map_err(|error| format!("encode semantic evidence: {error}"))?,
        )
        .map_err(|error| format!("semantic evidence is not UTF-8: {error}"))?;
        let battery = String::from_utf8(
            battery
                .encode()
                .map_err(|error| format!("encode battery receipt: {error}"))?,
        )
        .map_err(|error| format!("battery receipt is not UTF-8: {error}"))?;
        let compiler = String::from_utf8(
            ::gate::encode_verifier_compiler_receipt(compiler, &compiler_authority(repository))
                .map_err(|error| format!("encode compiler receipt: {error}"))?,
        )
        .map_err(|error| format!("compiler receipt is not UTF-8: {error}"))?;
        if !state.accepted() {
            return Err("full-verifier evidence requires an accepted state transition".to_owned());
        }
        let state = String::from_utf8(
            state
                .encode()
                .map_err(|error| format!("encode state receipt: {error}"))?,
        )
        .map_err(|error| format!("state receipt is not UTF-8: {error}"))?;
        let evidence = Self {
            version: FULL_VERIFIER_EVIDENCE_VERSION,
            cargo_jobs: cargo_jobs.max(1),
            battery,
            compiler,
            state,
            semantic,
        };
        evidence.validate_shape()?;
        Ok(evidence)
    }

    /// Canonical bytes of this complete bundle.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    fn decode(document: &[u8]) -> Result<Self, String> {
        let evidence: Self = wire::decode(document)
            .map_err(|error| format!("decode full-verifier evidence: {error}"))?;
        evidence.validate_shape()?;
        let canonical = evidence
            .encode()
            .map_err(|error| format!("encode full-verifier evidence: {error}"))?;
        if canonical != document {
            return Err("full-verifier evidence is not canonical".to_owned());
        }
        Ok(evidence)
    }

    fn validate_shape(&self) -> Result<(), String> {
        if self.version != FULL_VERIFIER_EVIDENCE_VERSION {
            return Err(format!(
                "unsupported full-verifier evidence version {}; expected {FULL_VERIFIER_EVIDENCE_VERSION}",
                self.version,
            ));
        }
        if self.cargo_jobs == 0
            || self.battery.is_empty()
            || self.compiler.is_empty()
            || self.state.is_empty()
            || self.semantic.is_empty()
        {
            return Err("full-verifier evidence is partial".to_owned());
        }
        Ok(())
    }

    /// Atomically replace the evidence for this worktree.
    pub fn publish(&self, repository: &Repository) -> Result<(), String> {
        let paths = repository
            .admit_state()
            .map_err(|error| format!("admit full-verifier state: {error}"))?;
        fs::create_dir_all(paths.worktree_runtime()).map_err(|error| {
            format!(
                "create full-verifier state directory {}: {error}",
                paths.worktree_runtime().display(),
            )
        })?;
        let paths = repository
            .admit_state()
            .map_err(|error| format!("re-admit full-verifier state: {error}"))?;
        let bytes = self
            .encode()
            .map_err(|error| format!("encode full-verifier evidence: {error}"))?;
        atomic::Destination::new(paths.full_verifier_evidence())
            .map_err(|error| format!("resolve full-verifier destination: {error}"))?
            .publish(|staged| staged.write_all(&bytes))
            .map_err(|error| format!("publish full-verifier evidence: {error}"))
    }
}

fn accept_state_with_retry(
    prepared: &PreparedBattery<'_>,
    battery: &BatteryReceipt,
) -> Result<StateReceipt, String> {
    let receipt = battery
        .encode()
        .map_err(|error| format!("encode battery receipt for state retry: {error}"))?;
    for attempt in 1..=STATE_PUBLICATION_ATTEMPTS {
        let result = if attempt == 1 {
            prepared.accept_state()
        } else {
            let retry = prepare_strict(Selector::staged())
                .map_err(|error| format!("recapture full-verifier battery state: {error}"))?;
            if retry.source() != prepared.source() {
                return Err(
                    "staged source moved while retrying full-verifier state publication".to_owned(),
                );
            }
            let admitted = retry
                .admit_battery_receipt(&receipt)
                .map_err(|error| format!("re-admit full-verifier battery state: {error}"))?;
            if &admitted != battery {
                return Err(
                    "battery receipt changed while retrying full-verifier state publication"
                        .to_owned(),
                );
            }
            retry.accept_state()
        };
        match result {
            Ok(state) => return Ok(state),
            Err(error)
                if error.is_stale_state_publication() && attempt < STATE_PUBLICATION_ATTEMPTS => {}
            Err(error) => {
                return Err(format!("accept full-verifier battery state: {error}"));
            }
        }
    }
    unreachable!("the fixed state publication attempt loop always returns")
}

/// A retained bundle independently matched to a fresh strict capture.
pub struct AdmittedFullVerifierEvidence {
    source: Digest,
    evidence: Digest,
    battery: BatteryReceipt,
}

impl AdmittedFullVerifierEvidence {
    /// Exact staged source proven by every retained arm.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Identity of the exact canonical bundle consumed by the hook.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.evidence
    }

    /// Complete re-admitted policy-arm evidence.
    #[must_use]
    pub const fn battery(&self) -> &BatteryReceipt {
        &self.battery
    }
}

/// Consume the worktree-local bundle only when every current authority agrees.
///
/// Missing, malformed, stale, reordered, partial, or contract-drifted content
/// is a cache miss.  Filesystem shape and read faults remain faults; the caller
/// must not silently step around a redirected or unavailable state path.
pub fn admit_full_verifier_evidence(
    repository: &Repository,
    selector: Selector,
) -> Result<Option<AdmittedFullVerifierEvidence>, RunError> {
    let paths = repository
        .admit_state()
        .map_err(|error| RunError::fault("gate::full-verifier-state-unavailable", error))?;
    let document = match fs::read(paths.full_verifier_evidence()) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RunError::fault(
                "gate::full-verifier-evidence-unavailable",
                error,
            ));
        }
    };
    let Ok(evidence) = FullVerifierEvidence::decode(&document) else {
        return Ok(None);
    };
    let Ok(semantic) = semantic::SemanticEvidence::decode(evidence.semantic.as_bytes()) else {
        return Ok(None);
    };
    let Ok(mut prepared) = prepare_strict(selector) else {
        return Ok(None);
    };
    let Ok(current_closure) = GateSemanticClosure::capture(
        prepared.source().clone(),
        prepared.root(),
        prepared.paths(),
        prepared.changes(),
        prepared.generated_paths(),
    ) else {
        return Ok(None);
    };
    let Ok(state) = StateReceipt::decode(evidence.state.as_bytes()) else {
        return Ok(None);
    };
    let Ok(battery) =
        prepared.admit_battery_receipt_after_state(evidence.battery.as_bytes(), &state)
    else {
        return Ok(None);
    };
    let Ok(compiler) = prepared.admit_verifier_compiler_receipt(
        evidence.compiler.as_bytes(),
        evidence.cargo_jobs,
        &compiler_authority(repository),
    ) else {
        return Ok(None);
    };
    if semantic
        .validate_exact(repository, &battery, &compiler, &current_closure)
        .is_err()
    {
        return Ok(None);
    }
    Ok(Some(AdmittedFullVerifierEvidence {
        source: battery.tree().clone(),
        evidence: Digest::compute(Domain::GateReceipt, &document),
        battery,
    }))
}

/// Re-admit individual green arms when their semantic input closures are unchanged.
pub fn admit_semantic_full_verifier_evidence(
    repository: &Repository,
    selector: Selector,
) -> Result<SemanticFullVerifierAdmission, RunError> {
    let paths = repository
        .admit_state()
        .map_err(|error| RunError::fault("gate::full-verifier-state-unavailable", error))?;
    let document = match fs::read(paths.full_verifier_evidence()) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SemanticFullVerifierAdmission::Unavailable {
                detail: "no full-verifier evidence is published for this worktree".to_owned(),
            });
        }
        Err(error) => {
            return Err(RunError::fault(
                "gate::full-verifier-evidence-unavailable",
                error,
            ));
        }
    };
    let evidence = match FullVerifierEvidence::decode(&document) {
        Ok(evidence) => evidence,
        Err(detail) => return Ok(SemanticFullVerifierAdmission::Unavailable { detail }),
    };
    let mut prepared = match prepare_strict(selector) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Ok(SemanticFullVerifierAdmission::Unavailable {
                detail: error.to_string(),
            });
        }
    };
    let semantic = match semantic::SemanticEvidence::decode(evidence.semantic.as_bytes()) {
        Ok(semantic) => semantic,
        Err(detail) => return Ok(SemanticFullVerifierAdmission::Unavailable { detail }),
    };
    match semantic.admit(
        repository,
        &mut prepared,
        evidence.cargo_jobs,
        evidence.battery.as_bytes(),
        evidence.compiler.as_bytes(),
        evidence.state.as_bytes(),
        Digest::compute(Domain::GateReceipt, &document),
    ) {
        Ok(admitted) => Ok(SemanticFullVerifierAdmission::Admitted(Box::new(admitted))),
        Err(detail) => Ok(SemanticFullVerifierAdmission::Unavailable { detail }),
    }
}

fn compiler_authority(repository: &Repository) -> Vec<u8> {
    let path = repository.git_dir().as_os_str().as_encoded_bytes();
    framed_authority(AUTHORITY_PREFIX, path)
}

pub(super) fn prepared_compiler_authority(source: &Digest) -> Vec<u8> {
    framed_authority(PREPARED_AUTHORITY_PREFIX, source.as_str().as_bytes())
}

fn framed_authority(prefix: &[u8], identity: &[u8]) -> Vec<u8> {
    let mut authority =
        Vec::with_capacity(prefix.len() + std::mem::size_of::<u64>() + identity.len());
    authority.extend_from_slice(prefix);
    authority.extend_from_slice(
        &u64::try_from(identity.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    authority.extend_from_slice(identity);
    authority
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn bundle_shape_refuses_partial_versioned_and_noncanonical_documents() {
        let complete = FullVerifierEvidence {
            version: FULL_VERIFIER_EVIDENCE_VERSION,
            cargo_jobs: 3,
            battery: "battery".to_owned(),
            compiler: "compiler".to_owned(),
            state: "state".to_owned(),
            semantic: "semantic".to_owned(),
        };
        let canonical = complete.encode().expect("encode complete fixture");
        assert_eq!(
            FullVerifierEvidence::decode(&canonical).expect("decode canonical fixture"),
            complete,
        );

        let mut noncanonical = canonical;
        noncanonical.push(b'\n');
        assert!(FullVerifierEvidence::decode(&noncanonical).is_err());
        for partial in [
            FullVerifierEvidence {
                compiler: String::new(),
                ..complete.clone()
            },
            FullVerifierEvidence {
                battery: String::new(),
                ..complete.clone()
            },
            FullVerifierEvidence {
                state: String::new(),
                ..complete.clone()
            },
            FullVerifierEvidence {
                semantic: String::new(),
                ..complete.clone()
            },
            FullVerifierEvidence {
                cargo_jobs: 0,
                ..complete.clone()
            },
            FullVerifierEvidence {
                version: FULL_VERIFIER_EVIDENCE_VERSION + 1,
                ..complete
            },
        ] {
            let bytes = partial.encode().expect("encode invalid fixture");
            assert!(FullVerifierEvidence::decode(&bytes).is_err());
        }
    }

    #[test]
    fn compiler_provenance_authority_is_worktree_bound() {
        let scratch = Scratch::new();
        let first = scratch.root.join("first");
        let second = scratch.root.join("second");
        fs::create_dir_all(first.join(".git")).expect("create first Git directory");
        fs::create_dir_all(second.join(".git")).expect("create second Git directory");
        let first = Repository::discover(&first).expect("discover first worktree");
        let second = Repository::discover(&second).expect("discover second worktree");

        assert_ne!(compiler_authority(&first), compiler_authority(&second));
        assert_ne!(
            semantic::worktree_authority(&first),
            semantic::worktree_authority(&second),
        );
    }

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-full-verifier-evidence-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::create_dir(&root).expect("create evidence fixture");
            Self { root }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove evidence fixture");
        }
    }
}
