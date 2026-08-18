//! Atomic worktree-latest and repository-common parent evidence from the verifier.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use ::gate::{
    BatteryContext, BatteryReceipt, GateId, GateSemanticClosure, Selector, StateReceipt,
    VerifierCompilerEvidence,
};
use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::{PreparedBattery, RunError, prepare, prepare_strict};
use crate::gate::environment::exact_evidence_environment;
use crate::repository::Repository;

mod reachability;
mod semantic;
mod storage;

pub use semantic::{AdmittedSemanticVerifierEvidence, SemanticFullVerifierAdmission};

const FULL_VERIFIER_EVIDENCE_VERSION: u64 = 3;
const MERGE_PARENT_EVIDENCE_VERSION: u64 = 1;
const STATE_PUBLICATION_ATTEMPTS: usize = 4;
const AUTHORITY_PREFIX: &[u8] = b"spice/full-verifier/compiler-evidence/v1";
const MERGE_AUTHORITY_PREFIX: &[u8] = b"spice/full-verifier/merge-compiler-evidence/v1";
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
    #[serde(skip)]
    merge_parent: Option<MergeParentEvidence>,
}

/// Repository-portable parent proof published beside the exact worktree bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MergeParentEvidence {
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
        let semantic =
            semantic::SemanticEvidence::mint(repository, closure.clone(), battery, compiler)?;
        let merge_semantic =
            semantic::MergeSemanticEvidence::mint(repository, closure, battery, compiler)?;
        let state = accept_state_with_retry(prepared, battery)?;
        let semantic = String::from_utf8(
            semantic
                .encode()
                .map_err(|error| format!("encode semantic evidence: {error}"))?,
        )
        .map_err(|error| format!("semantic evidence is not UTF-8: {error}"))?;
        let merge_semantic = String::from_utf8(
            merge_semantic
                .encode()
                .map_err(|error| format!("encode merge semantic evidence: {error}"))?,
        )
        .map_err(|error| format!("merge semantic evidence is not UTF-8: {error}"))?;
        let battery = String::from_utf8(
            battery
                .encode()
                .map_err(|error| format!("encode battery receipt: {error}"))?,
        )
        .map_err(|error| format!("battery receipt is not UTF-8: {error}"))?;
        let exact_compiler =
            ::gate::encode_verifier_compiler_receipt(compiler, &compiler_authority(repository))
                .map_err(|error| format!("encode compiler receipt: {error}"))?;
        let merge_compiler = String::from_utf8(
            ::gate::encode_verifier_compiler_receipt(
                compiler,
                &merge_compiler_authority(repository),
            )
            .map_err(|error| format!("encode merge compiler receipt: {error}"))?,
        )
        .map_err(|error| format!("merge compiler receipt is not UTF-8: {error}"))?;
        let compiler = String::from_utf8(exact_compiler)
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
        let cargo_jobs = cargo_jobs.max(1);
        let merge_parent = MergeParentEvidence {
            version: MERGE_PARENT_EVIDENCE_VERSION,
            cargo_jobs,
            battery: battery.clone(),
            compiler: merge_compiler,
            state: state.clone(),
            semantic: merge_semantic,
        };
        merge_parent.validate_shape()?;
        let evidence = Self {
            version: FULL_VERIFIER_EVIDENCE_VERSION,
            cargo_jobs,
            battery,
            compiler,
            state,
            semantic,
            merge_parent: Some(merge_parent),
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

    /// Atomically replace this worktree's evidence and retain its parent archive.
    pub fn publish(&self, repository: &Repository) -> Result<(), String> {
        let paths = repository
            .admit_state()
            .map_err(|error| format!("admit full-verifier state: {error}"))?;
        for directory in [
            paths.worktree_runtime(),
            paths.full_verifier_evidence_archive(),
        ] {
            fs::create_dir_all(directory).map_err(|error| {
                format!(
                    "create full-verifier state directory {}: {error}",
                    directory.display(),
                )
            })?;
        }
        let paths = repository
            .admit_state()
            .map_err(|error| format!("re-admit full-verifier state: {error}"))?;
        let merge_parent = self
            .merge_parent
            .as_ref()
            .ok_or_else(|| "full-verifier publication lacks merge-parent evidence".to_owned())?;
        let semantic = semantic::MergeSemanticEvidence::decode(merge_parent.semantic.as_bytes())?;
        let archive_bytes = merge_parent
            .encode()
            .map_err(|error| format!("encode merge-parent evidence: {error}"))?;
        let exact_bytes = self
            .encode()
            .map_err(|error| format!("encode full-verifier evidence: {error}"))?;
        let archive = storage::LockedArchive::acquire(&paths)?;
        let protected = reachability::protected_sources(repository, semantic.source())?;
        archive.publish_and_groom(&semantic.source().to_string(), &archive_bytes, &protected)?;
        atomic::Destination::new(paths.full_verifier_evidence())
            .map_err(|error| format!("resolve full-verifier destination: {error}"))?
            .publish(|staged| staged.write_all(&exact_bytes))
            .map_err(|error| format!("publish full-verifier evidence: {error}"))
    }
}

