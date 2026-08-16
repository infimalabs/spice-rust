//! Acquisition of one complete core-family input battery.
//!
//! [`super::evaluate_core`] decides; this module only gathers. Every compiled
//! declaration produces exactly one observation, in compiled order, whether or
//! not its provider could measure. An arm this build cannot observe crosses as
//! an explicit non-completion state carrying its own recovery, so a missing
//! provider is visible in the report instead of being an omitted row.
//!
//! Two arms declare exact `git` argument vectors. They run once, while the
//! source is captured, under the contract's process bounds, and every process
//! failure becomes typed evidence rather than an acquisition error. The only
//! failures that refuse outright are the ones that would leave the battery
//! incomplete: an unreadable selected tree and a malformed arm.
//!
//! Acquisition is handed no repository. Every arm reads either the immutable
//! selected tree or an observation the capture already took, so a repository
//! mutated or swapped after capture has nothing left to reach: there is no
//! later state that can cross into a report under the captured digest.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use sill::{Argv, Completion, Environment, spawn};
use wire::{Digest, Phase, TaskHandle};

use super::admission::{validate_git_tree, validate_repo_path};
use super::config_keys::config_keys;
use super::configuration_inventory::ConfigurationInventorySource;
use super::declaration::CoreArmDeclaration;
use super::shape::repo_shape;
use super::{
    CoreArmInput, CoreEvidence, CoreInputError, CoreInputs, CoreObservation, CoreProvenance,
    DocMeasurement, MergeIntegrityEvidence, MergeTreeCompletion, PlanPhaseEvidence,
    RepoDocsEvidence, StagingEvidence, core_declarations,
};
use crate::GateId;
use crate::input::Contract;
use crate::jurisdiction::Jurisdiction;
use crate::measure::Bounds;
use crate::metrics::{
    BaselineCapture, MetricId, MetricKey, MetricStateBinding, ParserEvidence, ParserId,
    measure_repository_document, resolve_metric_rule,
};
use crate::policy::PolicyContext;
use crate::state::{
    ActorInputs, BoundDecision, BoundError, LatchEntry, LatchObservation, PeerClaim, owning_claim,
};
use crate::tree::SelectedTree;

/// The one program both Git-declared arms observe through.
///
/// Compiled absolutely, like every other gate tool, so acquisition reads no
/// ambient search path and two machines cannot silently observe two different
/// programs. A build without it starts nothing and says so.
const GIT: &str = "/usr/bin/git";

/// The board head captured with the source, before acquisition begins.
///
/// The gate holds no journal, so the claim is captured by the caller that owns
/// one and handed over whole. Absence is stated two different ways because a
/// store that answered "nothing is claimed" and a repository with no store at
/// all are different observations, and only the first is a measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimCapture {
    /// One task is claimed in this worktree, at this workflow phase.
    Active {
        /// Claimed task.
        task: TaskHandle,
        /// Current workflow phase of that claim.
        phase: Phase,
    },
    /// The claim store answered and holds no active claim.
    Idle,
    /// No claim store answered for this repository.
    Absent {
        /// Why no store answered.
        reason: String,
    },
}

/// The actor acting in this worktree, captured with the source.
///
/// Elastic headroom varies per actor, so acquisition needs an identity the
/// selected tree cannot supply: the acting actor lives in the live worktree the
/// gate does not read, exactly like the board head above. The caller that owns
/// one hands it over whole, and a run with no acting actor says so rather than
/// borrowing another input to stand in for one.
///
/// The active bound claims ride with the actor because a claim is peer-held
/// only relative to someone: with no actor there is no peer to be outranked by,
/// which is exactly the point at which the predecessor resolves no claims at
/// all. Production capture replaces this caller observation with the active
/// claims from its fixed-clock [`crate::BatteryStateRun`]; direct acquisition
/// tests may still provide the typed observation explicitly.
#[derive(Clone, Debug, PartialEq)]
pub enum ActorCapture {
    /// One actor is acting in this worktree.
    Acting {
        /// Identity that seeds this actor's headroom.
        actor: String,
        /// Bound claims the ledger held active when the caller read it.
        bound_claims: Vec<PeerClaim>,
    },
    /// No actor is acting in this worktree.
    Absent,
}

