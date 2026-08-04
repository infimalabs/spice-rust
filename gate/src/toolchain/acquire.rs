//! Acquisition of one complete toolchain-family observation battery.
//!
//! [`super::measure_toolchain`] validates, orders, and canonicalizes; this
//! module only observes. Every declaration produces exactly one
//! [`ArmObservation`], in compiled order, whether or not this build can analyze
//! it. Every declaration naming a compiled analysis routes to
//! [`super::analyze`];
//! the rest cross as [`ArmOutcome::Unsupported`], which is neither a pass nor a
//! skip, so an unmeasured tree is never admitted by silence.
//!
//! The rim directory, the executable search path, and the selected changes are
//! all ambient configuration. Each is captured at the binary rim and handed in
//! whole rather than read back from the process or the repository here, so two
//! machines cannot silently resolve two different programs or scan two
//! different sets.
//!
//! A declared program is probed exactly once, at one absolute path, and the
//! probe answers with one of three observations: not found, found and not
//! identifiable, or identified at exactly what it printed. The middle one is
//! why the first two are separate: a tool that is present and broken is a
//! different fact about this machine than a tool that was never installed, and
//! collapsing them would send an operator looking for the wrong thing.
//!
//! One state the family vocabulary cannot express: an arm whose declared tools
//! include both an unusable program and a resolved one at the wrong version.
//! `ToolUnavailable` requires every identified program to be exact and
//! `UnsupportedVersion` requires every program to be identified, so acquisition
//! reports the unusable program and lets [`super::measure_toolchain`] refuse the
//! incoherent pair loudly rather than inventing a third reading.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, spawn};
use wire::{Digest, Domain};

use super::analyze::{self, Analysis};
use super::{
    ArmObservation, ArmOutcome, ProbeFailure, TOOLCHAIN_ARMS, ToolEvidence, ToolRequirement,
    ToolchainDeclaration, contracted_version,
};
use crate::GateId;
use crate::input::Contract;
use crate::measure::Bounds;
use crate::tree::SelectedTree;

type ProgramReader = fn(&Path) -> std::io::Result<Vec<u8>>;

fn read_program(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

/// Everything the binary rim observed about itself, captured as one value.
///
/// The three travel together because none of them is complete alone. A
/// search-path entry is allowed to be relative or empty, and such an entry names
/// a directory relative to wherever it was written down; nothing downstream can
/// recover that directory afterwards, because probes run in the selected tree
/// and this process runs wherever it was started. Handing the directory in
/// beside the search path is what makes a resolved program one absolute file
/// rather than one file per reader, and keeping them in one value is what stops
/// a caller from anchoring one rim's search path to another rim's directory.
///
/// The search path is optional because a rim can genuinely have none. That is an
/// observation, not a defect: every declared program then resolves to
/// [`ToolEvidence::Missing`] and the arms that need one say so.
#[derive(Clone, Debug)]
pub struct RimCapture {
    directory: PathBuf,
    search_path: Option<OsString>,
    changes: Vec<String>,
}

impl RimCapture {
    /// Capture one rim's directory, executable search path, and changed paths.
    ///
    /// Changed paths are selected-tree-relative. A path that does not name a
    /// regular file in the captured tree is retained here and dropped by the
    /// scanner that reads it, because a deletion is a change with nothing to
    /// read rather than a defect in the capture.
    #[must_use]
    pub fn new(
        directory: impl Into<PathBuf>,
        search_path: Option<OsString>,
        changes: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            directory: directory.into(),
            search_path,
            changes: changes.into_iter().collect(),
        }
    }

    /// Absolute directory the search path was captured in.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Search path captured at the rim, if the rim had one.
    #[must_use]
    pub fn search_path(&self) -> Option<&OsStr> {
        self.search_path.as_deref()
    }

    /// Selected-tree-relative paths the rim captured as changed.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        &self.changes
    }
}

/// One captured tree and the rim that named the programs and changes to use.
///
/// The rim capture is taken at the rim rather than recomputed here. An arm
/// declaring [`super::TreeInput::SelectedChanges`] reads its list and nothing
/// else, so the paths it scans belong to the same instant as the tree they are
/// read from, and the programs it probes belong to the same instant as the
/// directory they resolve against.
#[derive(Debug)]
pub struct ToolchainSource<'a> {
    root: &'a Path,
    rim: RimCapture,
}

