//! Opaque compiler evidence passed from the full verifier into one battery run.
//!
//! The verifier already compiles the native workspace and the browser target
//! before it reaches the canonical battery. These values let that caller retain
//! successful process evidence in memory or encode only its source-bound
//! identities for one cross-process receipt. Admission reconstructs authority
//! against a fresh contract; raw environment values never cross the boundary.
//! A process token can only be minted by executing a [`VerifierCompilerContract`],
//! and a complete arm token can only be assembled from the exact successful
//! stages that arm requires.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sill::{
    Acquisition, Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Output,
    Possession, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use wire::{Digest, Domain};

use super::ToolEvidence;
use super::analyze::{
    CANVAS_SOURCE_LABEL, CANVAS_SOURCE_LOCK, canvas_source_root, make_cache_writable,
    rust_analysis_workspace, synchronize_source,
};
use crate::VERIFIER_APP_PUBLISH_FLAG;
use crate::battery::CANONICAL_BATTERY_MANIFEST;
use crate::manifest::GateId;

#[path = "verifier_receipt.rs"]
mod receipt;

pub use receipt::{admit_verifier_compiler_receipt, encode_verifier_compiler_receipt};

#[cfg(test)]
use receipt::VerifierCompilerReceiptSet;

const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";
const CARGO_BUILD_BUILD_DIR: &str = "CARGO_BUILD_BUILD_DIR";
const PROCESS_RUNTIME: Duration = Duration::from_secs(30 * 60);
const OUTPUT_DRAIN: Duration = Duration::from_secs(30);
const REAP_DEADLINE: Duration = Duration::from_secs(10);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

const NATIVE_CLIPPY_ARGUMENTS: &[&str] = &[
    "clippy",
    "--locked",
    "--workspace",
    "--all-targets",
    "--",
    "-D",
    "warnings",
];
const CANVAS_CHECK_ARGUMENTS: &[&str] = &[
    "check",
    "--locked",
    "--workspace",
    "--target",
    "wasm32-unknown-unknown",
];
const CANVAS_APP_ARGUMENTS: &[&str] = &["xtask", "app"];

const NATIVE_TARGETS: &[&str] = &["host", "workspace", "all-targets"];
const CANVAS_CHECK_TARGETS: &[&str] = &["wasm32-unknown-unknown", "workspace", "default-targets"];
const CANVAS_APP_TARGETS: &[&str] = &["wasm32-unknown-unknown", "app", "release"];

/// One full-verifier compiler process which can contribute to reusable proof.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifierCompilerStage {
    /// Clippy's native workspace/all-targets compiler analysis.
    NativeClippy,
    /// The exact workspace Wasm check also declared by the canvas arm.
    CanvasCheck,
    /// The repository-owned app build which runs the declared Trunk analysis.
    CanvasApp,
}

impl VerifierCompilerStage {
    const fn gate_id(self) -> GateId {
        match self {
            Self::NativeClippy => GateId::RustTypecheck,
            Self::CanvasCheck | Self::CanvasApp => GateId::CanvasWebTypecheck,
        }
    }

    const fn base_arguments(self) -> &'static [&'static str] {
        match self {
            Self::NativeClippy => NATIVE_CLIPPY_ARGUMENTS,
            Self::CanvasCheck => CANVAS_CHECK_ARGUMENTS,
            Self::CanvasApp => CANVAS_APP_ARGUMENTS,
        }
    }

    const fn targets(self) -> &'static [&'static str] {
        match self {
            Self::NativeClippy => NATIVE_TARGETS,
            Self::CanvasCheck => CANVAS_CHECK_TARGETS,
            Self::CanvasApp => CANVAS_APP_TARGETS,
        }
    }

    const fn tag(self) -> &'static str {
        match self {
            Self::NativeClippy => "native-clippy",
            Self::CanvasCheck => "canvas-check",
            Self::CanvasApp => "canvas-app",
        }
    }
}

/// The result of launching one verifier compiler stage.
pub struct VerifierCompilerRun {
    success: bool,
    code: Option<i32>,
    evidence: Option<VerifierCompilerProcessEvidence>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    completion: VerifierCompilerCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VerifierCompilerCompletion {
    Exited,
    Cancelled,
    TimedOut,
}

impl VerifierCompilerRun {
    /// Whether the compiler process exited successfully.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.success
    }

    /// Exact process exit code, absent when the process ended on a signal.
    #[must_use]
    pub const fn code(&self) -> Option<i32> {
        self.code
    }

    /// Complete bounded standard output from the compiler process.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Complete bounded standard error from the compiler process.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Whether cancellation terminated the compiler process group.
    #[must_use]
    pub const fn cancelled(&self) -> bool {
        matches!(self.completion, VerifierCompilerCompletion::Cancelled)
    }

    /// Whether the compiler process group exceeded its runtime deadline.
    #[must_use]
    pub const fn timed_out(&self) -> bool {
        matches!(self.completion, VerifierCompilerCompletion::TimedOut)
    }

    /// Successful reusable evidence, absent after process failure.
    #[must_use]
    pub fn into_evidence(self) -> Option<VerifierCompilerProcessEvidence> {
        self.evidence
    }
}

/// Capture-bound authority for the full verifier's compiler processes.
///
/// Fields are private and there is no deserializer or public constructor.  A
/// caller can execute this contract, but cannot manufacture the process token
/// returned by a successful execution.
#[derive(Clone, Debug)]
pub struct VerifierCompilerContract {
    gate_id: GateId,
    source: Digest,
    manifest: Digest,
    repository: PathBuf,
    selection_root: PathBuf,
    tools: Vec<ToolEvidence>,
    environment: Vec<(OsString, OsString)>,
}

impl VerifierCompilerContract {
    pub(crate) fn new(
        gate_id: GateId,
        source: Digest,
        repository: PathBuf,
        selection_root: PathBuf,
        tools: Vec<ToolEvidence>,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, String> {
        if !matches!(gate_id, GateId::RustTypecheck | GateId::CanvasWebTypecheck) {
            return Err(format!(
                "{} has no reusable full-verifier compiler contract",
                gate_id.as_str()
            ));
        }
        Ok(Self {
            gate_id,
            source,
            manifest: manifest_identity(),
            repository,
            selection_root,
            tools,
            environment,
        })
    }

    /// Gate arm this contract can prove.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.gate_id
    }

