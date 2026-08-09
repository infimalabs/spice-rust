//! Opaque compiler evidence passed from the full verifier into one battery run.
//!
//! The verifier already compiles the native workspace and the browser target
//! before it reaches the canonical battery.  These values let that one caller
//! hand the successful process evidence forward without turning success into a
//! string, an environment flag, or a reusable file. A process token can only
//! be minted by executing a [`VerifierCompilerContract`], and a complete arm
//! token can only be assembled from the exact successful stages that arm requires.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Output,
    ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use wire::{Digest, Domain};

use super::ToolEvidence;
use crate::battery::CANONICAL_BATTERY_MANIFEST;
use crate::manifest::GateId;

const GIT: &str = "/usr/bin/git";
const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";
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
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

    const fn arguments(self) -> &'static [&'static str] {
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
    evidence_refusal: Option<String>,
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

    /// Exact selected-tree invariant that prevented reusable evidence.
    #[must_use]
    pub fn evidence_refusal(&self) -> Option<&str> {
        self.evidence_refusal.as_deref()
    }

    /// Successful reusable evidence, absent after failure or source drift.
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

    /// Bind this contract and the evidence it may mint to one Cargo job cap.
    #[must_use]
    pub fn with_cargo_jobs(mut self, cargo_jobs: usize) -> Self {
        self.environment
            .retain(|(name, _)| name != OsStr::new(CARGO_BUILD_JOBS));
        self.environment.push((
            OsString::from(CARGO_BUILD_JOBS),
            OsString::from(cargo_jobs.max(1).to_string()),
        ));
        self
    }

    /// Run one exact compiler stage and mint evidence only after a clean exit.
    ///
    /// The worktree/index relation is checked on both sides of the process.  A
    /// command still provides ordinary verifier feedback when that relation is
    /// not exact, but it cannot then stand in for analysis of the staged tree.
    ///
    /// # Errors
    ///
    /// Returns an unsupported stage or process-launch failure.
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
        let source_before =
            selection_matches_worktree(&self.repository, &self.selection_root, &self.environment);
        let started = Instant::now();
        let environment = Environment::new(self.environment.clone()).map_err(io::Error::other)?;
        let deadlines = Deadlines::new(PROCESS_RUNTIME, OUTPUT_DRAIN, REAP_DEADLINE)
            .map_err(io::Error::other)?;
        let wait = WaitPolicy::new(
            format!("verifier compiler stage {}", stage.tag()),
            cancellation,
            RestartDisposition::Never,
        )
        .map_err(io::Error::other)?;
        let spec = Spec {
            argv: Argv::new(program, stage.arguments()).map_err(io::Error::other)?,
            cwd: self.repository.clone(),
            environment,
            group: ProcessGroup::New,
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
        let source_after =
            selection_matches_worktree(&self.repository, &self.selection_root, &self.environment);
        let success = status.success();
        let code = status.code();
        let completion = match outcome.completion() {
            Completion::Exited(_) => VerifierCompilerCompletion::Exited,
            Completion::Cancelled(_) => VerifierCompilerCompletion::Cancelled,
            Completion::TimedOut(_) => VerifierCompilerCompletion::TimedOut,
        };
        let evidence_refusal = source_before.err().or_else(|| source_after.err());
        let evidence = (completion == VerifierCompilerCompletion::Exited
            && success
            && evidence_refusal.is_none())
        .then(|| VerifierCompilerProcessEvidence::mint(self, stage, elapsed_ms, code));
        Ok(VerifierCompilerRun {
            success,
            code,
            evidence,
            stdout,
            stderr,
            completion,
            evidence_refusal,
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
    tools: Vec<ToolEvidence>,
    environment: Vec<(OsString, OsString)>,
    targets: Vec<String>,
    arguments: Vec<String>,
    exit_code: i32,
    elapsed_ms: u64,
    seal: Digest,
}

impl VerifierCompilerProcessEvidence {
    fn mint(
        contract: &VerifierCompilerContract,
        stage: VerifierCompilerStage,
        elapsed_ms: u64,
        code: Option<i32>,
    ) -> Self {
        let mut evidence = Self {
            gate_id: contract.gate_id,
            stage,
            source: contract.source.clone(),
            manifest: contract.manifest.clone(),
            repository: contract.repository.clone(),
            tools: contract.tools.clone(),
            environment: contract.environment.clone(),
            targets: stage
                .targets()
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            arguments: stage
                .arguments()
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
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
            && self.tools == contract.tools
            && self.environment == contract.environment
            && self.targets == self.stage.targets()
            && self.arguments == self.stage.arguments()
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
            frame(&mut bytes, argument.as_bytes());
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
    tools: Vec<ToolEvidence>,
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
            tools: contract.tools.clone(),
            processes,
            seal: Digest::compute(Domain::GateReceipt, b"unsealed verifier compiler evidence"),
        };
        evidence.seal = evidence.compute_seal();
        evidence
    }

    pub(crate) fn admits(&self, gate_id: GateId, source: &Digest, tools: &[ToolEvidence]) -> bool {
        let expected = expected_stages(gate_id);
        self.gate_id == gate_id
            && &self.source == source
            && self.manifest == manifest_identity()
            && self.tools == tools
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
                    && process.tools == self.tools
                    && process.exit_code == 0
                    && process.targets == process.stage.targets()
                    && process.arguments == process.stage.arguments()
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
        for tool in &self.tools {
            let encoded = wire::encode(tool).expect("tool evidence is encodable");
            frame(&mut bytes, &encoded);
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

fn selection_matches_worktree(
    repository: &Path,
    selection: &Path,
    environment: &[(OsString, OsString)],
) -> Result<(), String> {
    let indexed = Command::new(GIT)
        .args([
            "-C",
            repository.to_string_lossy().as_ref(),
            "ls-files",
            "-z",
        ])
        .env_clear()
        .envs(environment.iter().cloned())
        .output()
        .map_err(|error| format!("could not launch git ls-files: {error}"))?;
    if !indexed.status.success() {
        return Err(format!(
            "git ls-files exited {:?}: {}",
            indexed.status.code(),
            String::from_utf8_lossy(&indexed.stderr).trim()
        ));
    }
    let mut indexed = indexed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())))
        .collect::<Vec<_>>();
    indexed.sort();
    let mut selected = selected_paths(selection)
        .map_err(|error| format!("selected-tree paths could not be read: {error}"))?;
    selected.sort();
    if indexed != selected {
        let indexed_set = indexed.iter().collect::<BTreeSet<_>>();
        let selected_set = selected.iter().collect::<BTreeSet<_>>();
        let index_only = indexed_set.difference(&selected_set).next();
        let selection_only = selected_set.difference(&indexed_set).next();
        return Err(format!(
            "tracked path sets differ: index-only={index_only:?}, selection-only={selection_only:?}"
        ));
    }
    if let Some(path) = indexed
        .iter()
        .find(|path| !same_selected_entry(&selection.join(path), &repository.join(path)))
    {
        return Err(format!(
            "tracked worktree entry {path:?} differs from the selected tree"
        ));
    }
    let untracked = Command::new(GIT)
        .args([
            "-C",
            repository.to_string_lossy().as_ref(),
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .env_clear()
        .envs(environment.iter().cloned())
        .output()
        .map_err(|error| format!("could not launch git untracked-path scan: {error}"))?;
    if !untracked.status.success() {
        return Err(format!(
            "git untracked-path scan exited {:?}: {}",
            untracked.status.code(),
            String::from_utf8_lossy(&untracked.stderr).trim()
        ));
    }
    let first_untracked = untracked
        .stdout
        .split(|byte| *byte == 0)
        .find(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())));
    match first_untracked {
        Some(path) => Err(format!(
            "untracked worktree entry {path:?} is absent from the selected tree"
        )),
        None => Ok(()),
    }
}

fn selected_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut directories = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                directories.push(path);
            } else {
                paths.push(
                    path.strip_prefix(root)
                        .expect("selected descendants retain their root")
                        .to_path_buf(),
                );
            }
        }
    }
    Ok(paths)
}

