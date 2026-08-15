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
    CANVAS_SOURCE_LABEL, CANVAS_SOURCE_LOCK, canvas_output_roots, canvas_source_root,
    make_cache_writable, rust_analysis_workspace, synchronize_source,
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
const RUST_BUILD_DIRECTORY: &str = "spice-gate-native-build";
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
        let environment = contract_environment(gate_id, environment, &repository);
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

    /// Selected source and Cargo output roots this native Rust compiler contract
    /// has populated and later workspace tests may reuse serially.
    #[must_use]
    pub fn reusable_workspace(&self) -> Option<super::RustAnalysisWorkspace> {
        (self.gate_id == GateId::RustTypecheck).then(|| {
            let environment = compiler_environment(&self.environment, &self.repository);
            let target = environment_path(&environment, CARGO_TARGET_DIR)
                .unwrap_or_else(|| self.repository.join("target"));
            let build = environment_path(&environment, CARGO_BUILD_BUILD_DIR)
                .unwrap_or_else(|| target.join(RUST_BUILD_DIRECTORY));
            super::RustAnalysisWorkspace {
                source: self.selection_root.clone(),
                cargo_target: target,
                cargo_build: build,
            }
        })
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

fn contract_environment(
    gate_id: GateId,
    mut environment: Vec<(OsString, OsString)>,
    repository: &Path,
) -> Vec<(OsString, OsString)> {
    if gate_id != GateId::CanvasWebTypecheck {
        return environment;
    }
    let (target, build) = canvas_output_roots(repository);
    set_environment_value(&mut environment, CARGO_TARGET_DIR, target);
    set_environment_value(&mut environment, CARGO_BUILD_BUILD_DIR, build);
    environment.sort();
    environment
}

fn set_environment_value(environment: &mut Vec<(OsString, OsString)>, name: &str, value: PathBuf) {
    match environment
        .iter_mut()
        .find(|(candidate, _)| candidate == OsStr::new(name))
    {
        Some((_, current)) => *current = value.into_os_string(),
        None => environment.push((OsString::from(name), value.into_os_string())),
    }
}

/// One compiler contract bound to a verified stable mirror of its selection.
pub struct PreparedVerifierCompiler {
    contract: VerifierCompilerContract,
    source: StableCompilerSource,
}

impl PreparedVerifierCompiler {
    /// Gate arm owned by this stable compiler source.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.contract.gate_id()
    }

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

    /// Run one exact stage from the held stable source mirror under cancellation.
    ///
    /// # Errors
    ///
    /// Returns the same supervision failures as
    /// [`VerifierCompilerContract::run_supervised`].
    pub fn run_supervised(
        &self,
        stage: VerifierCompilerStage,
        cancellation: CancellationToken,
    ) -> io::Result<VerifierCompilerRun> {
        self.contract.run_supervised(stage, cancellation)
    }

    /// Native compiler output roots which workspace tests may reuse.
    #[must_use]
    pub fn reusable_workspace(&self) -> Option<super::RustAnalysisWorkspace> {
        self.contract.reusable_workspace()
    }

    /// Complete the arm proof while retaining stable-source possession.
    ///
    /// The prepared contract remains locked until it is dropped, allowing the
    /// full verifier to construct both its battery receipt and terminal durable
    /// evidence from one unchanged compiler authority.
    ///
    /// # Errors
    ///
    /// Refuses incomplete evidence or a source mirror that cannot be returned
    /// to its writable persistent state.
    pub fn complete(
        &self,
        processes: &[VerifierCompilerProcessEvidence],
    ) -> Result<VerifierCompilerEvidence, String> {
        self.contract.complete(processes)
    }

    /// Release the stable source mirror after every proof consumer has finished.
    ///
    /// # Errors
    ///
    /// Returns a permission error if the persistent mirror cannot be restored
    /// to its writable idle state.
    pub fn release(&mut self) -> io::Result<()> {
        self.source.release()
    }
}

struct StableCompilerSource {
    root: PathBuf,
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
        let environment = contract_environment(gate_id, environment.to_vec(), repository);
        let process_environment = compiler_environment(&environment, repository);
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
    let target = environment_path(&environment, CARGO_TARGET_DIR)
        .unwrap_or_else(|| repository.join("target"));
    if !environment
        .iter()
        .any(|(name, _)| name == OsStr::new(CARGO_BUILD_BUILD_DIR))
    {
        environment.push((
            OsString::from(CARGO_BUILD_BUILD_DIR),
            target.join(RUST_BUILD_DIRECTORY).into_os_string(),
        ));
    }
    environment.sort();
    environment
}

fn environment_path(environment: &[(OsString, OsString)], name: &str) -> Option<PathBuf> {
    environment
        .iter()
        .find(|(candidate, _)| candidate == OsStr::new(name))
        .map(|(_, value)| PathBuf::from(value))
}

fn frame(into: &mut Vec<u8>, bytes: &[u8]) {
    into.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    into.extend_from_slice(bytes);
}

#[cfg(test)]
#[path = "verifier_evidence/tests.rs"]
mod tests;