    /// Exact compiler stages required to prove this arm.
    #[must_use]
    pub fn stages(&self) -> &'static [VerifierCompilerStage] {
        expected_stages(self.gate_id)
    }

    /// Mirror this contract's immutable selection into its stable compiler root.
    ///
    /// The returned preparation holds the source-cache possession until every
    /// stage has completed. Unchanged bytes retain their mtimes, so Cargo can
    /// reuse exact fingerprints without a compiler wrapper or a live-worktree
    /// input. The contract itself remains reusable by callers whose compiler
    /// scheduling already owns a different source-root policy.
    ///
    /// # Errors
    ///
    /// Returns source-root resolution, possession, synchronization, or
    /// permission failures.
    pub fn prepare_stable(self) -> io::Result<PreparedVerifierCompiler> {
        let source = StableCompilerSource::capture(&self)?;
        let mut contract = self;
        contract.selection_root = source.root().to_path_buf();
        Ok(PreparedVerifierCompiler { contract, source })
    }

    /// Run one exact compiler stage and mint evidence only after a clean exit.
    ///
    /// The process runs from the immutable selected source root. Compiler
    /// outputs are redirected into the live repository, and the app stage is
    /// given an explicit separate publication root, so neither output nor
    /// ignored live-worktree inputs can change what was analyzed.
    ///
    /// # Errors
    ///
    /// Returns an unsupported stage, invalid supervision policy, launch, wait,
    /// drain, or capture failure.
    pub fn run(&self, stage: VerifierCompilerStage) -> io::Result<VerifierCompilerRun> {
        self.run_with_cancellation(stage, Cancellation::Never)
    }

    /// Run one compiler stage under an externally cancellable process group.
    ///
    /// # Errors
    ///
    /// Returns invalid supervision policy, launch, wait, drain, or capture
    /// failures. Cancellation and timeout are returned as typed run outcomes.
    pub fn run_supervised(
        &self,
        stage: VerifierCompilerStage,
        cancellation: CancellationToken,
    ) -> io::Result<VerifierCompilerRun> {
        self.run_with_cancellation(stage, Cancellation::Token(cancellation))
    }

    fn run_with_cancellation(
        &self,
        stage: VerifierCompilerStage,
        cancellation: Cancellation,
    ) -> io::Result<VerifierCompilerRun> {
        if stage.gate_id() != self.gate_id {
            return Err(io::Error::other(format!(
                "{} cannot execute {} evidence",
                self.gate_id.as_str(),
                stage.tag()
            )));
        }
        let Some(program) = cargo_program(&self.tools) else {
            return Err(io::Error::other(format!(
                "{} evidence has no identified Cargo executable",
                self.gate_id.as_str()
            )));
        };
        let arguments = stage_arguments(stage, &self.repository);
        let environment = compiler_environment(&self.environment, &self.repository);
        let started = Instant::now();
        let process_environment =
            Environment::new(environment.clone()).map_err(io::Error::other)?;
        let deadlines = Deadlines::new(PROCESS_RUNTIME, OUTPUT_DRAIN, REAP_DEADLINE)
            .map_err(io::Error::other)?;
        let wait = WaitPolicy::new(
            format!("verifier compiler stage {}", stage.tag()),
            cancellation,
            RestartDisposition::Never,
        )
        .map_err(io::Error::other)?;
        let spec = Spec {
            argv: Argv::new(program, arguments.iter().cloned()).map_err(io::Error::other)?,
            cwd: self.selection_root.clone(),
            environment: process_environment,
            group: crate::measure::bounded_process_group(),
            deadlines,
            wait,
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture {
                    limit: OUTPUT_LIMIT,
                },
                StreamPolicy::Capture {
                    limit: OUTPUT_LIMIT,
                },
            ),
            redaction: Redaction::None,
            grace: TERMINATION_GRACE,
        };
        let outcome = sill::spawn(spec)
            .map_err(io::Error::other)?
            .wait()
            .map_err(io::Error::other)?;
        if !outcome.outputs_drained() {
            return Err(io::Error::other(format!(
                "{} compiler output did not drain before cleanup",
                stage.tag()
            )));
        }
        let stdout = complete_output(stage, "stdout", outcome.stdout())?;
        let stderr = complete_output(stage, "stderr", outcome.stderr())?;
        let status = outcome.completion().status();
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let success = status.success();
        let code = status.code();
        let completion = match outcome.completion() {
            Completion::Exited(_) => VerifierCompilerCompletion::Exited,
            Completion::Cancelled(_) => VerifierCompilerCompletion::Cancelled,
            Completion::TimedOut(_) => VerifierCompilerCompletion::TimedOut,
        };
        let evidence = (completion == VerifierCompilerCompletion::Exited && success).then(|| {
            VerifierCompilerProcessEvidence::mint(
                self,
                stage,
                environment,
                arguments,
                elapsed_ms,
                code,
            )
        });
        Ok(VerifierCompilerRun {
            success,
            code,
            evidence,
            stdout,
            stderr,
            completion,
        })
    }

    /// Assemble a complete reusable arm proof from successful stage tokens.
    ///
    /// # Errors
    ///
    /// Refuses a missing, duplicate, foreign, stale, or mutated stage.
    pub fn complete(
        &self,
        processes: &[VerifierCompilerProcessEvidence],
    ) -> Result<VerifierCompilerEvidence, String> {
        let expected = expected_stages(self.gate_id);
        let selected = processes
            .iter()
            .filter(|process| process.gate_id == self.gate_id)
            .cloned()
            .collect::<Vec<_>>();
        if selected.len() != expected.len() {
            return Err(format!(
                "{} compiler evidence has {} of {} required stages",
                self.gate_id.as_str(),
                selected.len(),
                expected.len()
            ));
        }
        for stage in expected {
            let matching = selected
                .iter()
                .filter(|process| process.stage == *stage)
                .collect::<Vec<_>>();
            if matching.len() != 1 || !matching[0].valid_for(self) {
                return Err(format!(
                    "{} compiler evidence has no one exact {} stage",
                    self.gate_id.as_str(),
                    stage.tag()
                ));
            }
        }
        Ok(VerifierCompilerEvidence::mint(self, selected))
    }
}