fn same_selected_entry(selected: &Path, worktree: &Path) -> bool {
    let (Ok(selected_metadata), Ok(worktree_metadata)) = (
        fs::symlink_metadata(selected),
        fs::symlink_metadata(worktree),
    ) else {
        return false;
    };
    let selected_type = selected_metadata.file_type();
    let worktree_type = worktree_metadata.file_type();
    if selected_type.is_symlink() != worktree_type.is_symlink()
        || selected_type.is_file() != worktree_type.is_file()
        || git_executable(selected_metadata.mode()) != git_executable(worktree_metadata.mode())
    {
        return false;
    }
    if selected_type.is_symlink() {
        return fs::read_link(selected).ok() == fs::read_link(worktree).ok();
    }
    if !selected_type.is_file() {
        return false;
    }
    matches!(
        (fs::read(selected), fs::read(worktree)),
        (Ok(selected), Ok(worktree)) if selected == worktree
    )
}

const fn git_executable(mode: u32) -> bool {
    mode & 0o111 != 0
}

fn frame(into: &mut Vec<u8>, bytes: &[u8]) {
    into.extend_from_slice(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    into.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(label: &[u8]) -> Digest {
        Digest::compute(Domain::GateTree, label)
    }

    #[test]
    fn git_executability_ignores_normalized_permission_width() {
        assert_eq!(git_executable(0o500), git_executable(0o755));
        assert_ne!(git_executable(0o400), git_executable(0o500));
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
                    u64::try_from(index + 1).expect("small fixture duration"),
                    Some(0),
                )
            })
            .collect()
    }

    #[test]
    fn exact_complete_evidence_admits_each_compiler_arm() {
        for gate_id in [GateId::RustTypecheck, GateId::CanvasWebTypecheck] {
            let contract = contract(gate_id);
            let evidence = contract
                .complete(&complete_processes(&contract))
                .expect("complete evidence");

            assert!(evidence.admits(gate_id, &contract.source, &contract.tools));
        }
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
        let admits = |candidate: &VerifierCompilerEvidence| {
            candidate.admits(contract.gate_id, &contract.source, &contract.tools)
        };

        let mut candidate = exact.clone();
        candidate.gate_id = GateId::RustTypecheck;
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.source = source(b"stale selected tree");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.manifest = Digest::compute(Domain::GateBatteryManifest, b"foreign manifest");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.tools[0] = tool("cargo", "/forged/cargo");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].gate_id = GateId::RustTypecheck;
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].stage = VerifierCompilerStage::NativeClippy;
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].source = source(b"stale process source");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].manifest =
            Digest::compute(Domain::GateBatteryManifest, b"stale process manifest");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].repository = PathBuf::from("/forged/repository");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].tools[0] = tool("cargo", "/forged/process-cargo");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0]
            .environment
            .push((OsString::from("FORGED"), OsString::from("1")));
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0]
            .targets
            .push("foreign-target".to_owned());
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].arguments.push("--forged".to_owned());
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].exit_code = 17;
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].elapsed_ms += 1;
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes[0].seal = Digest::compute(Domain::GateReceipt, b"forged process seal");
        assert!(!admits(&candidate));

        let mut candidate = exact.clone();
        candidate.processes.pop();
        assert!(!admits(&candidate));

        let mut candidate = exact;
        candidate.seal = Digest::compute(Domain::GateReceipt, b"forged seal");
        assert!(!admits(&candidate));
    }

    #[test]
    fn a_failed_process_returns_no_reusable_token() {
        let mut contract = contract(GateId::RustTypecheck);
        contract.repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate crate has a workspace parent")
            .to_path_buf();
        contract.tools = vec![tool("cargo", "/usr/bin/false")];

        let run = contract
            .run(VerifierCompilerStage::NativeClippy)
            .expect("false launches");

        assert!(!run.success());
        assert!(run.into_evidence().is_none());
    }
}