impl ActorCapture {
    /// The identity acting in this worktree, when one is.
    #[must_use]
    pub fn active_actor(&self) -> Option<&str> {
        match self {
            Self::Acting { actor, .. } => Some(actor),
            Self::Absent => None,
        }
    }

    /// Present one candidate path's provenance in the documented order.
    ///
    /// [`ActorInputs`] is the single resolver both this family and every bound
    /// rule seed through. The gate observes no published trailer and no
    /// candidate blob, so an acting actor is the only member above the path
    /// this capture can present, and its absence falls to the candidate path
    /// the order already names last.
    ///
    /// The provenance crosses whole rather than pre-resolved to a seed. The
    /// bound rule resolves it itself, and resolving it here as well would let
    /// this family and the rule that decides its limit name two different
    /// actors for one document.
    ///
    /// The structural file-shape and complexity arms seed the same way over
    /// their own selected paths, so this is visible to them rather than
    /// restated there: two spellings of the order above could disagree about
    /// which actor a path's headroom varies with, and each would look right
    /// beside its own arm. The refusal crosses whole for the same reason, and
    /// each caller names the family the path was selected for.
    pub(crate) fn actor_inputs(&self, path: &str) -> Result<ActorInputs, BoundError> {
        let inputs = ActorInputs::new(path)?;
        Ok(match self {
            Self::Acting { actor, .. } => inputs.with_active_actor(actor),
            Self::Absent => inputs,
        })
    }

    /// Whether a peer holds the bound claim that governs `path`.
    ///
    /// Ownership is resolved by the ledger's own rule, so this acquisition and
    /// the state transaction that redirects a claimant name the same owner for
    /// one path. The actor is trimmed for the same reason the transaction
    /// trims its claimant: a claim is held by an identity, not by a spelling
    /// with surrounding space.
    fn peer_holds(&self, path: &str) -> bool {
        match self {
            Self::Acting {
                actor,
                bound_claims,
            } => {
                owning_claim(bound_claims, path).is_some_and(|claim| claim.actor() != actor.trim())
            }
            Self::Absent => false,
        }
    }
}

/// One captured Git source and everything acquisition is permitted to read.
///
/// Capture computes the immutable tree identity once and, in the same moment,
/// takes either both Git-declared observations for a complete run or the one a
/// selected arm requires. Every acquired arm therefore binds to the same
/// digest, and [`CoreInputs`] cannot be assembled from a moving tree or a
/// moving repository.
///
/// The durable latches are copied in for the same reason: a bounded arm decides
/// against the ledger this run publishes as its state authority, and reading
/// that ledger again mid-acquisition could answer from a version the report
/// never names.
#[derive(Debug)]
pub struct CoreSource<'a> {
    root: &'a Path,
    jurisdiction: Jurisdiction,
    digest: Digest,
    claim: ClaimCapture,
    actor: ActorCapture,
    git: GitObservations,
    latches: Vec<LatchEntry>,
    reached: Option<BTreeSet<String>>,
}

impl<'a> CoreSource<'a> {
    /// Capture one immutable source for the core family.
    ///
    /// The repository is taken from the tree rather than accepted beside it, so
    /// no caller can name one source for the identity and another for the Git
    /// evidence.
    ///
    /// `latches` are the durable breaches the caller read from the ledger it
    /// publishes as this run's state authority, handed over for the same reason
    /// its bound claims are: the enclosing battery state transaction supplies
    /// the one run-start reading the report names, rather than allowing this
    /// family to take a second one of its own.
    ///
    /// # Errors
    ///
    /// Returns the walk or read failure that prevented one complete tree
    /// identity. A partially digested tree is never returned.
    pub fn capture(
        tree: &'a SelectedTree,
        contract: &Contract,
        claim: ClaimCapture,
        actor: ActorCapture,
        latches: &[LatchEntry],
    ) -> Result<Self, CoreInputError> {
        Self::capture_inner(tree, contract, claim, actor, latches, None, None)
    }

