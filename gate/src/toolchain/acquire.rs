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
//! different sets. Reading nothing back is also why a relative rim directory is
//! refused by [`RimCapture::new`] instead of resolved there: resolving it would
//! read the one piece of process state this module declines to consult.
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
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::thread;

use sill::{Argv, Completion, spawn};
use wire::{Digest, Domain};

use super::analyze::{self, Analysis};
use super::{
    ArmObservation, ArmOutcome, ArmTiming, ProbeFailure, TOOLCHAIN_ARMS, ToolEvidence,
    ToolRequirement, ToolchainDeclaration, ToolchainError, TreeInput, VerifierCompilerEvidence,
    contracted_version,
};
use crate::GateId;
use crate::input::Contract;
use crate::measure::Bounds;
use crate::metrics::BaselineCapture;
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
    /// The directory must be absolute, and a relative one is refused here
    /// rather than resolved. This constructor takes everything it holds as an
    /// argument and reads nothing ambient, so the only directory it could
    /// anchor a relative one against is whatever directory this process happens
    /// to have been started in, which is the reader-dependent anchor the
    /// capture exists to remove. Refusing is what makes [`Self::directory`]
    /// absolute for every reader instead of only for the well-behaved caller.
    ///
    /// Changed paths are selected-tree-relative. A path that does not name a
    /// regular file in the captured tree is retained here and dropped by the
    /// scanner that reads it, because a deletion is a change with nothing to
    /// read rather than a defect in the capture.
    ///
    /// # Errors
    ///
    /// Refuses a relative directory, which cannot anchor a search path.
    pub fn new(
        directory: impl Into<PathBuf>,
        search_path: Option<OsString>,
        changes: impl IntoIterator<Item = String>,
    ) -> Result<Self, RimDirectoryError> {
        let directory = directory.into();
        if !directory.is_absolute() {
            return Err(RimDirectoryError(directory));
        }
        Ok(Self {
            directory,
            search_path,
            changes: changes.into_iter().collect(),
        })
    }

    /// Absolute directory the search path was captured in.
    ///
    /// Absolute by construction: [`Self::new`] admits nothing else.
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

    /// Replace the changed-path view with one validated selected-arm reach.
    pub(crate) fn with_changes(mut self, changes: Vec<String>) -> Self {
        self.changes = changes;
        self
    }
}

/// A directory that cannot anchor one rim's search path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RimDirectoryError(PathBuf);

impl fmt::Display for RimDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rim directory must be absolute to anchor a search path: {:?}",
            self.0
        )
    }
}

impl std::error::Error for RimDirectoryError {}

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
    repository: &'a Path,
    paths: &'a [String],
    rim: RimCapture,
    reached_paths: Option<Vec<String>>,
    baselines: Option<&'a BaselineCapture>,
    identity: Option<&'a Digest>,
    verifier_environment: &'a [(OsString, OsString)],
    verifier_evidence: &'a [VerifierCompilerEvidence],
    verifier_evidence_required: bool,
}

impl<'a> ToolchainSource<'a> {
    /// Capture one selected tree beside the rim that will supply its programs.
    ///
    /// Standalone toolchain acquisition does not infer ancestry from the live
    /// repository after the tree was selected. The production battery supplies
    /// ancestry from its atomic repository capture instead.
    #[must_use]
    pub fn capture(tree: &'a SelectedTree, rim: RimCapture) -> Self {
        Self {
            root: tree.root(),
            repository: tree.repository(),
            paths: tree.paths(),
            rim,
            reached_paths: None,
            baselines: None,
            identity: None,
            verifier_environment: &[],
            verifier_evidence: &[],
            verifier_evidence_required: false,
        }
    }