/// One compiler contract bound to a verified stable mirror of its selection.
pub struct PreparedVerifierCompiler {
    contract: VerifierCompilerContract,
    source: StableCompilerSource,
}

impl PreparedVerifierCompiler {
    /// Exact stages this prepared arm must complete.
    #[must_use]
    pub fn stages(&self) -> &'static [VerifierCompilerStage] {
        self.contract.stages()
    }

    /// Run one exact stage from the held stable source mirror.
    ///
    /// # Errors
    ///
    /// Returns the same supervision failures as [`VerifierCompilerContract::run`].
    pub fn run(&self, stage: VerifierCompilerStage) -> io::Result<VerifierCompilerRun> {
        self.contract.run(stage)
    }

    /// Complete the arm proof and release its stable source possession.
    ///
    /// # Errors
    ///
    /// Refuses incomplete evidence or a source mirror that cannot be returned
    /// to its writable persistent state.
    pub fn complete(
        mut self,
        processes: &[VerifierCompilerProcessEvidence],
    ) -> Result<VerifierCompilerEvidence, String> {
        let evidence = self.contract.complete(processes)?;
        self.source.release().map_err(|error| {
            format!(
                "{} {} could not be released: {error}",
                self.source.label,
                self.source.root.display(),
            )
        })?;
        Ok(evidence)
    }
}

struct StableCompilerSource {
    root: PathBuf,
    label: &'static str,
    possession: Option<Possession>,
}

impl StableCompilerSource {
    fn capture(contract: &VerifierCompilerContract) -> io::Result<Self> {
        let (root, lock, label) = stable_source_contract(contract)?;
        let parent = root.parent().ok_or_else(|| {
            io::Error::other(format!(
                "{label} {} has no parent directory",
                root.display()
            ))
        })?;
        fs::create_dir_all(parent)?;
        let source_lock =
            crate::held_root::held_root_lock(&root, lock, label).map_err(io::Error::other)?;
        let possession = match source_lock.try_acquire().map_err(io::Error::other)? {
            Acquisition::Acquired(possession) => possession,
            Acquisition::Contended => {
                return Err(io::Error::other(format!(
                    "{label} {} is in use by another canonical gate",
                    root.display(),
                )));
            }
        };
        synchronize_source(&contract.selection_root, &root)?;
        Ok(Self {
            root,
            label,
            possession: Some(possession),
        })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn release(&mut self) -> io::Result<()> {
        make_cache_writable(&self.root)?;
        drop(self.possession.take());
        Ok(())
    }
}

impl Drop for StableCompilerSource {
    fn drop(&mut self) {
        if self.possession.is_some() {
            let _ = make_cache_writable(&self.root);
            drop(self.possession.take());
        }
    }
}

fn stable_source_contract(
    contract: &VerifierCompilerContract,
) -> io::Result<(PathBuf, &'static str, &'static str)> {
    match contract.gate_id {
        GateId::RustTypecheck => rust_analysis_workspace(&contract.repository)
            .map(|workspace| {
                (
                    workspace.source().to_path_buf(),
                    "rust-analysis-source-cache",
                    "rust analysis source cache",
                )
            })
            .map_err(io::Error::other),
        GateId::CanvasWebTypecheck => Ok((
            canvas_source_root(&contract.repository),
            CANVAS_SOURCE_LOCK,
            CANVAS_SOURCE_LABEL,
        )),
        gate_id => Err(io::Error::other(format!(
            "{} has no stable verifier compiler source",
            gate_id.as_str(),
        ))),
    }
}

/// One successful process token. Only [`VerifierCompilerContract::run`] or
/// [`VerifierCompilerContract::run_supervised`] can mint it; callers may retain
/// and return it but cannot alter its fields.
#[derive(Clone, Debug)]
pub struct VerifierCompilerProcessEvidence {
    gate_id: GateId,
    stage: VerifierCompilerStage,
    source: Digest,
    manifest: Digest,
    repository: PathBuf,
    selection_root: PathBuf,
    tools: Vec<ToolEvidence>,
    environment: Vec<(OsString, OsString)>,
    targets: Vec<String>,
    arguments: Vec<OsString>,
    exit_code: i32,
    elapsed_ms: u64,
    seal: Digest,
}

impl VerifierCompilerProcessEvidence {
    fn mint(
        contract: &VerifierCompilerContract,
        stage: VerifierCompilerStage,
        environment: Vec<(OsString, OsString)>,
        arguments: Vec<OsString>,
        elapsed_ms: u64,
        code: Option<i32>,
    ) -> Self {
        let mut evidence = Self {
            gate_id: contract.gate_id,
            stage,
            source: contract.source.clone(),
            manifest: contract.manifest.clone(),
            repository: contract.repository.clone(),
            selection_root: contract.selection_root.clone(),
            tools: contract.tools.clone(),
            environment,
            targets: stage
                .targets()
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            arguments,
            exit_code: code.unwrap_or(0),
            elapsed_ms,
            seal: Digest::compute(Domain::GateReceipt, b"unsealed verifier compiler process"),
        };
        evidence.seal = evidence.compute_seal();
        evidence
    }

    fn valid_for(&self, contract: &VerifierCompilerContract) -> bool {
        self.gate_id == contract.gate_id
            && self.stage.gate_id() == contract.gate_id
            && self.source == contract.source
            && self.manifest == contract.manifest
            && self.repository == contract.repository
            && self.selection_root == contract.selection_root
            && self.tools == contract.tools
            && self.environment == compiler_environment(&contract.environment, &contract.repository)
            && self.targets == self.stage.targets()
            && self.arguments == stage_arguments(self.stage, &contract.repository)
            && self.exit_code == 0
            && self.seal == self.compute_seal()
    }

