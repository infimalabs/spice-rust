//! Opaque compiler evidence passed from the full verifier into one battery run.
//!
//! The verifier already compiles the native workspace and the browser target
//! before it reaches the canonical battery.  These values let that one caller
//! hand the successful process evidence forward without turning success into a
//! string, an environment flag, or a reusable file.  A process token can only
//! be minted by [`VerifierCompilerContract::run`], and a complete arm token can
//! only be assembled from the exact successful stages that arm requires.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use wire::{Digest, Domain};

use super::ToolEvidence;
use crate::VERIFIER_APP_PUBLISH_FLAG;
use crate::battery::CANONICAL_BATTERY_MANIFEST;
use crate::manifest::GateId;

const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";
const CARGO_BUILD_BUILD_DIR: &str = "CARGO_BUILD_BUILD_DIR";

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

    /// Run one exact compiler stage and mint evidence only after a clean exit.
    ///
    /// The process runs from the immutable selected source root. Compiler
    /// outputs are redirected into the live repository, and the app stage is
    /// given an explicit separate publication root, so neither output nor
    /// ignored live-worktree inputs can change what was analyzed.
    ///
    /// # Errors
    ///
    /// Returns an unsupported stage or process-launch failure.
    pub fn run(&self, stage: VerifierCompilerStage) -> io::Result<VerifierCompilerRun> {
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
        let status = Command::new(program)
            .args(&arguments)
            .current_dir(&self.selection_root)
            .env_clear()
            .envs(environment.iter().cloned())
            .status()?;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let success = status.success();
        let code = status.code();
        let evidence = success.then(|| {
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

/// One successful process token.  Only [`VerifierCompilerContract::run`] can
/// mint it; callers may retain and return it but cannot alter its fields.
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
    use std::fs;

    use super::*;

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
}