    /// Capture only the live Git observation required by one selected arm.
    pub(crate) fn capture_arm(
        tree: &'a SelectedTree,
        contract: &Contract,
        claim: ClaimCapture,
        actor: ActorCapture,
        latches: &[LatchEntry],
        gate_id: GateId,
        reached: Option<&[String]>,
    ) -> Result<Self, CoreInputError> {
        Self::capture_inner(
            tree,
            contract,
            claim,
            actor,
            latches,
            Some(gate_id),
            reached,
        )
    }

    fn capture_inner(
        tree: &'a SelectedTree,
        contract: &Contract,
        claim: ClaimCapture,
        actor: ActorCapture,
        latches: &[LatchEntry],
        gate_id: Option<GateId>,
        reached: Option<&[String]>,
    ) -> Result<Self, CoreInputError> {
        // Taken before the walk because it is the only subject here that can
        // still move: the selected tree was sealed at materialization, while
        // the repository stays live and writable underneath the run.
        let git = gate_id.map_or_else(
            || {
                GitObservations::capture(
                    tree.repository(),
                    tree.captured_index(),
                    contract.bounds(),
                )
            },
            |gate_id| {
                GitObservations::capture_arm(
                    tree.repository(),
                    tree.captured_index(),
                    contract.bounds(),
                    gate_id,
                )
            },
        );
        let jurisdiction = Jurisdiction::walk(tree.root(), contract.policy().ignore())
            .map_err(|error| CoreInputError::new(format!("core source jurisdiction: {error}")))?;
        let digest = jurisdiction
            .tree_digest()
            .map_err(|error| CoreInputError::new(format!("core source tree identity: {error}")))?;
        Ok(Self {
            root: tree.root(),
            jurisdiction,
            digest,
            claim,
            actor,
            git,
            latches: latches.to_vec(),
            reached: reached.map(|paths| paths.iter().cloned().collect()),
        })
    }

    /// Immutable selected-tree root every compiled provider reads.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
    }

    /// Admitted inventory that produced this source identity.
    pub(super) const fn jurisdiction(&self) -> &Jurisdiction {
        &self.jurisdiction
    }

    /// Identity every acquired arm is bound to.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Board head captured with this source.
    #[must_use]
    pub const fn claim(&self) -> &ClaimCapture {
        &self.claim
    }

    /// Acting actor captured with this source.
    #[must_use]
    pub const fn actor(&self) -> &ActorCapture {
        &self.actor
    }

    /// Git's exact tree for a clean two-parent synthesis, when one was proven.
    ///
    /// This private capture evidence lets structural acquisition distinguish a
    /// file Git synthesized from a file the merge receiver edited. Conflicted,
    /// octopus, absent, and unmeasured merges return no tree and therefore grant
    /// no authorship exemption.
    #[must_use]
    pub(crate) fn clean_merge_tree(&self) -> Option<(&str, &str, &str)> {
        self.git.clean_merge_tree.as_ref().map(|capture| {
            (
                capture.head.as_str(),
                capture.merging.as_str(),
                capture.tree.as_str(),
            )
        })
    }
}

/// The two exact parent objects and clean tree one Git invocation joined.
#[derive(Debug)]
struct CleanMergeTree {
    head: String,
    merging: String,
    tree: String,
}

/// Git observations taken once when the source is captured.
///
/// Acquisition projects these rather than running the declared vectors again,
/// so the repository is read exactly once, in the same capture that fixes the
/// identity the arms are labelled with. The two declared observations cross as
/// typed evidence; the private clean-merge tree is optional and can only grant
/// a narrow authorship exemption when Git proved it.
#[derive(Debug)]
struct GitObservations {
    merge_integrity: Option<CoreObservation>,
    staging: Option<CoreObservation>,
    clean_merge_tree: Option<CleanMergeTree>,
}

impl GitObservations {
    fn capture(repository: &Path, captured_index: Option<&Path>, bounds: &Bounds) -> Self {
        Self {
            merge_integrity: Some(merge_integrity(repository, captured_index, bounds)),
            staging: Some(staging(repository, bounds)),
            clean_merge_tree: clean_merge_tree(repository, bounds),
        }
    }