    fn compute_seal(&self) -> Digest {
        let mut bytes = Vec::new();
        frame(&mut bytes, self.gate_id.as_str().as_bytes());
        frame(&mut bytes, self.stage.tag().as_bytes());
        frame(&mut bytes, self.source.as_str().as_bytes());
        frame(&mut bytes, self.manifest.as_str().as_bytes());
        frame(&mut bytes, self.repository.as_os_str().as_bytes());
        frame(&mut bytes, self.selection_root.as_os_str().as_bytes());
        for tool in &self.tools {
            let encoded = wire::encode(tool).expect("tool evidence is encodable");
            frame(&mut bytes, &encoded);
        }
        for (name, value) in &self.environment {
            frame(&mut bytes, name.as_bytes());
            frame(&mut bytes, value.as_bytes());
        }
        for target in &self.targets {
            frame(&mut bytes, target.as_bytes());
        }
        for argument in &self.arguments {
            frame(&mut bytes, argument.as_os_str().as_bytes());
        }
        frame(&mut bytes, &self.exit_code.to_be_bytes());
        frame(&mut bytes, &self.elapsed_ms.to_be_bytes());
        Digest::compute(Domain::GateReceipt, &bytes)
    }
}

/// Complete proof that the full verifier already performed one arm's compiler
/// analysis for this exact capture.
#[derive(Clone, Debug)]
pub struct VerifierCompilerEvidence {
    gate_id: GateId,
    source: Digest,
    manifest: Digest,
    repository: PathBuf,
    selection_root: PathBuf,
    tools: Vec<ToolEvidence>,
    environment: Vec<(OsString, OsString)>,
    processes: Vec<VerifierCompilerProcessEvidence>,
    seal: Digest,
}

impl VerifierCompilerEvidence {
    fn mint(
        contract: &VerifierCompilerContract,
        processes: Vec<VerifierCompilerProcessEvidence>,
    ) -> Self {
        let mut evidence = Self {
            gate_id: contract.gate_id,
            source: contract.source.clone(),
            manifest: contract.manifest.clone(),
            repository: contract.repository.clone(),
            selection_root: contract.selection_root.clone(),
            tools: contract.tools.clone(),
            environment: contract.environment.clone(),
            processes,
            seal: Digest::compute(Domain::GateReceipt, b"unsealed verifier compiler evidence"),
        };
        evidence.seal = evidence.compute_seal();
        evidence
    }

    pub(crate) fn admits(
        &self,
        gate_id: GateId,
        source: &Digest,
        repository: &Path,
        selection_root: &Path,
        environment: &[(OsString, OsString)],
        tools: &[ToolEvidence],
    ) -> bool {
        let expected = expected_stages(gate_id);
        let process_environment = compiler_environment(environment, repository);
        self.gate_id == gate_id
            && &self.source == source
            && self.manifest == manifest_identity()
            && self.repository == repository
            && self.selection_root == selection_root
            && self.tools == tools
            && self.environment == environment
            && self.processes.len() == expected.len()
            && expected.iter().all(|stage| {
                self.processes
                    .iter()
                    .filter(|process| process.stage == *stage)
                    .count()
                    == 1
            })
            && self.processes.iter().all(|process| {
                process.gate_id == gate_id
                    && &process.source == source
                    && process.manifest == self.manifest
                    && process.repository == repository
                    && process.selection_root == selection_root
                    && process.tools == self.tools
                    && process.environment == process_environment
                    && process.exit_code == 0
                    && process.targets == process.stage.targets()
                    && process.arguments == stage_arguments(process.stage, repository)
                    && process.seal == process.compute_seal()
            })
            && self.seal == self.compute_seal()
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.processes.iter().fold(0_u64, |elapsed, process| {
            elapsed.saturating_add(process.elapsed_ms)
        })
    }

    #[cfg(test)]
    pub(crate) fn corrupt_source_for_test(&mut self) {
        self.source = Digest::compute(Domain::GateTree, b"stale verifier source fixture");
    }

    fn compute_seal(&self) -> Digest {
        let mut bytes = Vec::new();
        frame(&mut bytes, self.gate_id.as_str().as_bytes());
        frame(&mut bytes, self.source.as_str().as_bytes());
        frame(&mut bytes, self.manifest.as_str().as_bytes());
        frame(&mut bytes, self.repository.as_os_str().as_bytes());
        frame(&mut bytes, self.selection_root.as_os_str().as_bytes());
        for tool in &self.tools {
            let encoded = wire::encode(tool).expect("tool evidence is encodable");
            frame(&mut bytes, &encoded);
        }
        for (name, value) in &self.environment {
            frame(&mut bytes, name.as_bytes());
            frame(&mut bytes, value.as_bytes());
        }
        for process in &self.processes {
            frame(&mut bytes, process.seal.as_str().as_bytes());
        }
        Digest::compute(Domain::GateReceipt, &bytes)
    }
}

fn expected_stages(gate_id: GateId) -> &'static [VerifierCompilerStage] {
    match gate_id {
        GateId::RustTypecheck => &[VerifierCompilerStage::NativeClippy],
        GateId::CanvasWebTypecheck => &[
            VerifierCompilerStage::CanvasCheck,
            VerifierCompilerStage::CanvasApp,
        ],
        _ => &[],
    }
}

fn manifest_identity() -> Digest {
    Digest::compute(
        Domain::GateBatteryManifest,
        CANONICAL_BATTERY_MANIFEST.as_bytes(),
    )
}

fn cargo_program(tools: &[ToolEvidence]) -> Option<&OsStr> {
    tools.iter().find_map(|tool| match tool {
        ToolEvidence::Identified {
            requested_program,
            resolved_program,
            ..
        } if requested_program == "cargo" => Some(OsStr::new(resolved_program)),
        ToolEvidence::Missing { .. }
        | ToolEvidence::Unprobed { .. }
        | ToolEvidence::Identified { .. } => None,
    })
}

fn complete_output(
    stage: VerifierCompilerStage,
    stream: &str,
    output: Option<&sill::CapturedOutput>,
) -> io::Result<Vec<u8>> {
    let output = output.ok_or_else(|| {
        io::Error::other(format!(
            "{} compiler {stream} capture is unexpectedly absent",
            stage.tag()
        ))
    })?;
    if output.truncated() || !output.complete() {
        return Err(io::Error::other(format!(
            "{} compiler {stream} exceeded its bounded complete-output contract",
            stage.tag()
        )));
    }
    Ok(output.bytes().to_vec())
}

fn stage_arguments(stage: VerifierCompilerStage, repository: &Path) -> Vec<OsString> {
    let mut arguments = stage
        .base_arguments()
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    if stage == VerifierCompilerStage::CanvasApp {
        arguments.push(OsString::from(VERIFIER_APP_PUBLISH_FLAG));
        arguments.push(repository.as_os_str().to_owned());
    }
    arguments
}