impl MergeParentEvidence {
    fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    fn decode(document: &[u8]) -> Result<Self, String> {
        let evidence: Self = wire::decode(document)
            .map_err(|error| format!("decode merge-parent evidence: {error}"))?;
        evidence.validate_shape()?;
        let canonical = evidence
            .encode()
            .map_err(|error| format!("encode merge-parent evidence: {error}"))?;
        if canonical != document {
            return Err("merge-parent evidence is not canonical".to_owned());
        }
        Ok(evidence)
    }

    fn validate_shape(&self) -> Result<(), String> {
        if self.version != MERGE_PARENT_EVIDENCE_VERSION {
            return Err(format!(
                "unsupported merge-parent evidence version {}; expected {MERGE_PARENT_EVIDENCE_VERSION}",
                self.version,
            ));
        }
        if self.cargo_jobs == 0
            || self.battery.is_empty()
            || self.compiler.is_empty()
            || self.state.is_empty()
            || self.semantic.is_empty()
        {
            return Err("merge-parent evidence is partial".to_owned());
        }
        Ok(())
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
    changes: Vec<String>,
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

    /// Exact changed paths from the strict capture which admitted this bundle.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        &self.changes
    }
}

/// Exact stage at which reusable full-verifier evidence became unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullVerifierEvidenceStage {
    Document,
    Bundle,
    SemanticDocument,
    Environment,
    Capture,
    Source,
    Closure,
    State,
    Battery,
    Compiler,
    Semantic,
}

impl FullVerifierEvidenceStage {
    /// Stable diagnostic name used by hook progress and refusals.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Bundle => "bundle",
            Self::SemanticDocument => "semantic-document",
            Self::Environment => "environment",
            Self::Capture => "capture",
            Self::Source => "source",
            Self::Closure => "closure",
            Self::State => "state",
            Self::Battery => "battery",
            Self::Compiler => "compiler",
            Self::Semantic => "semantic",
        }
    }
}

/// One cache miss with its first failed exact-admission stage preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FullVerifierEvidenceMiss {
    stage: FullVerifierEvidenceStage,
    detail: String,
}

impl FullVerifierEvidenceMiss {
    /// Construct a diagnostic miss for a caller-owned admission boundary.
    #[must_use]
    pub fn new(stage: FullVerifierEvidenceStage, detail: impl ToString) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }

    /// First exact-admission stage which did not match.
    #[must_use]
    pub const fn stage(&self) -> FullVerifierEvidenceStage {
        self.stage
    }

    /// Bounded causal detail from that stage.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Exact reusable evidence or a typed, first-cause cache miss.
pub enum FullVerifierEvidenceAdmission {
    Admitted(Box<AdmittedFullVerifierEvidence>),
    Unavailable(FullVerifierEvidenceMiss),
}