    fn capture_arm(
        repository: &Path,
        captured_index: Option<&Path>,
        bounds: &Bounds,
        gate_id: GateId,
    ) -> Self {
        Self {
            merge_integrity: (gate_id == GateId::MergeIntegrity)
                .then(|| merge_integrity(repository, captured_index, bounds)),
            staging: (gate_id == GateId::Staging).then(|| staging(repository, bounds)),
            clean_merge_tree: (gate_id == GateId::FileShape)
                .then(|| clean_merge_tree(repository, bounds))
                .flatten(),
        }
    }
}

/// Acquire one complete core-family battery from one captured source.
///
/// Every declaration in [`core_declarations`] contributes exactly one arm, in
/// compiled order. No arm is omitted, none is invented, and no standing is
/// computed here. `baselines` must be the committed capture taken beside
/// `source`; repository-document debt is a comparison between those two
/// immutable inputs, not a later read of the live repository.
///
/// # Errors
///
/// Returns the first refusal that would leave the battery incomplete: a
/// provenance the declaration cannot supply, evidence that fails admission, or
/// an arm vector the family will not admit.
pub fn acquire_core(
    source: &CoreSource<'_>,
    contract: &Contract,
    baselines: &BaselineCapture,
) -> Result<CoreInputs, CoreInputError> {
    acquire_core_with_observations(source, contract, baselines).map(|(inputs, _)| inputs)
}

/// Acquire the complete core family and retain its exact state observations.
pub(crate) fn acquire_core_with_observations(
    source: &CoreSource<'_>,
    contract: &Contract,
    baselines: &BaselineCapture,
) -> Result<(CoreInputs, Vec<LatchObservation>), CoreInputError> {
    let configuration = ConfigurationInventorySource::capture(source)?;
    let mut arms = Vec::with_capacity(core_declarations().len());
    let mut bounds = Vec::new();
    let mut observations = Vec::new();
    for declaration in core_declarations() {
        let predecessor = declaration.predecessor();
        let provenance = CoreProvenance::new(
            predecessor.release(),
            predecessor.source(),
            predecessor.observation(),
        )?;
        arms.push(CoreArmInput::new(
            declaration.id(),
            source.digest.clone(),
            provenance,
            observe(
                declaration,
                source,
                contract,
                baselines,
                Some(&configuration),
                &mut bounds,
                &mut observations,
            )?,
        )?);
    }
    CoreInputs::new(arms).map(|inputs| (inputs.with_bounds(bounds), observations))
}

/// Acquire exactly one core arm and the elastic bounds that arm decided.
pub(crate) fn acquire_core_arm(
    source: &CoreSource<'_>,
    contract: &Contract,
    baselines: &BaselineCapture,
    gate_id: GateId,
) -> Result<(CoreArmInput, Vec<BoundDecision>, Vec<LatchObservation>), CoreInputError> {
    let declaration = core_declarations()
        .iter()
        .find(|declaration| declaration.id() == gate_id)
        .ok_or_else(|| CoreInputError::new(format!("{gate_id} is not a compiled core arm")))?;
    let configuration = matches!(
        gate_id,
        GateId::ConfigFalseDisable | GateId::ConfigTrackedTrust
    )
    .then(|| ConfigurationInventorySource::capture(source))
    .transpose()?;
    let predecessor = declaration.predecessor();
    let provenance = CoreProvenance::new(
        predecessor.release(),
        predecessor.source(),
        predecessor.observation(),
    )?;
    let mut bounds = Vec::new();
    let mut observations = Vec::new();
    let observation = observe(
        declaration,
        source,
        contract,
        baselines,
        configuration.as_ref(),
        &mut bounds,
        &mut observations,
    )?;
    CoreArmInput::new(gate_id, source.digest.clone(), provenance, observation)
        .map(|input| (input, bounds, observations))
}

