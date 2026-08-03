//! Acquisition of one complete toolchain-family observation battery.
//!
//! [`super::measure_toolchain`] validates, orders, and canonicalizes; this
//! module only observes. Every declaration produces exactly one
//! [`ArmObservation`], in compiled order, whether or not this build can analyze
//! it. An arm whose analysis is not compiled crosses as
//! [`ArmOutcome::Unsupported`], which is neither a pass nor a skip, so an
//! unmeasured tree is never admitted by silence.
//!
//! The executable search path is ambient configuration. It is captured at the
//! binary rim and handed in whole rather than read from the process environment
//! here, so two machines cannot silently resolve two different programs.
//!
//! One state the family vocabulary cannot express: an arm whose declared tools
//! include both an unresolvable program and a resolved one at the wrong
//! version. `ToolUnavailable` requires every identified program to be exact and
//! `UnsupportedVersion` requires every program to be identified, so acquisition
//! reports the missing program and lets [`super::measure_toolchain`] refuse the
//! incoherent pair loudly rather than inventing a third reading.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, spawn};
use wire::{Digest, Domain};

use super::{
    ArmObservation, ArmOutcome, TOOLCHAIN_ARMS, ToolEvidence, ToolRequirement, ToolchainDeclaration,
};
use crate::GateId;
use crate::input::Contract;
use crate::measure::Bounds;
use crate::tree::SelectedTree;

/// One captured tree and the executable search path acquisition may resolve in.
///
/// The search path is optional because a rim can genuinely have none. That is
/// an observation, not a defect: every declared program then resolves to
/// [`ToolEvidence::Missing`] and the arms that need one say so.
#[derive(Debug)]
pub struct ToolchainSource<'a> {
    root: &'a Path,
    search_path: Option<OsString>,
}

impl<'a> ToolchainSource<'a> {
    /// Capture one selected tree and the rim's executable search path.
    #[must_use]
    pub fn capture(tree: &'a SelectedTree, search_path: Option<OsString>) -> Self {
        Self {
            root: tree.root(),
            search_path,
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
    let (outcome, reason) = classify(declaration, &tools);
    ArmObservation {
        gate_id: declaration.id,
        outcome,
        reason,
        tools,
        findings: Vec::new(),
        exit_code: None,
    }
}

/// Decide which observed state the probed evidence puts this arm in.
///
/// Only states acquisition actually witnessed are reachable here. Neither
/// `NotApplicable` nor `Excluded` is ever produced, because both are skips that
/// admit the battery and this module has measured nothing that earns one.
fn classify(declaration: ToolchainDeclaration, tools: &[ToolEvidence]) -> (ArmOutcome, String) {
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
        return (
            ArmOutcome::ToolUnavailable,
            format!(
                "{} did not resolve its declared program: {}",
                declaration.id.as_str(),
                missing.join(", ")
            ),
        );
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
        return (
            ArmOutcome::UnsupportedVersion,
            format!(
                "{} left its exact version contract: {}",
                declaration.id.as_str(),
                drifted.join("; ")
            ),
        );
    }

    (
        ArmOutcome::Unsupported,
        format!(
            "{} has no compiled analyzer in this build; {}",
            declaration.id.as_str(),
            recovery(declaration.id)
        ),
    )
}

/// The exact analyzer each arm is still owed.
const fn recovery(id: GateId) -> &'static str {
    match id {
        GateId::Formatters => "compile the cargo fmt check analysis and its formatting findings",
        GateId::LocalPaths => "compile the LOCAL_PATH_MARKER scanner over selected changes",
        GateId::Taste => "compile the PACKAGED_TASTE_WORDS prose scanner over selected changes",
        GateId::CanvasWebTypecheck => {
            "compile the wasm32 check and trunk release build analyses under bounds"
        }
        GateId::JavascriptUnused => {
            "compile the JavaScript declaration and reference analysis over selected modules"
        }
        GateId::PythonTypecheck => "compile the pyright error-level analysis over selected sources",
        GateId::EnvPolicy => "compile the PACKAGED_ENV_PROFILE environment access scanner",
        GateId::EnvNameLedger => "compile the PACKAGED_ENV_PROFILE environment name ledger",
        _ => "compile the analyzer this declaration names",
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