impl FullVerifierEvidenceAdmission {
    /// Consume an admitted bundle while discarding a diagnostic miss.
    #[must_use]
    pub fn into_admitted(self) -> Option<AdmittedFullVerifierEvidence> {
        match self {
            Self::Admitted(evidence) => Some(*evidence),
            Self::Unavailable(_) => None,
        }
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
    inspect_full_verifier_evidence(repository, selector)
        .map(FullVerifierEvidenceAdmission::into_admitted)
}

/// Re-admit exact evidence while preserving the first causal cache-miss stage.
pub fn inspect_full_verifier_evidence(
    repository: &Repository,
    selector: Selector,
) -> Result<FullVerifierEvidenceAdmission, RunError> {
    let paths = repository
        .admit_state()
        .map_err(|error| RunError::fault("gate::full-verifier-state-unavailable", error))?;
    let document = match fs::read(paths.full_verifier_evidence()) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return unavailable(
                FullVerifierEvidenceStage::Document,
                "no full-verifier evidence is published for this worktree",
            );
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
        Err(detail) => return unavailable(FullVerifierEvidenceStage::Bundle, detail),
    };
    let semantic = match semantic::SemanticEvidence::decode(evidence.semantic.as_bytes()) {
        Ok(semantic) => semantic,
        Err(detail) => {
            return unavailable(FullVerifierEvidenceStage::SemanticDocument, detail);
        }
    };
    let verifier_environment = match exact_evidence_environment(repository) {
        Ok(environment) => environment,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Environment, error),
    };
    let mut prepared = match prepare_strict(selector) {
        Ok(prepared) => prepared,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Capture, error),
    };
    prepared.bind_exact_evidence_environment(verifier_environment);
    if semantic.source() != prepared.source() {
        return unavailable(
            FullVerifierEvidenceStage::Source,
            format!(
                "retained source {} does not match current source {}",
                semantic.source(),
                prepared.source(),
            ),
        );
    }
    let current_closure = match GateSemanticClosure::capture(
        prepared.source().clone(),
        prepared.root(),
        prepared.paths(),
        prepared.changes(),
        prepared.generated_paths(),
    ) {
        Ok(closure) => closure,
        Err(detail) => return unavailable(FullVerifierEvidenceStage::Closure, detail),
    };
    let state = match StateReceipt::decode(evidence.state.as_bytes()) {
        Ok(state) => state,
        Err(detail) => return unavailable(FullVerifierEvidenceStage::State, detail),
    };
    let battery =
        match prepared.admit_battery_receipt_after_state(evidence.battery.as_bytes(), &state) {
            Ok(battery) => battery,
            Err(error) => return unavailable(FullVerifierEvidenceStage::Battery, error),
        };
    let compiler = match prepared.admit_verifier_compiler_receipt(
        evidence.compiler.as_bytes(),
        evidence.cargo_jobs,
        &compiler_authority(repository),
    ) {
        Ok(compiler) => compiler,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Compiler, error),
    };
    if let Err(detail) = semantic.validate_exact(repository, &battery, &compiler, &current_closure)
    {
        return unavailable(FullVerifierEvidenceStage::Semantic, detail);
    }
    Ok(FullVerifierEvidenceAdmission::Admitted(Box::new(
        AdmittedFullVerifierEvidence {
            source: battery.tree().clone(),
            evidence: Digest::compute(Domain::GateReceipt, &document),
            battery,
            changes: prepared.changes().to_vec(),
        },
    )))
}