fn observe(
    declaration: &CoreArmDeclaration,
    source: &CoreSource<'_>,
    contract: &Contract,
    baselines: &BaselineCapture,
    configuration: Option<&ConfigurationInventorySource>,
    bounds: &mut Vec<BoundDecision>,
    observations: &mut Vec<LatchObservation>,
) -> Result<CoreObservation, CoreInputError> {
    Ok(match declaration.id() {
        GateId::MergeIntegrity => source.git.merge_integrity.clone().ok_or_else(|| {
            CoreInputError::new("merge-integrity acquisition omitted its Git observation")
        })?,
        GateId::PlanPhase => plan_phase(&source.claim),
        GateId::RepoShape => return repo_shape(source, contract),
        GateId::Staging => source.git.staging.clone().ok_or_else(|| {
            CoreInputError::new("staging acquisition omitted its Git observation")
        })?,
        GateId::ConfigKeyValidity => return config_keys(contract),
        GateId::RepoDocs => {
            return repo_docs(source, contract, baselines, bounds, observations);
        }
        GateId::ConfigFalseDisable => {
            return configuration
                .ok_or_else(|| {
                    CoreInputError::new("false-disable acquisition omitted its configuration")
                })?
                .false_disable();
        }
        GateId::ConfigTrackedTrust => {
            return configuration
                .ok_or_else(|| {
                    CoreInputError::new("tracked-trust acquisition omitted its configuration")
                })?
                .tracked_trust();
        }
        id => unreachable!("{id} is not a compiled core declaration"),
    })
}

fn repo_docs(
    source: &CoreSource<'_>,
    contract: &Contract,
    baselines: &BaselineCapture,
    bounds: &mut Vec<BoundDecision>,
    observations: &mut Vec<LatchObservation>,
) -> Result<CoreObservation, CoreInputError> {
    if let BaselineCapture::Unreachable(reason) = baselines {
        return Err(CoreInputError::new(format!(
            "committed repository-document parents could not be read: {reason}"
        )));
    }
    let mut measurements = Vec::new();
    for entry in source.jurisdiction.measurable() {
        let path = entry.path();
        if source
            .reached
            .as_ref()
            .is_some_and(|reached| !reached.contains(path))
        {
            continue;
        }
        let selected = contract
            .rule_policy()
            .selects_repository_document(path)
            .map_err(|error| {
                CoreInputError::new(format!(
                    "repository-document selection for {path:?}: {error}"
                ))
            })?;
        if !selected {
            continue;
        }
        let bytes = fs::read(source.root.join(path)).map_err(|error| {
            CoreInputError::new(format!(
                "read selected repository document {path:?}: {error}"
            ))
        })?;
        let document = measure_repository_document(path, &bytes).map_err(|error| {
            CoreInputError::new(format!(
                "measure selected repository document {path:?}: {error}"
            ))
        })?;
        let committed = baselines
            .baseline_bytes(path, &bytes)
            .filter(|committed| *committed != bytes)
            .and_then(|committed| measure_repository_document(path, committed).ok())
            .map(|document| document.characters());
        let (measurement, decision, observation) = document_measurement(
            &source.actor,
            contract,
            &source.latches,
            path,
            document.characters(),
            committed,
        )?;
        measurements.push(measurement);
        bounds.push(decision);
        observations.push(observation);
    }
    RepoDocsEvidence::new(measurements)
        .map(CoreEvidence::RepoDocs)
        .map(measured)
}