    /// Capture one selected tree with the parents that can prove merge authorship.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "source, ancestry, verifier, rim, and reach are independent captured authorities"
    )]
    pub(crate) fn capture_with_baselines(
        tree: &'a SelectedTree,
        rim: RimCapture,
        baselines: &'a BaselineCapture,
        identity: &'a Digest,
        verifier_environment: &'a [(OsString, OsString)],
        verifier_evidence: &'a [VerifierCompilerEvidence],
        verifier_evidence_required: bool,
        reached_paths: Option<&[String]>,
    ) -> Self {
        Self {
            root: tree.root(),
            repository: tree.repository(),
            paths: tree.paths(),
            rim,
            reached_paths: reached_paths.map(<[String]>::to_vec),
            baselines: Some(baselines),
            identity: Some(identity),
            verifier_environment,
            verifier_evidence,
            verifier_evidence_required,
        }
    }

    /// Selected-tree root every probe runs against.
    #[must_use]
    pub const fn root(&self) -> &Path {
        self.root
    }

    /// Repository that owns stable writable build outputs for this selection.
    #[must_use]
    pub const fn repository(&self) -> &Path {
        self.repository
    }

    /// The rim this acquisition resolves and scans on behalf of.
    #[must_use]
    pub const fn rim(&self) -> &RimCapture {
        &self.rim
    }

    /// Selected-tree identity reusable verifier evidence must name.
    pub(super) const fn identity(&self) -> Option<&Digest> {
        self.identity
    }

    /// Opaque successful verifier evidence offered to this battery run.
    pub(super) const fn verifier_evidence(&self) -> &[VerifierCompilerEvidence] {
        self.verifier_evidence
    }

    /// Ambient compiler environment captured by the consuming verifier.
    pub(super) const fn verifier_environment(&self) -> &[(OsString, OsString)] {
        self.verifier_environment
    }

    /// Whether compiler arms must refuse rather than execute on evidence drift.
    pub(super) const fn verifier_evidence_required(&self) -> bool {
        self.verifier_evidence_required
    }

    /// Selected-tree-relative paths the rim captured as changed.
    #[must_use]
    pub fn changes(&self) -> &[String] {
        self.rim.changes()
    }

    /// Validated paths for one focused arm, when narrowed.
    fn reached_paths(&self) -> Option<&[String]> {
        self.reached_paths.as_deref()
    }

    /// Paths one analyzer is authorized to consume in this capture.
    ///
    /// Complete batteries retain the complete selected tree. A focused arm
    /// receives dependency-expanded paths through the command rim.
    pub(super) fn analysis_paths(&self) -> &[String] {
        self.reached_paths.as_deref().unwrap_or(self.paths)
    }

    /// Whether this pending merge inherited one selected path verbatim.
    fn merge_inherits(&self, path: &str) -> bool {
        let Some(baselines) = self.baselines else {
            return false;
        };
        fs::read(self.root.join(path)).is_ok_and(|bytes| baselines.merge_inherits(path, &bytes))
    }

    /// Changed paths authored by this pending commit rather than a merge parent.
    fn chargeable_changes(&self) -> Vec<String> {
        self.changes()
            .iter()
            .filter(|path| !self.merge_inherits(path))
            .cloned()
            .collect()
    }
}

/// Acquire one observation for every declared toolchain arm.
///
/// The result is one [`ArmObservation`] per entry in [`TOOLCHAIN_ARMS`], in
/// declaration order, with no arm omitted and none invented. Feed it straight
/// to [`super::measure_toolchain`], which is the only admission authority.
#[must_use]
pub fn acquire_toolchain(source: &ToolchainSource<'_>, contract: &Contract) -> Vec<ArmObservation> {
    acquire_toolchain_observed(source, contract, |_, _| {})
}