fn compiler_environment(
    environment: &[(OsString, OsString)],
    repository: &Path,
) -> Vec<(OsString, OsString)> {
    let mut environment = environment.to_vec();
    for variable in [CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR] {
        let name = OsStr::new(variable);
        match environment
            .iter_mut()
            .find(|(candidate, _)| candidate == name)
        {
            Some((_, value)) if Path::new(value).is_relative() => {
                *value = repository.join(&*value).into_os_string();
            }
            Some(_) | None => {}
        }
    }
    if !environment
        .iter()
        .any(|(name, _)| name == OsStr::new(CARGO_TARGET_DIR))
    {
        environment.push((
            OsString::from(CARGO_TARGET_DIR),
            repository.join("target").into_os_string(),
        ));
    }
    environment.sort();
    environment
}

fn frame(into: &mut Vec<u8>, bytes: &[u8]) {
    into.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    into.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::{Command, Stdio};

    use super::*;
    use sill::ProcessGroup;

    const AUTHORITY: &[u8] = b"one-time verifier authority fixture";
    const CONTAINMENT_ROOT: &str = "GATE_VERIFIER_CONTAINMENT_ROOT";
    const COMPILER_PID: &str = "GATE_VERIFIER_COMPILER_PID";
    const DESCENDANT_PID: &str = "GATE_VERIFIER_DESCENDANT_PID";
    const CONTAINMENT_STARTUP_ALLOWANCE: Duration = Duration::from_secs(5);
    const CONTAINMENT_RUNTIME: Duration = Duration::from_secs(2);

    fn source(label: &[u8]) -> Digest {
        Digest::compute(Domain::GateTree, label)
    }

    fn tool(name: &str, program: &str) -> ToolEvidence {
        ToolEvidence::Identified {
            name: format!("fixture {name}"),
            requested_program: name.to_owned(),
            resolved_program: program.to_owned(),
            version: format!("{name} fixture-version"),
            digest: Digest::compute(Domain::GateTool, program.as_bytes()).to_string(),
        }
    }

    fn contract(gate_id: GateId) -> VerifierCompilerContract {
        let tools = match gate_id {
            GateId::RustTypecheck => vec![tool("cargo", "/usr/bin/true")],
            GateId::CanvasWebTypecheck => vec![
                tool("cargo", "/usr/bin/true"),
                tool("trunk", "/usr/bin/true"),
            ],
            _ => panic!("fixture supports compiler arms only"),
        };
        VerifierCompilerContract::new(
            gate_id,
            source(b"fixture selected tree"),
            PathBuf::from("/fixture/repository"),
            PathBuf::from("/fixture/selection"),
            tools,
            Vec::new(),
        )
        .expect("fixture compiler contract")
    }

    fn complete_processes(
        contract: &VerifierCompilerContract,
    ) -> Vec<VerifierCompilerProcessEvidence> {
        expected_stages(contract.gate_id)
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                VerifierCompilerProcessEvidence::mint(
                    contract,
                    *stage,
                    compiler_environment(&contract.environment, &contract.repository),
                    stage_arguments(*stage, &contract.repository),
                    u64::try_from(index + 1).expect("small fixture duration"),
                    Some(0),
                )
            })
            .collect()
    }

    fn admits(candidate: &VerifierCompilerEvidence, contract: &VerifierCompilerContract) -> bool {
        candidate.admits(
            contract.gate_id,
            &contract.source,
            &contract.repository,
            &contract.selection_root,
            &contract.environment,
            &contract.tools,
        )
    }

    fn first_process_mut(
        candidate: &mut VerifierCompilerEvidence,
    ) -> &mut VerifierCompilerProcessEvidence {
        candidate
            .processes
            .first_mut()
            .expect("complete evidence has a compiler process")
    }

    fn complete_evidence(contract: &VerifierCompilerContract) -> VerifierCompilerEvidence {
        contract
            .complete(&complete_processes(contract))
            .expect("complete fixture evidence")
    }

    #[test]
    fn exact_complete_evidence_admits_each_compiler_arm() {
        for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
            let contract = contract(gate_id);
            let evidence = contract
                .complete(&complete_processes(&contract))
                .expect("complete evidence");

            assert!(evidence.admits(
                gate_id,
                &contract.source,
                &contract.repository,
                &contract.selection_root,
                &contract.environment,
                &contract.tools,
            ));
        }
    }

    #[test]
    fn ordinary_verifier_compilers_own_their_process_group() {
        assert_eq!(crate::measure::bounded_process_group(), ProcessGroup::New);
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the recursive process-boundary fixture passes its private root through one exact environment"
    )]
    fn pre_commit_deadline_contains_verifier_compiler_descendants() {
        if let Some(root) = env::var_os(CONTAINMENT_ROOT) {
            run_contained_verifier_fixture(Path::new(&root));
            return;
        }

        let root = crate::toolchain::exclusive_scratch_root("gate-verifier-containment");
        let compiler = root.join("fake-cargo");
        let compiler_pid = root.join("compiler.pid");
        let descendant_pid = root.join("descendant.pid");
        fs::write(
            &compiler,
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$$\" > \"$GATE_VERIFIER_COMPILER_PID\"\n",
                "/bin/sleep 30 &\n",
                "child=$!\n",
                "printf '%s\\n' \"$child\" > \"$GATE_VERIFIER_DESCENDANT_PID\"\n",
                "wait\n",
            ),
        )
        .expect("write fake verifier compiler");
        let mut permissions = fs::metadata(&compiler)
            .expect("fake compiler metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&compiler, permissions).expect("make fake compiler executable");

        let mut environment = env::vars_os()
            .filter(|(name, _)| name != OsStr::new(CONTAINMENT_ROOT))
            .collect::<Vec<_>>();
        environment.push((
            OsString::from(CONTAINMENT_ROOT),
            root.as_os_str().to_owned(),
        ));
        let spec = Spec {
            argv: Argv::new(
                env::current_exe().expect("resolve gate test executable"),
                [
                    OsString::from("pre_commit_deadline_contains_verifier_compiler_descendants"),
                    OsString::from("--nocapture"),
                    OsString::from("--test-threads=1"),
                ],
            )
            .expect("declare contained verifier fixture"),
            cwd: root.clone(),
            environment: Environment::new(environment).expect("capture fixture environment"),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(
                CONTAINMENT_STARTUP_ALLOWANCE + CONTAINMENT_RUNTIME,
                Duration::from_millis(100),
                Duration::from_millis(100),
            )
            .expect("declare fixture deadlines"),
            wait: WaitPolicy::new(
                "pre-commit verifier containment fixture",
                Cancellation::Never,
                RestartDisposition::Never,
            )
            .expect("declare fixture wait policy"),
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture { limit: 1024 * 1024 },
                StreamPolicy::Capture { limit: 1024 * 1024 },
            ),
            redaction: Redaction::None,
            grace: Duration::from_millis(100),
        };
        let outcome = sill::spawn(spec)
            .expect("launch contained verifier fixture")
            .wait()
            .expect("terminate and reap contained verifier fixture");
        assert!(
            matches!(outcome.completion(), Completion::TimedOut(_)),
            "fixture did not reach its outer worker deadline: {:?}",
            outcome.completion()
        );
        assert_process_gone(&compiler_pid, "verifier compiler");
        assert_process_gone(&descendant_pid, "verifier compiler descendant");
        fs::remove_dir_all(root).expect("remove verifier containment fixture");
    }

    fn run_contained_verifier_fixture(root: &Path) {
        let _containment =
            crate::measure::inherit_process_group().expect("enter commit-gate containment");
        assert_eq!(
            crate::measure::bounded_process_group(),
            ProcessGroup::Inherit
        );
        let contract = VerifierCompilerContract::new(
            GateId::RustTypecheck,
            source(b"contained verifier selected tree"),
            root.to_path_buf(),
            root.to_path_buf(),
            vec![tool(
                "cargo",
                root.join("fake-cargo")
                    .to_str()
                    .expect("fixture path is UTF-8"),
            )],
            vec![
                (
                    OsString::from(COMPILER_PID),
                    root.join("compiler.pid").into_os_string(),
                ),
                (
                    OsString::from(DESCENDANT_PID),
                    root.join("descendant.pid").into_os_string(),
                ),
            ],
        )
        .expect("declare contained verifier contract");
        let run = contract
            .run(VerifierCompilerStage::NativeClippy)
            .expect("run fake verifier compiler");
        panic!(
            "outer worker deadline did not contain fake verifier compiler: completion={:?}",
            run.completion
        );
    }

    fn assert_process_gone(path: &Path, label: &str) {
        let pid = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {label} pid {}: {error}", path.display()))
            .trim()
            .parse::<u32>()
            .unwrap_or_else(|error| panic!("parse {label} pid {}: {error}", path.display()));
        for _ in 0..100 {
            if !process_exists(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Do not leave the escaped regression fixture behind after a failure.
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        panic!("outer worker deadline leaked {label} {pid}");
    }

    fn process_exists(pid: u32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn stable_preparation_mirrors_once_holds_its_lock_and_preserves_fingerprints() {
        let root = crate::toolchain::exclusive_scratch_root("gate-verifier-stable-source");
        let repository = root.join("repository");
        let selection = root.join("selection");
        fs::create_dir_all(selection.join("src")).expect("create selected source");
        fs::create_dir(&repository).expect("create repository");
        fs::write(selection.join("Cargo.toml"), b"[workspace]\n").expect("write manifest");
        fs::write(
            selection.join("src/lib.rs"),
            b"pub fn answer() -> u8 { 42 }\n",
        )
        .expect("write selected body");
        let contract = || {
            VerifierCompilerContract::new(
                GateId::CanvasWebTypecheck,
                source(b"stable compiler selected tree"),
                repository.clone(),
                selection.clone(),
                vec![
                    tool("cargo", "/usr/bin/true"),
                    tool("trunk", "/usr/bin/true"),
                ],
                Vec::new(),
            )
            .expect("stable compiler contract")
        };

        let prepared = contract().prepare_stable().expect("prepare stable source");
        let cached = canvas_source_root(&repository);
        assert_eq!(prepared.contract.selection_root, cached);
        assert_eq!(
            fs::read(cached.join("src/lib.rs")).expect("read mirrored body"),
            b"pub fn answer() -> u8 { 42 }\n"
        );
        let modified = fs::metadata(cached.join("src/lib.rs"))
            .expect("read mirrored metadata")
            .modified()
            .expect("mirrored mtime");
        let contention = contract().prepare_stable();
        assert!(
            contention
                .err()
                .is_some_and(|error| error.to_string().contains("in use")),
            "a second preparation acquired the held stable source"
        );
        drop(prepared);

        let repeated = contract().prepare_stable().expect("repeat stable source");
        assert_eq!(
            fs::metadata(cached.join("src/lib.rs"))
                .expect("read repeated metadata")
                .modified()
                .expect("repeated mtime"),
            modified,
            "unchanged selected bytes lost their Cargo fingerprint mtime"
        );
        drop(repeated);
        fs::remove_dir_all(root).expect("remove stable source fixture");
    }

    #[test]
    fn partial_or_duplicate_stage_sets_cannot_mint_evidence() {
        let contract = contract(GateId::CanvasWebTypecheck);
        let complete = complete_processes(&contract);

        assert!(contract.complete(&complete[..1]).is_err());
        assert!(
            contract
                .complete(&[complete[0].clone(), complete[0].clone()])
                .is_err()
        );
    }

    #[test]
    fn every_bound_field_mutation_invalidates_evidence() {
        let contract = contract(GateId::CanvasWebTypecheck);
        let exact = contract
            .complete(&complete_processes(&contract))
            .expect("complete evidence");

        let mut candidate = exact.clone();
        candidate.gate_id = GateId::RustTypecheck;
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.source = source(b"stale selected tree");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.manifest = Digest::compute(Domain::GateBatteryManifest, b"foreign manifest");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.repository = PathBuf::from("/foreign/repository");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.selection_root = PathBuf::from("/foreign/selection");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.tools[0] = tool("cargo", "/forged/cargo");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate
            .environment
            .push((OsString::from("FORGED"), OsString::from("forged")));
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        candidate.processes.pop();
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact;
        candidate.seal = Digest::compute(Domain::GateReceipt, b"forged seal");
        assert!(!admits(&candidate, &contract));
    }

    #[test]
    fn every_bound_process_field_mutation_invalidates_evidence() {
        let contract = contract(GateId::CanvasWebTypecheck);
        let exact = contract
            .complete(&complete_processes(&contract))
            .expect("complete evidence");

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).gate_id = GateId::RustTypecheck;
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).stage = VerifierCompilerStage::NativeClippy;
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).source = source(b"stale process source");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).manifest =
            Digest::compute(Domain::GateBatteryManifest, b"stale process manifest");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).repository = PathBuf::from("/forged/repository");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).selection_root = PathBuf::from("/forged/selection");
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).tools = vec![tool("cargo", "/forged/process-cargo")];
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate)
            .environment
            .push((OsString::from("FORGED"), OsString::from("forged")));
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate)
            .targets
            .push("foreign-target".to_owned());
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate)
            .arguments
            .push(OsString::from("--forged"));
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).exit_code = 17;
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).elapsed_ms += 1;
        assert!(!admits(&candidate, &contract));

        let mut candidate = exact.clone();
        first_process_mut(&mut candidate).seal =
            Digest::compute(Domain::GateReceipt, b"forged process seal");
        assert!(!admits(&candidate, &contract));
    }

    #[test]
    fn sealed_evidence_cannot_cross_a_consumer_environment_or_capture_root() {
        let mut producer = contract(GateId::RustTypecheck);
        producer.environment.push((
            OsString::from("RUSTFLAGS"),
            OsString::from("--cfg=producer"),
        ));
        producer.environment.sort();
        let evidence = producer
            .complete(&complete_processes(&producer))
            .expect("producer evidence");

        let mut consumer_environment = producer.environment.clone();
        let (_, rustflags) = consumer_environment
            .iter_mut()
            .find(|(name, _)| name == "RUSTFLAGS")
            .expect("fixture RUSTFLAGS");
        *rustflags = OsString::from("--cfg=consumer");
        assert!(!evidence.admits(
            producer.gate_id,
            &producer.source,
            &producer.repository,
            &producer.selection_root,
            &consumer_environment,
            &producer.tools,
        ));
        assert!(!evidence.admits(
            producer.gate_id,
            &producer.source,
            &producer.repository,
            Path::new("/another/capture/root"),
            &producer.environment,
            &producer.tools,
        ));
    }

    #[test]
    fn canonical_receipt_crosses_capture_roots_without_leaking_environment_values() {
        let mut producers = [
            contract(GateId::RustTypecheck),
            contract(GateId::CanvasWebTypecheck),
        ];
        for producer in &mut producers {
            producer.environment.push((
                OsString::from("RECEIPT_SECRET_FIXTURE"),
                OsString::from("receipt-secret-must-not-cross-process"),
            ));
            producer.environment.sort();
        }
        let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
        let receipt =
            encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode receipt");
        assert!(
            !receipt
                .windows(b"receipt-secret-must-not-cross-process".len())
                .any(|window| window == b"receipt-secret-must-not-cross-process"),
            "receipt leaked an ambient environment value"
        );
        assert!(
            !receipt
                .windows(AUTHORITY.len())
                .any(|window| window == AUTHORITY),
            "receipt leaked its one-time provenance authority"
        );

        let consumers = producers
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, mut consumer)| {
                consumer.selection_root = PathBuf::from(format!("/consumer/selection/{index}"));
                consumer
            })
            .collect::<Vec<_>>();
        let admitted = admit_verifier_compiler_receipt(&receipt, &consumers, AUTHORITY)
            .expect("fresh receipt admits across private capture roots");
        assert_eq!(admitted.len(), consumers.len());
        assert!(
            admitted
                .iter()
                .zip(&consumers)
                .all(|(candidate, consumer)| admits(candidate, consumer))
        );
    }

    #[test]
    fn missing_partial_duplicate_and_stale_receipts_fail_closed() {
        let producers = [
            contract(GateId::RustTypecheck),
            contract(GateId::CanvasWebTypecheck),
        ];
        let evidence = producers.iter().map(complete_evidence).collect::<Vec<_>>();
        let exact =
            encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode exact receipt");

        assert!(admit_verifier_compiler_receipt(&[], &producers, AUTHORITY).is_err());
        assert!(admit_verifier_compiler_receipt(&exact, &producers, b"foreign authority").is_err());
        let partial =
            encode_verifier_compiler_receipt(&evidence[..1], AUTHORITY).expect("encode partial");
        assert!(admit_verifier_compiler_receipt(&partial, &producers, AUTHORITY).is_err());
        let duplicate = encode_verifier_compiler_receipt(
            &[evidence[0].clone(), evidence[0].clone()],
            AUTHORITY,
        )
        .expect("encode duplicate");
        assert!(admit_verifier_compiler_receipt(&duplicate, &producers, AUTHORITY).is_err());

        let mut stale_source = producers.to_vec();
        stale_source[0].source = source(b"changed staged source");
        assert!(admit_verifier_compiler_receipt(&exact, &stale_source, AUTHORITY).is_err());

        let mut stale_environment = producers.to_vec();
        stale_environment[0]
            .environment
            .push((OsString::from("RUSTFLAGS"), OsString::from("--cfg=changed")));
        stale_environment[0].environment.sort();
        assert!(admit_verifier_compiler_receipt(&exact, &stale_environment, AUTHORITY).is_err());

        let mut stale_tool = producers.to_vec();
        stale_tool[0].tools = vec![tool("cargo", "/changed/cargo")];
        assert!(admit_verifier_compiler_receipt(&exact, &stale_tool, AUTHORITY).is_err());
    }

    #[test]
    fn malformed_noncanonical_and_tampered_receipts_fail_closed() {
        let contracts = [
            contract(GateId::RustTypecheck),
            contract(GateId::CanvasWebTypecheck),
        ];
        let evidence = contracts.iter().map(complete_evidence).collect::<Vec<_>>();
        let exact =
            encode_verifier_compiler_receipt(&evidence, AUTHORITY).expect("encode exact receipt");

        let mut noncanonical = exact.clone();
        noncanonical.push(b'\n');
        assert!(admit_verifier_compiler_receipt(&noncanonical, &contracts, AUTHORITY).is_err());

        let mut unsupported: VerifierCompilerReceiptSet =
            wire::decode(&exact).expect("decode receipt fixture");
        unsupported.version += 1;
        unsupported.authority_seal = unsupported.compute_authority_seal(AUTHORITY);
        let unsupported = wire::encode(&unsupported).expect("encode unsupported receipt");
        assert!(admit_verifier_compiler_receipt(&unsupported, &contracts, AUTHORITY).is_err());

        let mut partial_stage: VerifierCompilerReceiptSet =
            wire::decode(&exact).expect("decode receipt fixture");
        partial_stage.evidence[1].processes.pop();
        partial_stage.evidence[1].seal = partial_stage.evidence[1].compute_seal();
        partial_stage.authority_seal = partial_stage.compute_authority_seal(AUTHORITY);
        let partial_stage = wire::encode(&partial_stage).expect("encode partial-stage receipt");
        assert!(admit_verifier_compiler_receipt(&partial_stage, &contracts, AUTHORITY).is_err());

        let mut tampered: VerifierCompilerReceiptSet =
            wire::decode(&exact).expect("decode receipt fixture");
        tampered.evidence[0].processes[0].targets =
            Digest::compute(Domain::GateReceipt, b"foreign targets");
        tampered.evidence[0].processes[0].seal = tampered.evidence[0].processes[0].compute_seal();
        tampered.evidence[0].seal = tampered.evidence[0].compute_seal();
        tampered.authority_seal = tampered.compute_authority_seal(AUTHORITY);
        let tampered = wire::encode(&tampered).expect("encode tampered receipt");
        assert!(admit_verifier_compiler_receipt(&tampered, &contracts, AUTHORITY).is_err());
    }

    #[test]
    fn ignored_live_compiler_inputs_cannot_mint_selected_source_evidence() {
        let root = crate::toolchain::exclusive_scratch_root("gate-verifier-selected-source");
        let repository = root.join("repository");
        let selection = root.join("selection");
        fs::create_dir_all(repository.join(".cargo")).expect("live Cargo configuration root");
        fs::create_dir_all(repository.join("ignored")).expect("live ignored build root");
        fs::create_dir(&selection).expect("immutable selected root");
        let manifest = concat!(
            "[package]\n",
            "name = \"selected-source-fixture\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2024\"\n",
            "build = \"ignored/build.rs\"\n",
        );
        for root in [&repository, &selection] {
            fs::write(root.join("Cargo.toml"), manifest).expect("tracked fixture manifest");
            fs::write(root.join(".gitignore"), ".cargo/config.toml\nignored/\n")
                .expect("tracked ignored-input policy");
        }
        fs::write(
            repository.join(".cargo/config.toml"),
            "[build]\nrustflags = []\n",
        )
        .expect("ignored live Cargo configuration");
        fs::write(repository.join("ignored/build.rs"), "fn main() {}\n")
            .expect("ignored live build script");
        for (label, live_only_input) in [
            ("ignored-cargo-config", ".cargo/config.toml"),
            ("ignored-build-script", "ignored/build.rs"),
        ] {
            let cargo = root.join(label);
            fs::write(
                &cargo,
                format!(
                    concat!(
                        "#!/bin/sh\n",
                        "if [ -f '{live_only_input}' ]; then\n",
                        "  exit 0\n",
                        "fi\n",
                        "exit 19\n",
                    ),
                    live_only_input = live_only_input,
                ),
            )
            .expect("write compiler-input witness");
            fs::set_permissions(
                &cargo,
                fs::metadata("/usr/bin/true")
                    .expect("read fixture executable permissions")
                    .permissions(),
            )
            .expect("make compiler-input witness executable");
            let contract = VerifierCompilerContract::new(
                GateId::RustTypecheck,
                source(label.as_bytes()),
                repository.clone(),
                selection.clone(),
                vec![tool("cargo", cargo.to_str().expect("UTF-8 fixture path"))],
                Vec::new(),
            )
            .expect("selected-source compiler contract");

            let run = contract
                .run(VerifierCompilerStage::NativeClippy)
                .expect("compiler-input witness launches");

            assert!(!run.success());
            assert!(
                run.into_evidence().is_none(),
                "{live_only_input} minted evidence from outside the selected root"
            );
            assert!(
                contract.complete(&[]).is_err(),
                "{live_only_input} admitted evidence without a successful process token"
            );
        }
        fs::remove_dir_all(root).expect("remove selected-source fixture");
    }

    #[test]
    fn a_failed_process_returns_no_reusable_token() {
        let mut contract = contract(GateId::RustTypecheck);
        contract.repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate crate has a workspace parent")
            .to_path_buf();
        contract.selection_root = contract.repository.clone();
        contract.tools = vec![tool("cargo", "/usr/bin/false")];

        let run = contract
            .run(VerifierCompilerStage::NativeClippy)
            .expect("false launches");

        assert!(!run.success());
        assert!(run.into_evidence().is_none());
    }

    #[test]
    fn failed_process_feedback_is_captured_completely_without_minting_evidence() {
        let root = crate::toolchain::exclusive_scratch_root("gate-verifier-captured-output");
        let repository = root.join("repository");
        let selection = root.join("selection");
        fs::create_dir(&repository).expect("fixture repository");
        fs::create_dir(&selection).expect("fixture selected root");
        let cargo = root.join("cargo");
        fs::write(
            &cargo,
            "#!/bin/sh\nprintf 'bounded stdout\\n'\nprintf 'bounded stderr\\n' >&2\nexit 23\n",
        )
        .expect("write fixture Cargo executable");
        fs::set_permissions(
            &cargo,
            fs::metadata("/usr/bin/true")
                .expect("read fixture executable permissions")
                .permissions(),
        )
        .expect("make fixture Cargo executable");
        let contract = VerifierCompilerContract::new(
            GateId::RustTypecheck,
            source(b"captured output selected tree"),
            repository,
            selection,
            vec![tool("cargo", cargo.to_str().expect("UTF-8 fixture path"))],
            Vec::new(),
        )
        .expect("captured output contract");

        let run = contract
            .run(VerifierCompilerStage::NativeClippy)
            .expect("fixture Cargo launches");

        assert!(!run.success());
        assert_eq!(run.code(), Some(23));
        assert_eq!(run.stdout(), b"bounded stdout\n");
        assert_eq!(run.stderr(), b"bounded stderr\n");
        assert!(!run.cancelled());
        assert!(!run.timed_out());
        assert!(run.into_evidence().is_none());
        fs::remove_dir_all(root).expect("remove captured output fixture");
    }
}