/// Decide one selected document's bound against the acting actor's headroom.
///
/// The decision is taken through [`MetricStateBinding`], which is the same
/// route [`crate::state::CurrentState`] reconciles a durable latch through, so
/// the limit this arm is judged against and the limit the ledger would latch
/// are one computation rather than two spellings of it. That binding resolves
/// the rule, the ratchet variation, and the `repo_doc_chars` latch key from the
/// tracked contract, so nothing here restates a policy field.
///
/// The headroom seed is the actor's, never the document's own path: a seed
/// taken from the path hands every actor one identical cap, which is the one
/// thing per-actor variation exists to prevent. The path remains the first hash
/// input, so a single actor still receives a different cap per document.
///
/// A latched document is held to its base limit, because a breach already
/// spent the elastic headroom the ratchet lends. The latch is read from the
/// ledger this run published as its state authority; changed committed bytes
/// already above base infers the same state before a first durable observation
/// can exist. The arm and the report therefore answer to one debt rule whether
/// the prior breach is visible in the ledger or in the committed document.
///
/// A bound claim is resolved only for a document that breached its cap, which
/// is the same set the predecessor asks the ledger about: a document inside its
/// bound claims nothing and can be held by nobody. An unlimited document is
/// never claimed for the same reason, since it has no cap left to breach.
fn document_measurement(
    actor: &ActorCapture,
    contract: &Contract,
    latches: &[LatchEntry],
    path: &str,
    characters: u64,
    committed: Option<u64>,
) -> Result<(DocMeasurement, BoundDecision, LatchObservation), CoreInputError> {
    let rule = resolve_metric_rule(
        contract.rule_policy(),
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path(path),
    )
    .map_err(|error| {
        CoreInputError::new(format!(
            "resolve repository-document bound for {path:?}: {error}"
        ))
    })?;
    let committed_over_base = rule.changed_committed_base_breach(committed);
    let key = MetricKey::path(path)
        .map_err(|error| CoreInputError::new(format!("repository-document key: {error}")))?;
    let binding = MetricStateBinding::new(
        MetricId::RepositoryDocumentCharacters,
        key,
        characters,
        ParserEvidence::Native {
            parser: ParserId::Utf8DocumentCharactersV1,
        },
        rule,
    )
    .map_err(|error| CoreInputError::new(format!("bind repository-document {path:?}: {error}")))?;
    let latched = latches.contains(binding.entry()) || committed_over_base;
    let actor_inputs = actor.actor_inputs(path).map_err(|error| {
        CoreInputError::new(format!(
            "actor provenance for repository document {path:?}: {error}"
        ))
    })?;
    let (decision, observation) =
        binding
            .observe_decision(latched, &actor_inputs)
            .map_err(|error| {
                CoreInputError::new(format!(
                    "resolve active repository-document bound for {path:?}: {error}"
                ))
            })?;
    let measurement = if decision.unlimited() {
        DocMeasurement::unlimited(path, characters)?
    } else {
        let peer_claim = decision.over_limit() && actor.peer_holds(path);
        DocMeasurement::new(path, characters, decision.active_limit(), peer_claim)?
    };
    Ok((measurement, decision, observation))
}

fn plan_phase(claim: &ClaimCapture) -> CoreObservation {
    match claim {
        ClaimCapture::Active { task, phase } => {
            measured(CoreEvidence::PlanPhase(PlanPhaseEvidence::Active {
                task: task.clone(),
                phase: phase.clone(),
            }))
        }
        ClaimCapture::Idle => measured(CoreEvidence::PlanPhase(PlanPhaseEvidence::NoActiveClaim)),
        ClaimCapture::Absent { reason } => CoreObservation::Skipped {
            reason: reason.clone(),
        },
    }
}

fn merge_integrity(
    repository: &Path,
    captured_index: Option<&Path>,
    bounds: &Bounds,
) -> CoreObservation {
    let merge_head = match git(
        repository,
        bounds,
        &["rev-parse", "--git-path", "MERGE_HEAD"],
        &[0],
    ) {
        Ok((_, output)) => output,
        Err(refusal) => return refusal.into_observation(),
    };
    if !repository.join(merge_head.trim()).exists() {
        return measured(CoreEvidence::MergeIntegrity(
            MergeIntegrityEvidence::NoMerge,
        ));
    }

    let staged_tree = match git_tree_with_index(repository, captured_index, bounds, &["write-tree"])
    {
        Ok(tree) => tree,
        Err(refusal) => return refusal.into_observation(),
    };
    let head_tree = match git_tree(repository, bounds, &["rev-parse", "HEAD^{tree}"]) {
        Ok(tree) => tree,
        Err(refusal) => return refusal.into_observation(),
    };
    if staged_tree != head_tree {
        return measured(CoreEvidence::MergeIntegrity(
            MergeIntegrityEvidence::StagedContent {
                head_tree,
                staged_tree,
            },
        ));
    }

    let arguments = &[
        "merge-tree",
        "--write-tree",
        "-z",
        "--name-only",
        "HEAD",
        "MERGE_HEAD",
    ];
    let (code, output) = match git(repository, bounds, arguments, &[0, 1]) {
        Ok(pair) => pair,
        Err(refusal) => return refusal.into_observation(),
    };
    let completion = if code == 0 {
        MergeTreeCompletion::Clean
    } else {
        MergeTreeCompletion::Conflicted
    };
    match parse_merge_tree(&output) {
        Ok((expected_tree, conflicted_paths)) => measured(CoreEvidence::MergeIntegrity(
            MergeIntegrityEvidence::Compared {
                head_tree,
                expected_tree,
                completion,
                conflicted_paths,
            },
        )),
        Err(refusal) => refusal.into_observation(),
    }
}

