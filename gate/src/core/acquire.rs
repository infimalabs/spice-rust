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

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use sill::{Argv, Completion, spawn};
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
    MetricId, MetricKey, MetricStateBinding, ParserEvidence, ParserId, measure_repository_document,
    resolve_metric_rule,
};
use crate::policy::PolicyContext;
use crate::state::{ActorInputs, BoundDecision, BoundError, LatchEntry, PeerClaim, owning_claim};
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
/// all. The gate holds no clock, so the caller reads the ledger under its own
/// run clock and hands over what was still active in that moment.
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
    /// The structural file-shape arm seeds the same way over its own selected
    /// paths, so this is visible to it rather than restated there: two
    /// spellings of the order above could disagree about which actor a path's
    /// headroom varies with, and each would look right beside its own arm.
    /// The refusal crosses whole for the same reason, and each caller names the
    /// family the path was selected for.
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
/// takes both Git-declared observations from the repository the tree was
/// materialized from. Every arm therefore binds to the same digest, and
/// [`CoreInputs`] cannot be assembled from a moving tree or a moving repository.
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
    /// its bound claims are: the gate holds no clock and opens no transaction
    /// here, so it decides against the reading the report already names rather
    /// than taking a second one of its own.
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
        // Taken before the walk because it is the only subject here that can
        // still move: the selected tree was sealed at materialization, while
        // the repository stays live and writable underneath the run.
        let git = GitObservations::capture(tree.repository(), contract.bounds());
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
}

/// Both Git-declared observations, taken once when the source is captured.
///
/// Acquisition projects these rather than running the declared vectors again,
/// so the repository is read exactly once, in the same capture that fixes the
/// identity the arms are labelled with. Neither observation can refuse the
/// capture: a Git failure is already typed evidence, and an arm that could not
/// be measured crosses saying so.
#[derive(Debug)]
struct GitObservations {
    merge_integrity: CoreObservation,
    staging: CoreObservation,
}

impl GitObservations {
    fn capture(repository: &Path, bounds: &Bounds) -> Self {
        Self {
            merge_integrity: merge_integrity(repository, bounds),
            staging: staging(repository, bounds),
        }
    }
}

/// Acquire one complete core-family battery from one captured source.
///
/// Every declaration in [`core_declarations`] contributes exactly one arm, in
/// compiled order. No arm is omitted, none is invented, and no standing is
/// computed here.
///
/// # Errors
///
/// Returns the first refusal that would leave the battery incomplete: a
/// provenance the declaration cannot supply, evidence that fails admission, or
/// an arm vector the family will not admit.
pub fn acquire_core(
    source: &CoreSource<'_>,
    contract: &Contract,
) -> Result<CoreInputs, CoreInputError> {
    let configuration = ConfigurationInventorySource::capture(source)?;
    let mut arms = Vec::with_capacity(core_declarations().len());
    let mut bounds = Vec::new();
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
            observe(declaration, source, contract, &configuration, &mut bounds)?,
        )?);
    }
    CoreInputs::new(arms).map(|inputs| inputs.with_bounds(bounds))
}

fn observe(
    declaration: &CoreArmDeclaration,
    source: &CoreSource<'_>,
    contract: &Contract,
    configuration: &ConfigurationInventorySource,
    bounds: &mut Vec<BoundDecision>,
) -> Result<CoreObservation, CoreInputError> {
    Ok(match declaration.id() {
        GateId::MergeIntegrity => source.git.merge_integrity.clone(),
        GateId::PlanPhase => plan_phase(&source.claim),
        GateId::RepoShape => return repo_shape(source, contract),
        GateId::Staging => source.git.staging.clone(),
        GateId::ConfigKeyValidity => return config_keys(contract),
        GateId::RepoDocs => return repo_docs(source, contract, bounds),
        GateId::ConfigFalseDisable => return configuration.false_disable(),
        GateId::ConfigTrackedTrust => return configuration.tracked_trust(),
        id => unreachable!("{id} is not a compiled core declaration"),
    })
}

fn repo_docs(
    source: &CoreSource<'_>,
    contract: &Contract,
    bounds: &mut Vec<BoundDecision>,
) -> Result<CoreObservation, CoreInputError> {
    let mut measurements = Vec::new();
    for entry in source.jurisdiction.measurable() {
        let path = entry.path();
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
        let (measurement, decision) = document_measurement(
            &source.actor,
            contract,
            &source.latches,
            path,
            document.characters(),
        )?;
        measurements.push(measurement);
        bounds.push(decision);
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
/// ledger this run published as its state authority, so the arm and the report
/// answer to one reading.
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
) -> Result<(DocMeasurement, BoundDecision), CoreInputError> {
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
    let latched = latches.contains(binding.entry());
    let actor_inputs = actor.actor_inputs(path).map_err(|error| {
        CoreInputError::new(format!(
            "actor provenance for repository document {path:?}: {error}"
        ))
    })?;
    let decision = binding.decide(latched, &actor_inputs).map_err(|error| {
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
    Ok((measurement, decision))
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

fn merge_integrity(repository: &Path, bounds: &Bounds) -> CoreObservation {
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

    let staged_tree = match git_tree(repository, bounds, &["write-tree"]) {
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
    let (_, output) = git(repository, bounds, arguments, &[0])?;
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
    let argv = Argv::new(GIT, arguments.iter().map(OsStr::new))
        .map_err(|error| GitRefusal::Unmeasured(format!("git argument vector: {error}")))?;
    let outcome = spawn(bounds.spec(argv, repository))
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