impl<'a> ToolchainSource<'a> {
    /// Capture one selected tree beside the rim that will supply its programs.
    #[must_use]
    pub fn capture(tree: &'a SelectedTree, rim: RimCapture) -> Self {
        Self {
            root: tree.root(),
            rim,
        }
    }

    /// Selected-tree root every probe runs against.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
    }

    /// The rim this acquisition resolves and scans on behalf of.
    #[must_use]
    pub const fn rim(&self) -> &RimCapture {
        &self.rim
    }

    /// Selected-tree-relative paths the rim captured as changed.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        self.rim.changes()
    }
}

/// Acquire one observation for every declared toolchain arm.
///
/// The result is one [`ArmObservation`] per entry in [`TOOLCHAIN_ARMS`], in
/// declaration order, with no arm omitted and none invented. Feed it straight
/// to [`super::measure_toolchain`], which is the only admission authority.
#[must_use]
pub fn acquire_toolchain(source: &ToolchainSource<'_>, contract: &Contract) -> Vec<ArmObservation> {
    TOOLCHAIN_ARMS
        .into_iter()
        .map(|declaration| observe(declaration, source, contract.bounds()))
        .collect()
}

fn observe(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
) -> ArmObservation {
    let tools: Vec<ToolEvidence> = declaration
        .tools
        .iter()
        .map(|requirement| probe(requirement, source, bounds))
        .collect();
    let Analysis {
        outcome,
        reason,
        findings,
        exit_code,
    } = examine(declaration, source, bounds, &tools);
    ArmObservation {
        gate_id: declaration.id,
        outcome,
        reason,
        tools,
        findings,
        exit_code,
    }
}

/// Run the analysis this arm names, once its declared programs are usable.
///
/// Evidence is read before analysis because an arm whose program is absent or
/// at the wrong version has nothing to measure with; the refusal it earns says
/// so and no analysis is attempted. Every remaining identity is named outright:
/// a ninth declared arm brings a ninth [`GateId`], which makes
/// [`owed_analyzer`] fail to compile until it is answered for.
fn examine(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
    tools: &[ToolEvidence],
) -> Analysis {
    if let Some(refusal) = refusal(declaration, tools) {
        return refusal;
    }
    let Some(owed) = owed_analyzer(declaration.id) else {
        return match declaration.id {
            GateId::Formatters => analyze::formatters(
                declaration,
                source.root,
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::CanvasWebTypecheck => analyze::canvas_web_typecheck(
                declaration,
                source.root,
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::LocalPaths => analyze::local_paths(declaration, source.root, source.changes()),
            GateId::Taste => analyze::taste(declaration, source.root, source.changes()),
            GateId::JavascriptUnused => analyze::javascript_unused(declaration, source.root),
            GateId::CsharpUnusedCandidates => analyze::csharp_unused(declaration, source.root),
            GateId::EnvPolicy => analyze::env_policy(declaration, source.root, source.changes()),
            GateId::EnvNameLedger => analyze::env_name_ledger(declaration, source.root),
            id => Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!(
                    "{} is declared as a toolchain arm this family compiles no analysis for",
                    id.as_str()
                ),
            ),
        };
    };
    Analysis::terminal(
        ArmOutcome::Unsupported,
        format!(
            "{} has no compiled analyzer in this build; {owed}",
            declaration.id.as_str()
        ),
    )
}

/// The refusal this arm's probed evidence earns before any analysis runs.
///
/// `None` means every declared program resolved at its exact version. Neither
/// `NotApplicable` nor `Excluded` is ever produced here, because both are skips
/// that admit the battery and evidence alone has measured nothing to earn one.
///
/// An absent program and an unusable one earn the same refusal, because neither
/// leaves the arm anything to measure with, and the sentence says which of the
/// two happened to each program so the refusal is actionable rather than merely
/// correct.
fn refusal(declaration: ToolchainDeclaration, tools: &[ToolEvidence]) -> Option<Analysis> {
    let unusable: Vec<String> = tools
        .iter()
        .filter_map(|evidence| match evidence {
            ToolEvidence::Missing {
                requested_program, ..
            } => Some(format!("{requested_program} did not resolve")),
            ToolEvidence::Unprobed {
                requested_program,
                resolved_program,
                failure,
                ..
            } => Some(format!(
                "{requested_program} resolved to {resolved_program} and {failure}"
            )),
            ToolEvidence::Identified { .. } => None,
        })
        .collect();
    if !unusable.is_empty() {
        return Some(Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!(
                "{} has no usable declared program: {}",
                declaration.id.as_str(),
                unusable.join("; ")
            ),
        ));
    }

    let drifted: Vec<String> = declaration
        .tools
        .iter()
        .zip(tools)
        .filter_map(|(requirement, evidence)| match evidence {
            ToolEvidence::Identified { version, .. }
                if contracted_version(version) != requirement.exact_version =>
            {
                Some(format!(
                    "{} is {:?} and not {:?}",
                    requirement.name,
                    contracted_version(version),
                    requirement.exact_version
                ))
            }
            _ => None,
        })
        .collect();
    if !drifted.is_empty() {
        return Some(Analysis::terminal(
            ArmOutcome::UnsupportedVersion,
            format!(
                "{} left its exact version contract: {}",
                declaration.id.as_str(),
                drifted.join("; ")
            ),
        ));
    }

    None
}

