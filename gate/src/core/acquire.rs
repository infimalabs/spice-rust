//! Acquisition of one complete core-family input battery.
//!
//! [`super::evaluate_core`] decides; this module only gathers. Every compiled
//! declaration produces exactly one observation, in compiled order, whether or
//! not its provider could measure. An arm this build cannot observe crosses as
//! an explicit non-completion state carrying its own recovery, so a missing
//! provider is visible in the report instead of being an omitted row.
//!
//! Two arms declare exact `git` argument vectors. They observe the repository
//! the source was captured from, under the contract's process bounds, and every
//! process failure becomes typed evidence rather than an acquisition error. The
//! only failures that refuse outright are the ones that would leave the battery
//! incomplete: an unreadable selected tree and a malformed arm.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, spawn};
use wire::{Digest, Phase, TaskHandle};

use super::admission::{validate_git_tree, validate_repo_path};
use super::declaration::CoreArmDeclaration;
use super::{
    CoreArmInput, CoreEvidence, CoreInputError, CoreInputs, CoreObservation, CoreProvenance,
    DocMeasurement, MergeIntegrityEvidence, MergeTreeCompletion, PlanPhaseEvidence,
    RepoDocsEvidence, StagingEvidence, core_declarations,
};
use crate::GateId;
use crate::input::Contract;
use crate::jurisdiction::Jurisdiction;
use crate::measure::Bounds;
use crate::metrics::measure_repository_document;
use crate::policy::{BoundName, PolicyContext};
use crate::state::varied_elastic_limit;
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

/// One captured Git source and everything acquisition is permitted to read.
///
/// Capture computes the immutable tree identity once, so every arm binds to the
/// same digest and [`CoreInputs`] cannot be assembled from a moving tree.
#[derive(Debug)]
pub struct CoreSource<'a> {
    repository: PathBuf,
    root: &'a Path,
    jurisdiction: Jurisdiction,
    digest: Digest,
    claim: ClaimCapture,
}

impl<'a> CoreSource<'a> {
    /// Capture one immutable source for the core family.
    ///
    /// # Errors
    ///
    /// Returns the walk or read failure that prevented one complete tree
    /// identity. A partially digested tree is never returned.
    pub fn capture(
        repository: impl Into<PathBuf>,
        tree: &'a SelectedTree,
        contract: &Contract,
        claim: ClaimCapture,
    ) -> Result<Self, CoreInputError> {
        let jurisdiction = Jurisdiction::walk(tree.root(), contract.policy().ignore())
            .map_err(|error| CoreInputError::new(format!("core source jurisdiction: {error}")))?;
        let digest = jurisdiction
            .tree_digest()
            .map_err(|error| CoreInputError::new(format!("core source tree identity: {error}")))?;
        Ok(Self {
            repository: repository.into(),
            root: tree.root(),
            jurisdiction,
            digest,
            claim,
        })
    }

    /// Repository the two Git-declared arms observe.
    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    /// Immutable selected-tree root every compiled provider reads.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
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
    let mut arms = Vec::with_capacity(core_declarations().len());
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
            observe(declaration, source, contract)?,
        )?);
    }
    CoreInputs::new(arms)
}

fn observe(
    declaration: &CoreArmDeclaration,
    source: &CoreSource<'_>,
    contract: &Contract,
) -> Result<CoreObservation, CoreInputError> {
    let observation = match declaration.id() {
        GateId::MergeIntegrity => merge_integrity(source, contract.bounds()),
        GateId::PlanPhase => plan_phase(&source.claim),
        GateId::Staging => staging(source, contract.bounds()),
        GateId::RepoDocs => return repo_docs(source, contract),
        id @ (GateId::RepoShape
        | GateId::ConfigKeyValidity
        | GateId::ConfigFalseDisable
        | GateId::ConfigTrackedTrust) => uncompiled(id),
        id => unreachable!("{id} is not a compiled core declaration"),
    };
    Ok(observation)
}