/// Compute Git's clean two-parent result without granting it gate standing.
///
/// The merge-integrity arm only needs `merge-tree` when the staged tree still
/// equals `HEAD`; file authorship needs the clean result even when the pending
/// index correctly contains it. This observation therefore rides beside the
/// public core evidence. Only exit zero proves an automatic clean synthesis.
/// A conflict, multiple merge heads, unreadable state, or process failure is a
/// conservative absence: structural arms keep measuring the staged bytes.
fn clean_merge_tree(repository: &Path, bounds: &Bounds) -> Option<CleanMergeTree> {
    let (_, merge_head_path) = git(
        repository,
        bounds,
        &["rev-parse", "--git-path", "MERGE_HEAD"],
        &[0],
    )
    .ok()?;
    let merge_heads = fs::read_to_string(repository.join(merge_head_path.trim())).ok()?;
    let mut merge_heads = merge_heads
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let merging = merge_heads.next()?;
    if merge_heads.next().is_some() {
        return None;
    }
    let (_, head) = git(repository, bounds, &["rev-parse", "HEAD"], &[0]).ok()?;
    let head = head.trim().to_owned();
    validate_git_tree(&head).ok()?;
    let (code, output) = git(
        repository,
        bounds,
        &["merge-tree", "--write-tree", &head, merging],
        &[0, 1],
    )
    .ok()?;
    if code != 0 {
        return None;
    }
    let tree = output.lines().next()?.trim().to_owned();
    validate_git_tree(&tree).ok()?;
    Some(CleanMergeTree {
        head,
        merging: merging.to_owned(),
        tree,
    })
}

/// Split `--write-tree -z --name-only` output into its tree and conflict names.
///
/// The record separator is NUL and the conflicted-name section is terminated by
/// an empty record, after which the remainder is informational text this gate
/// does not interpret. Every name is put through the same admission the family
/// applies to any repository path, so an unusable name is unmeasured evidence
/// rather than an arm that refuses the whole battery.
fn parse_merge_tree(output: &str) -> Result<(String, Vec<String>), GitRefusal> {
    let mut records = output.split('\0');
    let tree = records.next().unwrap_or_default().trim().to_owned();
    validate_git_tree(&tree).map_err(|error| {
        GitRefusal::Unmeasured(format!("git merge-tree computed no tree: {error}"))
    })?;
    let mut conflicted: Vec<String> = records
        .take_while(|record| !record.is_empty())
        .map(|record| record.trim_matches('\n').to_owned())
        .filter(|record| !record.is_empty())
        .collect();
    conflicted.sort();
    conflicted.dedup();
    for path in &conflicted {
        validate_repo_path(path).map_err(|error| {
            GitRefusal::Unmeasured(format!("git merge-tree named an unusable path: {error}"))
        })?;
    }
    Ok((tree, conflicted))
}

fn git_tree(repository: &Path, bounds: &Bounds, arguments: &[&str]) -> Result<String, GitRefusal> {
    git_tree_with_environment(repository, bounds, arguments, Environment::empty())
}

fn git_tree_with_index(
    repository: &Path,
    captured_index: Option<&Path>,
    bounds: &Bounds,
    arguments: &[&str],
) -> Result<String, GitRefusal> {
    let environment = captured_index.map_or_else(
        || Ok(Environment::empty()),
        |index| {
            Environment::new([("GIT_INDEX_FILE", index.as_os_str())])
                .map_err(|error| GitRefusal::Unmeasured(format!("Git index environment: {error}")))
        },
    )?;
    git_tree_with_environment(repository, bounds, arguments, environment)
}

fn git_tree_with_environment(
    repository: &Path,
    bounds: &Bounds,
    arguments: &[&str],
    environment: Environment,
) -> Result<String, GitRefusal> {
    let (_, output) = git_with_environment(repository, bounds, arguments, &[0], environment)?;
    let tree = output.trim().to_owned();
    validate_git_tree(&tree).map_err(|error| {
        GitRefusal::Unmeasured(format!(
            "git {} named no tree: {error}",
            arguments.join(" ")
        ))
    })?;
    Ok(tree)
}