fn unavailable(
    stage: FullVerifierEvidenceStage,
    detail: impl ToString,
) -> Result<FullVerifierEvidenceAdmission, RunError> {
    Ok(FullVerifierEvidenceAdmission::Unavailable(
        FullVerifierEvidenceMiss::new(stage, detail),
    ))
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

/// Compose two independently admitted parent bundles for one open Git merge.
///
/// This is a separate admission surface from ordinary semantic reuse. Both
/// parent revisions are freshly captured, each archived bundle must be exact
/// for its parent, and only arms whose closure and portable authority match the
/// staged merge candidate are returned. Absence or drift is a cache miss, so
/// the caller executes the selected invariant normally.
pub fn admit_merge_parent_full_verifier_evidence(
    repository: &Repository,
) -> Result<SemanticFullVerifierAdmission, RunError> {
    let parents = match merge_parent_revisions(repository)? {
        MergeParentRevisions::Available(parents) => parents,
        MergeParentRevisions::Unavailable(detail) => {
            return Ok(SemanticFullVerifierAdmission::Unavailable { detail });
        }
    };
    let paths = repository
        .admit_state()
        .map_err(|error| RunError::fault("gate::full-verifier-state-unavailable", error))?;
    let archive = storage::LockedArchive::acquire(&paths)
        .map_err(|error| RunError::fault("gate::full-verifier-evidence-unavailable", error))?;
    let current = match prepare_merge_candidate(repository) {
        Ok(current) => current,
        Err(detail) => {
            return Ok(SemanticFullVerifierAdmission::Unavailable { detail });
        }
    };
    let mut admitted = Vec::with_capacity(parents.len());
    for parent in parents {
        match admit_archived_merge_parent(
            repository,
            &archive,
            paths.full_verifier_evidence_archive(),
            &parent,
            &current,
        )? {
            ArchivedMergeParent::Admitted(parent) => admitted.push(*parent),
            ArchivedMergeParent::Unavailable(detail) => {
                return Ok(SemanticFullVerifierAdmission::Unavailable { detail });
            }
        }
    }
    match AdmittedSemanticVerifierEvidence::compose_merge(admitted) {
        Ok(evidence) => Ok(SemanticFullVerifierAdmission::Admitted(Box::new(evidence))),
        Err(detail) => Ok(SemanticFullVerifierAdmission::Unavailable { detail }),
    }
}

enum MergeParentRevisions {
    Available([String; 2]),
    Unavailable(String),
}

fn merge_parent_revisions(repository: &Repository) -> Result<MergeParentRevisions, RunError> {
    let merge_head = match fs::read_to_string(repository.git_dir().join("MERGE_HEAD")) {
        Ok(head) => head,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(MergeParentRevisions::Unavailable(
                "no two-parent Git merge is open".to_owned(),
            ));
        }
        Err(error) => return Err(RunError::fault("gate::merge-parent-unavailable", error)),
    };
    let merge_heads = merge_head.lines().collect::<Vec<_>>();
    let canonical = merge_heads.len() == 1
        && matches!(merge_heads[0].len(), 40 | 64)
        && merge_heads[0]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !canonical {
        return Ok(MergeParentRevisions::Unavailable(
            "MERGE_HEAD does not name exactly one canonical parent".to_owned(),
        ));
    }
    Ok(MergeParentRevisions::Available([
        "HEAD".to_owned(),
        merge_heads[0].to_owned(),
    ]))
}

struct MergeCandidateEvidence {
    compiler: BTreeMap<GateId, Digest>,
    context: BatteryContext,
    closure: GateSemanticClosure,
}

fn prepare_merge_candidate(repository: &Repository) -> Result<MergeCandidateEvidence, String> {
    let mut prepared = prepare(Selector::staged()).map_err(|error| error.to_string())?;
    let repository_authority = semantic::repository_authority(repository);
    let compiler = prepared
        .merge_semantic_compiler_authorities(1, &repository_authority)
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect();
    let context = prepared.context().map_err(|error| error.to_string())?;
    let closure = GateSemanticClosure::capture(
        prepared.source().clone(),
        prepared.root(),
        prepared.paths(),
        prepared.changes(),
        prepared.generated_paths(),
    )?;
    Ok(MergeCandidateEvidence {
        compiler,
        context,
        closure,
    })
}

enum ArchivedMergeParent {
    Admitted(Box<AdmittedSemanticVerifierEvidence>),
    Unavailable(String),
}