fn repo_docs(
    source: &CoreSource<'_>,
    contract: &Contract,
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
        let bound = contract
            .rule_policy()
            .resolve_bound(
                BoundName::RepoTruthDocChars,
                &PolicyContext::new().with_path(path),
            )
            .map_err(|error| {
                CoreInputError::new(format!(
                    "resolve repository-document bound for {path:?}: {error}"
                ))
            })?;
        let measurement = if bound.unlimited() {
            DocMeasurement::unlimited(path, document.characters())?
        } else {
            let active_limit = varied_elastic_limit(
                bound.limit(),
                bound.ratchet_limit(),
                path,
                path,
                contract
                    .rule_policy()
                    .document()
                    .ratchet()
                    .variation_percent(),
            )
            .map_err(|error| {
                CoreInputError::new(format!(
                    "resolve active repository-document bound for {path:?}: {error}"
                ))
            })?;
            DocMeasurement::new(path, document.characters(), active_limit, false)?
        };
        measurements.push(measurement);
    }
    RepoDocsEvidence::new(measurements)
        .map(CoreEvidence::RepoDocs)
        .map(measured)
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

/// Arms whose compiled provider this build does not carry yet.
///
/// Each states the exact provider that is missing, so the report names the work
/// rather than reporting a measurement nobody made.
fn uncompiled(id: GateId) -> CoreObservation {
    let (reason, recovery) = match id {
        GateId::RepoShape => (
            "this build compiles no repository-shape provider",
            "compile the namespace, path-shape, and name-cluster scanners over the selected tree",
        ),
        GateId::ConfigKeyValidity => (
            "this build compiles no layered-configuration admission provider",
            "compile packaged and active-layer key admission over the resolved configuration",
        ),
        GateId::ConfigFalseDisable => (
            "this build compiles no false-disable registry inventory",
            "compile the declared false-disable paths and the consumers that resolve them",
        ),
        GateId::ConfigTrackedTrust => (
            "this build compiles no tracked-config trust inventory",
            "compile the declared executable repository config paths and their approval guards",
        ),
        id => unreachable!("{id} has a compiled core provider"),
    };
    CoreObservation::Unsupported {
        reason: reason.to_owned(),
        recovery: recovery.to_owned(),
    }
}

fn merge_integrity(source: &CoreSource<'_>, bounds: &Bounds) -> CoreObservation {
    let merge_head = match git(
        source,
        bounds,
        &["rev-parse", "--git-path", "MERGE_HEAD"],
        &[0],
    ) {
        Ok((_, output)) => output,
        Err(refusal) => return refusal.into_observation(),
    };
    if !source.repository.join(merge_head.trim()).exists() {
        return measured(CoreEvidence::MergeIntegrity(
            MergeIntegrityEvidence::NoMerge,
        ));
    }

    let staged_tree = match git_tree(source, bounds, &["write-tree"]) {
        Ok(tree) => tree,
        Err(refusal) => return refusal.into_observation(),
    };
    let head_tree = match git_tree(source, bounds, &["rev-parse", "HEAD^{tree}"]) {
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
    let (code, output) = match git(source, bounds, arguments, &[0, 1]) {
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

fn git_tree(
    source: &CoreSource<'_>,
    bounds: &Bounds,
    arguments: &[&str],
) -> Result<String, GitRefusal> {
    let (_, output) = git(source, bounds, arguments, &[0])?;
    let tree = output.trim().to_owned();
    validate_git_tree(&tree).map_err(|error| {
        GitRefusal::Unmeasured(format!(
            "git {} named no tree: {error}",
            arguments.join(" ")
        ))
    })?;
    Ok(tree)
}

fn staging(source: &CoreSource<'_>, bounds: &Bounds) -> CoreObservation {
    let arguments = &["diff", "--cached", "--name-only", "--diff-filter=ACMR"];
    let staged = match git(source, bounds, arguments, &[0]) {
        Ok((_, output)) => output,
        Err(refusal) => return refusal.into_observation(),
    };
    let unstaged = match git(source, bounds, &["diff", "--name-only"], &[0]) {
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
    source: &CoreSource<'_>,
    bounds: &Bounds,
    arguments: &[&str],
    accepted: &[i32],
) -> Result<(i32, String), GitRefusal> {
    let argv = Argv::new(GIT, arguments.iter().map(OsStr::new))
        .map_err(|error| GitRefusal::Unmeasured(format!("git argument vector: {error}")))?;
    let outcome = spawn(bounds.spec(argv, &source.repository))
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