/// Every declared program of one arm, keyed by the name that requested it.
///
/// Only reached once [`refusal`] has ruled out every unusable program, so an
/// arm arriving here has one identified row per declared tool. The path handed
/// on is the one the probe already ran, so an analysis cannot reach a second
/// file under the same declared name.
fn resolved_programs(
    declaration: ToolchainDeclaration,
    tools: &[ToolEvidence],
) -> BTreeMap<&str, &str> {
    declaration
        .tools
        .iter()
        .zip(tools)
        .filter_map(|(requirement, evidence)| match evidence {
            ToolEvidence::Identified {
                resolved_program, ..
            } => Some((requirement.program, resolved_program.as_str())),
            ToolEvidence::Missing { .. } | ToolEvidence::Unprobed { .. } => None,
        })
        .collect()
}

/// The exact analyzer one identity is still owed, or `None` once nothing is.
///
/// Nothing is owed in two cases, and neither is a gap: every compiled variant
/// named in the match below answers for itself, and an identity that is not a
/// toolchain arm is not this family's to compile at all. No sentence here
/// carries a count, because a count goes stale the moment an analysis lands.
/// Every identity is named outright, so a new [`GateId`] fails to compile here
/// rather than receiving a sentence written for something else.
pub(super) const fn owed_analyzer(id: GateId) -> Option<&'static str> {
    match id {
        GateId::Formatters
        | GateId::LocalPaths
        | GateId::Taste
        | GateId::CanvasWebTypecheck
        | GateId::JavascriptUnused
        | GateId::CsharpUnusedCandidates
        | GateId::EnvPolicy
        | GateId::EnvNameLedger => None,
        GateId::PythonTypecheck => {
            Some("compile the pyright error-level analysis over selected sources")
        }
        GateId::MergeIntegrity
        | GateId::PlanPhase
        | GateId::RepoShape
        | GateId::Staging
        | GateId::RepoDocs
        | GateId::ConfigKeyValidity
        | GateId::ConfigFalseDisable
        | GateId::ConfigTrackedTrust
        | GateId::FileShape
        | GateId::Complexity
        | GateId::MagicNumbers
        | GateId::MarkdownLinks
        | GateId::Reachability
        | GateId::SymbolReachability
        | GateId::PythonUnused
        | GateId::AssertionFreeTests
        | GateId::PrivateInternals => None,
    }
}