fn admit_archived_merge_parent(
    repository: &Repository,
    locked_archive: &storage::LockedArchive,
    archive_root: &Path,
    parent: &str,
    current: &MergeCandidateEvidence,
) -> Result<ArchivedMergeParent, RunError> {
    let mut prepared = match prepare(Selector::revision(parent.to_owned())) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Ok(ArchivedMergeParent::Unavailable(format!(
                "capture merge parent {parent}: {error}"
            )));
        }
    };
    let archive = archive_root.join(format!("{}.json", prepared.source()));
    let document = match locked_archive.read(&archive) {
        Ok(Some(document)) => document,
        Ok(None) => {
            return Ok(ArchivedMergeParent::Unavailable(format!(
                "merge parent {parent} has no archived full-verifier evidence"
            )));
        }
        Err(error) => {
            return Err(RunError::fault(
                "gate::full-verifier-evidence-unavailable",
                error,
            ));
        }
    };
    let unavailable = |detail| Ok(ArchivedMergeParent::Unavailable(detail));
    let evidence = match MergeParentEvidence::decode(&document) {
        Ok(evidence) => evidence,
        Err(detail) => return unavailable(detail),
    };
    let semantic = match semantic::MergeSemanticEvidence::decode(evidence.semantic.as_bytes()) {
        Ok(semantic) => semantic,
        Err(detail) => return unavailable(detail),
    };
    if semantic.source() != prepared.source() {
        return unavailable(format!(
            "merge parent {parent} archive names another source"
        ));
    }
    match semantic.admit_merge_parent(
        repository,
        &mut prepared,
        &current.closure,
        &current.context,
        &current.compiler,
        evidence.cargo_jobs,
        evidence.battery.as_bytes(),
        evidence.compiler.as_bytes(),
        evidence.state.as_bytes(),
        Digest::compute(Domain::GateReceipt, &document),
    ) {
        Ok(parent) => Ok(ArchivedMergeParent::Admitted(Box::new(parent))),
        Err(detail) => unavailable(detail),
    }
}

fn compiler_authority(repository: &Repository) -> Vec<u8> {
    let path = repository.git_dir().as_os_str().as_encoded_bytes();
    framed_authority(AUTHORITY_PREFIX, path)
}

fn merge_compiler_authority(repository: &Repository) -> Vec<u8> {
    let path = repository.common_dir().as_os_str().as_encoded_bytes();
    framed_authority(MERGE_AUTHORITY_PREFIX, path)
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
            merge_parent: None,
        };
        let canonical = complete.encode().expect("encode complete fixture");
        let canonical_text = String::from_utf8(canonical.clone()).expect("canonical UTF-8");
        assert!(canonical_text.contains("\"version\":3"));
        assert!(!canonical_text.contains("merge_parent"));
        assert!(!canonical_text.contains("merge_compiler"));
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
    fn merge_parent_bundle_has_one_distinct_canonical_contract() {
        let complete = MergeParentEvidence {
            version: MERGE_PARENT_EVIDENCE_VERSION,
            cargo_jobs: 3,
            battery: "battery".to_owned(),
            compiler: "compiler".to_owned(),
            state: "state".to_owned(),
            semantic: "semantic".to_owned(),
        };
        let canonical = complete.encode().expect("encode merge-parent fixture");
        assert_eq!(
            MergeParentEvidence::decode(&canonical).expect("decode merge-parent fixture"),
            complete,
        );

        let mut noncanonical = canonical;
        noncanonical.push(b'\n');
        assert!(MergeParentEvidence::decode(&noncanonical).is_err());
        for partial in [
            MergeParentEvidence {
                compiler: String::new(),
                ..complete.clone()
            },
            MergeParentEvidence {
                battery: String::new(),
                ..complete.clone()
            },
            MergeParentEvidence {
                state: String::new(),
                ..complete.clone()
            },
            MergeParentEvidence {
                semantic: String::new(),
                ..complete.clone()
            },
            MergeParentEvidence {
                cargo_jobs: 0,
                ..complete.clone()
            },
            MergeParentEvidence {
                version: MERGE_PARENT_EVIDENCE_VERSION + 1,
                ..complete
            },
        ] {
            let bytes = partial
                .encode()
                .expect("encode invalid merge-parent fixture");
            assert!(MergeParentEvidence::decode(&bytes).is_err());
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
