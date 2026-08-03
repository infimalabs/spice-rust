//! Acquisition of one complete toolchain-family observation battery.
//!
//! [`super::measure_toolchain`] validates, orders, and canonicalizes; this
//! module only observes. Every declaration produces exactly one
//! [`ArmObservation`], in compiled order, whether or not this build can analyze
//! it. Four declarations route to a compiled analysis in [`super::analyze`];
//! the rest cross as [`ArmOutcome::Unsupported`], which is neither a pass nor a
//! skip, so an unmeasured tree is never admitted by silence.
//!
//! The executable search path and the selected changes are both ambient
//! configuration. Each is captured at the binary rim and handed in whole rather
//! than read back from the process or the repository here, so two machines
//! cannot silently resolve two different programs or scan two different sets.
//!
//! One state the family vocabulary cannot express: an arm whose declared tools
//! include both an unresolvable program and a resolved one at the wrong
//! version. `ToolUnavailable` requires every identified program to be exact and
//! `UnsupportedVersion` requires every program to be identified, so acquisition
//! reports the missing program and lets [`super::measure_toolchain`] refuse the
//! incoherent pair loudly rather than inventing a third reading.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, spawn};
use wire::{Digest, Domain};

use super::analyze::{self, Analysis};
use super::{
    ArmObservation, ArmOutcome, TOOLCHAIN_ARMS, ToolEvidence, ToolRequirement, ToolchainDeclaration,
};
use crate::GateId;
use crate::input::Contract;
use crate::measure::Bounds;
use crate::tree::SelectedTree;

/// One captured tree, the search path to resolve in, and the changes selected.
///
/// The search path is optional because a rim can genuinely have none. That is
/// an observation, not a defect: every declared program then resolves to
/// [`ToolEvidence::Missing`] and the arms that need one say so.
///
/// The selected changes are captured at the rim alongside the tree rather than
/// recomputed here. An arm declaring [`super::TreeInput::SelectedChanges`] reads
/// this list and nothing else, so the paths it scans belong to the same instant
/// as the tree they are read from.
#[derive(Debug)]
pub struct ToolchainSource<'a> {
    root: &'a Path,
    search_path: Option<OsString>,
    changes: Vec<String>,
}

impl<'a> ToolchainSource<'a> {
    /// Capture one selected tree, the rim's search path, and its changed paths.
    ///
    /// Changed paths are selected-tree-relative. A path that does not name a
    /// regular file in the captured tree is retained here and dropped by the
    /// scanner that reads it, because a deletion is a change with nothing to
    /// read rather than a defect in the capture.
    #[must_use]
    pub fn capture(
        tree: &'a SelectedTree,
        search_path: Option<OsString>,
        changes: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            root: tree.root(),
            search_path,
            changes: changes.into_iter().collect(),
        }
    }

    /// Selected-tree root every probe runs against.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
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
fn refusal(declaration: ToolchainDeclaration, tools: &[ToolEvidence]) -> Option<Analysis> {
    let missing: Vec<&str> = tools
        .iter()
        .filter_map(|evidence| match evidence {
            ToolEvidence::Missing {
                requested_program, ..
            } => Some(requested_program.as_str()),
            ToolEvidence::Identified { .. } => None,
        })
        .collect();
    if !missing.is_empty() {
        return Some(Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!(
                "{} did not resolve its declared program: {}",
                declaration.id.as_str(),
                missing.join(", ")
            ),
        ));
    }

    let drifted: Vec<String> = declaration
        .tools
        .iter()
        .zip(tools)
        .filter_map(|(requirement, evidence)| match evidence {
            ToolEvidence::Identified { version, .. } if version != requirement.exact_version => {
                Some(format!(
                    "{} is {version:?} and not {:?}",
                    requirement.name, requirement.exact_version
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
/// Only reached once [`refusal`] has ruled out every unresolved program, so an
/// arm arriving here has one identified row per declared tool.
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
            ToolEvidence::Missing { .. } => None,
        })
        .collect()
}

/// The exact analyzer one identity is still owed, or `None` once nothing is.
///
/// Nothing is owed in two cases, and neither is a gap: the four analyses this
/// build compiles answer for themselves, and an identity that is not a
/// toolchain arm is not this family's to compile at all. Every identity is
/// named outright, so a new [`GateId`] fails to compile here rather than
/// receiving a sentence written for something else.
pub(super) const fn owed_analyzer(id: GateId) -> Option<&'static str> {
    match id {
        GateId::Formatters | GateId::LocalPaths | GateId::Taste | GateId::CanvasWebTypecheck => {
            None
        }
        GateId::JavascriptUnused => {
            Some("compile the JavaScript declaration and reference analysis over selected modules")
        }
        GateId::PythonTypecheck => {
            Some("compile the pyright error-level analysis over selected sources")
        }
        GateId::EnvPolicy => Some("compile the PACKAGED_ENV_PROFILE environment access scanner"),
        GateId::EnvNameLedger => Some("compile the PACKAGED_ENV_PROFILE environment name ledger"),
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
/// Every failure to reach a usable program is [`ToolEvidence::Missing`] rather
/// than an error, because a program this machine cannot run is an observation
/// the report is supposed to carry.
fn probe(
    requirement: &ToolRequirement,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
) -> ToolEvidence {
    let missing = || ToolEvidence::Missing {
        name: requirement.name.to_owned(),
        requested_program: requirement.program.to_owned(),
    };
    let Some(resolved) = resolve(requirement.program, source.search_path()) else {
        return missing();
    };
    let Ok(bytes) = fs::read(&resolved) else {
        return missing();
    };
    let Some(version) = version_output(&resolved, requirement.version_argv, source.root, bounds)
    else {
        return missing();
    };
    ToolEvidence::Identified {
        name: requirement.name.to_owned(),
        requested_program: requirement.program.to_owned(),
        resolved_program: resolved.to_string_lossy().into_owned(),
        version,
        digest: Digest::compute(Domain::GateTool, &bytes)
            .as_str()
            .to_owned(),
    }
}

/// Resolve one program name against the captured search path.
fn resolve(program: &str, search_path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(search_path?)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// Run the declared version probe and retain exactly what it printed.
///
/// The trailing line terminator is the only byte removed, because the exact
/// version contracts are written without one.
fn version_output(
    program: &Path,
    arguments: &[&str],
    root: &Path,
    bounds: &Bounds,
) -> Option<String> {
    let argv = Argv::new(program, arguments.iter().map(OsStr::new)).ok()?;
    let outcome = spawn(bounds.spec(argv, root)).ok()?.wait().ok()?;
    let Completion::Exited(status) = outcome.completion() else {
        return None;
    };
    if !status.success() {
        return None;
    }
    let stdout = outcome.stdout()?;
    if stdout.truncated() || !stdout.complete() {
        return None;
    }
    let printed = String::from_utf8_lossy(stdout.bytes()).into_owned();
    Some(printed.trim_end_matches(['\n', '\r']).to_owned())
}