/// Probe one declared executable into evidence.
///
/// Nothing here is an error, because a program this machine cannot run is an
/// observation the report is supposed to carry. Only an unresolved program is
/// [`ToolEvidence::Missing`]; one that resolved and then failed keeps the exact
/// file it resolved to and says what the probe did, so an operator is never
/// sent looking for a program that is already installed.
///
/// The resolved path is computed once and used for the digest and the probe
/// both, so the bytes that were hashed are the bytes that answered.
fn probe(
    requirement: &ToolRequirement,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
) -> ToolEvidence {
    probe_with(requirement, source, bounds, read_program)
}

/// Probe one executable through the supplied byte reader.
///
/// The reader boundary keeps the acquisition contract testable under privileged
/// users without changing the production path, where [`fs::read`] remains the
/// sole source of executable bytes.
fn probe_with(
    requirement: &ToolRequirement,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
    read_program: ProgramReader,
) -> ToolEvidence {
    let rim = source.rim();
    let Some(resolved) = resolve(requirement.program, rim.directory(), rim.search_path()) else {
        return ToolEvidence::Missing {
            name: requirement.name.to_owned(),
            requested_program: requirement.program.to_owned(),
        };
    };
    let unprobed = |failure| ToolEvidence::Unprobed {
        name: requirement.name.to_owned(),
        requested_program: requirement.program.to_owned(),
        resolved_program: resolved.to_string_lossy().into_owned(),
        failure,
    };
    let Ok(bytes) = read_program(&resolved) else {
        return unprobed(ProbeFailure::Unreadable);
    };
    match version_output(&resolved, requirement.version_argv, source.root, bounds) {
        Ok(version) => ToolEvidence::Identified {
            name: requirement.name.to_owned(),
            requested_program: requirement.program.to_owned(),
            resolved_program: resolved.to_string_lossy().into_owned(),
            version,
            digest: Digest::compute(Domain::GateTool, &bytes)
                .as_str()
                .to_owned(),
        },
        Err(failure) => unprobed(failure),
    }
}

/// Resolve one program name against the captured search path.
///
/// Every entry is anchored to the captured rim directory before the program is
/// joined onto it, which leaves an absolute entry exactly as written and turns
/// a relative or empty one into a single absolute file. Without that, the same
/// entry would name one file to this process, which reads and hashes it from
/// wherever it was started, and another to the probe, which runs in the
/// selected tree: two different programs behind one recorded identity.
fn resolve(program: &str, rim: &Path, search_path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(search_path?)
        .map(|directory| rim.join(directory).join(program))
        .find(|candidate| candidate.is_file())
}

/// Run the declared version probe and retain exactly what it printed.
///
/// Nothing is removed. The printed bytes are the identity evidence, and the one
/// normalization the exact-version contracts imply belongs to the comparison
/// that reads them, not to the record that carries them. Output that is not
/// text identifies nothing, and neither does output with no version in it, so
/// both say so rather than being reported as an identity.
fn version_output(
    program: &Path,
    arguments: &[&str],
    root: &Path,
    bounds: &Bounds,
) -> Result<String, ProbeFailure> {
    let argv = Argv::new(program, arguments.iter().map(OsStr::new))
        .map_err(|_| ProbeFailure::NotStarted)?;
    let outcome = spawn(bounds.spec(argv, root))
        .map_err(|_| ProbeFailure::NotStarted)?
        .wait()
        .map_err(|_| ProbeFailure::NotStarted)?;
    match outcome.completion() {
        Completion::Exited(status) if status.success() => {}
        Completion::Exited(status) => {
            return Err(status.code().map_or(ProbeFailure::Signaled, |code| {
                ProbeFailure::Refused { code }
            }));
        }
        Completion::TimedOut(_) => return Err(ProbeFailure::TimedOut),
        Completion::Cancelled(_) => return Err(ProbeFailure::Signaled),
    }
    let stdout = outcome.stdout().ok_or(ProbeFailure::Incomplete)?;
    if stdout.truncated() || !stdout.complete() {
        return Err(ProbeFailure::Incomplete);
    }
    let printed =
        String::from_utf8(stdout.bytes().to_vec()).map_err(|_| ProbeFailure::Undecodable)?;
    if contracted_version(&printed).is_empty() {
        return Err(ProbeFailure::Incomplete);
    }
    Ok(printed)
}

#[cfg(test)]
include!("acquire/tests.rs");