fn staging(repository: &Path, bounds: &Bounds) -> CoreObservation {
    let arguments = &["diff", "--cached", "--name-only", "--diff-filter=ACMR"];
    let staged = match git(repository, bounds, arguments, &[0]) {
        Ok((_, output)) => output,
        Err(refusal) => return refusal.into_observation(),
    };
    let unstaged = match git(repository, bounds, &["diff", "--name-only"], &[0]) {
        Ok((_, output)) => output,
        Err(refusal) => return refusal.into_observation(),
    };
    let unstaged: Vec<&str> = unstaged.lines().map(str::trim).collect();
    let partial = staged
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty() && unstaged.contains(path));
    match StagingEvidence::new(partial) {
        Ok(evidence) => measured(CoreEvidence::Staging(evidence)),
        Err(error) => CoreObservation::Unmeasured {
            reason: format!("git named an unusable path: {error}"),
        },
    }
}

const fn measured(evidence: CoreEvidence) -> CoreObservation {
    CoreObservation::Measured { evidence }
}

/// Why one declared Git observation did not complete.
enum GitRefusal {
    Unavailable(String),
    TimedOut(u64),
    Unmeasured(String),
}

impl GitRefusal {
    fn into_observation(self) -> CoreObservation {
        match self {
            Self::Unavailable(reason) => CoreObservation::Unavailable { reason },
            Self::TimedOut(milliseconds) => CoreObservation::TimedOut { milliseconds },
            Self::Unmeasured(reason) => CoreObservation::Unmeasured { reason },
        }
    }
}

/// Run one declared Git observation and classify how it completed.
///
/// A non-accepted exit code, a lost or truncated capture, and a deadline are
/// each their own typed evidence. None of them is a measurement, and none of
/// them refuses the battery.
fn git(
    repository: &Path,
    bounds: &Bounds,
    arguments: &[&str],
    accepted: &[i32],
) -> Result<(i32, String), GitRefusal> {
    git_with_environment(
        repository,
        bounds,
        arguments,
        accepted,
        Environment::empty(),
    )
}

fn git_with_environment(
    repository: &Path,
    bounds: &Bounds,
    arguments: &[&str],
    accepted: &[i32],
    environment: Environment,
) -> Result<(i32, String), GitRefusal> {
    let argv = Argv::new(GIT, arguments.iter().map(OsStr::new))
        .map_err(|error| GitRefusal::Unmeasured(format!("git argument vector: {error}")))?;
    let mut spec = bounds.spec(argv, repository);
    spec.environment = environment;
    let outcome = spawn(spec)
        .map_err(|error| GitRefusal::Unavailable(format!("git did not start: {error}")))?
        .wait()
        .map_err(|error| GitRefusal::Unmeasured(format!("git was not reaped: {error}")))?;

    let named = arguments.join(" ");
    let status = match outcome.completion() {
        Completion::Exited(status) => *status,
        Completion::TimedOut(_) => {
            let milliseconds = bounds.deadlines().runtime().as_millis();
            let milliseconds = u64::try_from(milliseconds).unwrap_or(u64::MAX);
            return Err(GitRefusal::TimedOut(milliseconds.max(1)));
        }
        Completion::Cancelled(_) => {
            return Err(GitRefusal::Unmeasured(format!("git {named} was cancelled")));
        }
    };
    let code = status
        .code()
        .ok_or_else(|| GitRefusal::Unmeasured(format!("git {named} left no exit code")))?;
    if !accepted.contains(&code) {
        return Err(GitRefusal::Unmeasured(format!("git {named} exited {code}")));
    }

    let captured = outcome
        .stdout()
        .ok_or_else(|| GitRefusal::Unmeasured(format!("git {named} returned no stdout")))?;
    if captured.truncated() || !captured.complete() {
        return Err(GitRefusal::Unmeasured(format!(
            "git {named} stdout was truncated={} complete={}",
            captured.truncated(),
            captured.complete()
        )));
    }
    Ok((code, String::from_utf8_lossy(captured.bytes()).into()))
}