/// Acquire every declared observation and report each completed analysis.
///
/// Completion is emitted only after the analyzer has returned its full
/// observation. The repository executor uses this seam to expose a real
/// process-order event without granting the observer execution authority.
pub(crate) fn acquire_toolchain_observed(
    source: &ToolchainSource<'_>,
    contract: &Contract,
    mut completed: impl FnMut(GateId, Option<super::RustAnalysisWorkspace>),
) -> Vec<ArmObservation> {
    let mut observations = Vec::with_capacity(TOOLCHAIN_ARMS.len());
    let mut declarations = TOOLCHAIN_ARMS.into_iter();
    while let Some(declaration) = declarations.next() {
        if declaration.id != GateId::RustTypecheck {
            let observation = observe_declaration(declaration, source, contract);
            completed(declaration.id, None);
            observations.push(observation);
            continue;
        }
        let canvas = declarations
            .next()
            .expect("canvas analysis follows native Rust analysis");
        assert_eq!(canvas.id, GateId::CanvasWebTypecheck);
        let (rust_observation, canvas_observation) = thread::scope(|scope| {
            let canvas = scope.spawn(|| observe_declaration(canvas, source, contract));
            let rust = observe_declaration(declaration, source, contract);
            let reusable_workspace = (rust.outcome == ArmOutcome::Passed).then(|| {
                analyze::rust_analysis_workspace(source.repository())
                    .expect("passed native Rust analysis must retain its exact reusable workspace")
            });
            completed(declaration.id, reusable_workspace);
            let canvas = canvas
                .join()
                .expect("canvas toolchain analysis must not panic");
            completed(canvas.gate_id, None);
            (rust, canvas)
        });
        observations.extend([rust_observation, canvas_observation]);
    }
    observations
}

fn observe_declaration(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    contract: &Contract,
) -> ArmObservation {
    let declaration = scoped_declaration(declaration, source);
    observe(
        declaration,
        source,
        contract.bounds(),
        contract.environment_names(),
    )
}

pub(crate) fn acquire_toolchain_arm(
    source: &ToolchainSource<'_>,
    contract: &Contract,
    gate_id: GateId,
) -> Result<ArmObservation, ToolchainError> {
    let declaration = TOOLCHAIN_ARMS
        .into_iter()
        .find(|declaration| declaration.id == gate_id)
        .ok_or(ToolchainError::ForeignGate(gate_id))?;
    let declaration = scoped_declaration(declaration, source);
    Ok(observe(
        declaration,
        source,
        contract.bounds(),
        contract.environment_names(),
    ))
}

fn scoped_declaration(
    mut declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
) -> ToolchainDeclaration {
    if source.reached_paths().is_some()
        && matches!(
            declaration.id,
            GateId::Formatters
                | GateId::JavascriptUnused
                | GateId::CsharpUnusedCandidates
                | GateId::PythonTypecheck
                | GateId::EnvNameLedger
        )
    {
        declaration.jurisdiction.input = TreeInput::SelectedChanges;
    }
    declaration
}

/// Probe and validate the exact declared tools a reusable verifier contract
/// must bind, without running the arm's analysis.
///
/// The inner result distinguishes an inapplicable arm from acquired tools;
/// the outer result retains actual acquisition failures.
pub(crate) fn acquire_verifier_tools(
    source: &ToolchainSource<'_>,
    contract: &Contract,
    gate_id: GateId,
) -> Result<Result<Vec<ToolEvidence>, String>, String> {
    let declaration = TOOLCHAIN_ARMS
        .into_iter()
        .find(|declaration| declaration.id == gate_id)
        .ok_or_else(|| format!("{gate_id} is not a toolchain arm"))?;
    if !matches!(gate_id, GateId::RustTypecheck | GateId::CanvasWebTypecheck) {
        return Err(format!(
            "{} has no reusable verifier compiler contract",
            gate_id.as_str()
        ));
    }
    if let Some(analysis) = analyze::before_tools(
        declaration,
        source.root(),
        source.reached_paths(),
        source.analysis_paths(),
    ) {
        return if analysis.outcome == ArmOutcome::NotApplicable {
            Ok(Err(analysis.reason))
        } else {
            Err(analysis.reason)
        };
    }
    let tools = declaration
        .tools
        .iter()
        .map(|requirement| probe(requirement, source, contract.bounds()))
        .collect::<Vec<_>>();
    if let Some(analysis) = refusal(declaration, &tools) {
        return Err(analysis.reason);
    }
    Ok(Ok(tools))
}

fn observe(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
    environment_names: &[String],
) -> ArmObservation {
    if let Some(analysis) = analyze::before_tools(
        declaration,
        source.root(),
        source.reached_paths(),
        source.analysis_paths(),
    ) {
        return observation(declaration, analysis, Vec::new());
    }
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
        timing,
    } = examine(declaration, source, bounds, environment_names, &tools);
    let tools = if matches!(outcome, ArmOutcome::NotApplicable | ArmOutcome::Excluded) {
        Vec::new()
    } else {
        tools
    };
    observation(
        declaration,
        Analysis {
            outcome,
            reason,
            findings,
            exit_code,
            timing,
        },
        tools,
    )
}

fn observation(
    declaration: ToolchainDeclaration,
    analysis: Analysis,
    tools: Vec<ToolEvidence>,
) -> ArmObservation {
    ArmObservation {
        gate_id: declaration.id,
        tree_input: declaration.jurisdiction.input,
        outcome: analysis.outcome,
        reason: analysis.reason,
        tools,
        findings: analysis.findings,
        exit_code: analysis.exit_code,
        timing: analysis.timing,
    }
}

/// Run the analysis this arm names, once its declared programs are usable.
///
/// Evidence is read before analysis because an arm whose program is absent or
/// at the wrong version has nothing to measure with; the refusal it earns says
/// so and no analysis is attempted. Every identity is named outright, so a new
/// [`GateId`] fails to compile here until its one current analysis is supplied.
fn examine(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
    environment_names: &[String],
    tools: &[ToolEvidence],
) -> Analysis {
    if let Some(refusal) = analyzer_refusal(declaration, tools) {
        return refusal;
    }
    if let Some(reused) = reusable_verifier_analysis(declaration, source, bounds, tools) {
        return reused;
    }
    if source.verifier_evidence_required()
        && matches!(
            declaration.id,
            GateId::RustTypecheck | GateId::CanvasWebTypecheck
        )
    {
        return Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!(
                "{} required admitted verifier evidence, but current tool evidence no longer matches",
                declaration.id.as_str()
            ),
        );
    }
    let Some(owed) = owed_analyzer(declaration.id) else {
        return match declaration.id {
            GateId::Formatters => analyze::formatters(
                declaration,
                source.root,
                source.reached_paths(),
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::RustTypecheck => analyze::rust_typecheck(
                declaration,
                source.root,
                source.repository,
                source.changes(),
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::CanvasWebTypecheck => analyze::canvas_web_typecheck(
                declaration,
                source.root,
                source.repository,
                source.changes(),
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::LocalPaths => {
                let changes = source.chargeable_changes();
                analyze::local_paths(declaration, source.root, &changes)
            }
            GateId::Taste => {
                let changes = source.chargeable_changes();
                analyze::taste(declaration, source.root, &changes)
            }
            GateId::JavascriptUnused => {
                analyze::javascript_unused(declaration, source.root, source.analysis_paths())
            }
            GateId::CsharpUnusedCandidates => {
                analyze::csharp_unused(declaration, source.root, source.analysis_paths())
            }
            GateId::PythonTypecheck => analyze::python_typecheck(
                declaration,
                source.root,
                source.analysis_paths(),
                bounds,
                &resolved_programs(declaration, tools),
            ),
            GateId::EnvPolicy => {
                let changes = source.chargeable_changes();
                analyze::env_policy(declaration, source.root, &changes)
            }
            GateId::EnvNameLedger => analyze::env_name_ledger(
                declaration,
                source.root,
                source.analysis_paths(),
                environment_names,
                |path| !source.merge_inherits(path),
            ),
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

/// Admit one opaque full-verifier proof or leave the ordinary arm untouched.
///
/// Rejection is deliberately a fall-through, not a refusal: stale, forged,
/// partial, or failed evidence has no authority, so the standalone analysis
/// executes exactly as it did before evidence was offered.
fn reusable_verifier_analysis(
    declaration: ToolchainDeclaration,
    source: &ToolchainSource<'_>,
    bounds: &Bounds,
    tools: &[ToolEvidence],
) -> Option<Analysis> {
    let identity = source.identity()?;
    let evidence = source.verifier_evidence().iter().find(|evidence| {
        evidence.admits(
            declaration.id,
            identity,
            source.repository(),
            source.root(),
            source.verifier_environment(),
            tools,
        )
    })?;
    let analysis = Analysis::terminal(
        ArmOutcome::Passed,
        format!(
            "{} reused exact successful full-verifier compiler evidence",
            declaration.id.as_str()
        ),
    );
    if declaration.id != GateId::RustTypecheck {
        return Some(analysis);
    }
    let deadlines = bounds.deadlines();
    let observation = deadlines
        .runtime()
        .saturating_add(deadlines.output_drain())
        .saturating_add(deadlines.reap())
        .saturating_add(bounds.grace());
    Some(analysis.timed(ArmTiming {
        elapsed_ms: evidence.elapsed_ms(),
        observation_bound_ms: u64::try_from(observation.as_millis()).unwrap_or(u64::MAX),
    }))
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
    refusal_for(declaration.id, declaration.tools, tools)
}

/// Canvas may decide from exact Cargo metadata that Trunk is outside the
/// selected closure. Every other analyzer requires all declared programs
/// before it can establish applicability.
fn analyzer_refusal(declaration: ToolchainDeclaration, tools: &[ToolEvidence]) -> Option<Analysis> {
    if declaration.id != GateId::CanvasWebTypecheck {
        return refusal(declaration, tools);
    }
    let cargo = declaration
        .tools
        .iter()
        .position(|requirement| requirement.program == "cargo")
        .expect("canvas declaration retains its Cargo metadata authority");
    refusal_for(
        declaration.id,
        std::slice::from_ref(&declaration.tools[cargo]),
        std::slice::from_ref(&tools[cargo]),
    )
}

fn refusal_for(
    gate_id: GateId,
    declared: &[ToolRequirement],
    tools: &[ToolEvidence],
) -> Option<Analysis> {
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
                gate_id.as_str(),
                unusable.join("; ")
            ),
        ));
    }

    let drifted: Vec<String> = declared
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
                gate_id.as_str(),
                drifted.join("; ")
            ),
        ));
    }

    None
}

/// Every declared program of one arm, keyed by the name that requested it.
///
/// Normally reached once [`refusal`] has ruled out every unusable program.
/// Canvas may arrive with only exact Cargo so metadata can decide whether Trunk
/// belongs to the selected closure. Every retained path is the one its probe
/// already ran, so an analysis cannot reach a second file under the same name.
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
/// Every compiled variant is named explicitly so adding an arm or completing
/// an analyzer changes this exhaustive contract at compile time.
pub(super) const fn owed_analyzer(id: GateId) -> Option<&'static str> {
    match id {
        GateId::Formatters
        | GateId::LocalPaths
        | GateId::Taste
        | GateId::RustTypecheck
        | GateId::CanvasWebTypecheck
        | GateId::JavascriptUnused
        | GateId::CsharpUnusedCandidates
        | GateId::PythonTypecheck
        | GateId::EnvPolicy
        | GateId::EnvNameLedger => None,
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
        | GateId::PrivateInternals
        | GateId::SuiteSeam
        | GateId::MutationCoverage
        | GateId::TestSubsumption => None,
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
    let launchable =
        fs::metadata(&resolved).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0);
    if !launchable {
        return unprobed(ProbeFailure::NotStarted);
    }
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
mod tests;
