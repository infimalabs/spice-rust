//! Repository-local build, verification, and release commands.

use std::io::{self, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

use sill::CancellationToken;

mod app;
mod build_cache;
mod develop;
mod gate_command;
mod landing;
mod proof_image;
mod release;
#[cfg(not(target_arch = "wasm32"))]
mod test_evidence;
mod verify_browser;
mod verify_error;
mod verify_execution;
mod verify_timing;
#[cfg(not(target_arch = "wasm32"))]
mod workspace_tests;

use app::WASM_TARGET;
pub use app::{AppBuildError, app, app_to};
pub use build_cache::{
    BuildCacheContext, BuildCacheDisposition, BuildCacheError, BuildCacheReport, BuildCacheRequest,
    BuildCacheRootReport, build_cache, collect_build_cache_at, parse_byte_quantity,
};
pub use develop::{RefreshError, RefreshReport, RefreshRequest, refresh};
pub use gate_command::{gate_arm, gate_arm_report};
use verify_browser::BrowserRun;
use verify_error::valid_package_name;
pub use verify_error::{GateArmError, VerifyError};
#[cfg(test)]
const BROWSER_RECEIPT: &[u8] = verify_browser::RECEIPT;
// The install step drives `spice::install`, which the appliance itself gates
// off the wasm target; the workspace wasm check compiles this crate too.
#[cfg(not(target_arch = "wasm32"))]
pub use develop::refresh_install;
pub use proof_image::{ProofImageError, build_proof_image, load_proof_image};
pub use release::{CommandRecord, ReleaseEnvironment, TargetBuild, TargetBuildError, stage_target};
#[cfg(not(target_arch = "wasm32"))]
pub use test_evidence::{TestEvidenceRunError, run as run_test_evidence};
#[cfg(not(target_arch = "wasm32"))]
pub use workspace_tests::{
    WorkspaceTestError, WorkspaceTestExecution,
    prebuild_shared_at as prebuild_shared_workspace_tests_at, run as run_workspace_tests,
    run_at as run_workspace_tests_at,
    run_prebuilt_shared_at as run_prebuilt_shared_workspace_tests_at,
};

/// Every verification action, in stable report order.
///
/// `gate/src/battery/manifest.toml` names all 25 retired standard policy checks
/// and binds each to one compiled Rust arm; its additional Rust and C# arms are
/// native because they carry no retired-step name. The workspace tests exercise
/// that exact 27-arm manifest and the production executor for all four families,
/// including the structural family that once lacked acquisition. The real-browser
/// action is the one non-Cargo runtime check and requires its end-of-run receipt.
/// Contract landing remains terminal: no alternate executable discovered from
/// `PATH` repeats policy after the Rust-owned checks finish. Execution order is
/// constrained by the explicit graph in `verify_execution`, not this report
/// order.
const VERIFY_ACTIONS: [Action; 13] = [
    Action::Command(Step::cargo(
        RESIDENT_BUILD_LABEL,
        &["build", "--locked", "-p", "spice", "--bin", "spice"],
    )),
    Action::EntryPoint,
    Action::Format,
    Action::Command(Step::cargo(
        NATIVE_CLIPPY_LABEL,
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )),
    Action::Command(Step::cargo(
        "cargo clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
        &[
            "clippy",
            "--locked",
            "-p",
            "app",
            "--target",
            WASM_TARGET,
            "--",
            "-D",
            "warnings",
        ],
    )),
    Action::Command(Step::cargo(
        CANVAS_CHECK_LABEL,
        &["check", "--locked", "--workspace", "--target", WASM_TARGET],
    )),
    Action::Command(Step::cargo(CANVAS_APP_LABEL, &["xtask", "app"])),
    Action::Browser,
    Action::WorkspaceTests,
    Action::Command(Step::cargo(
        DOCUMENTATION_LABEL,
        &[
            "doc",
            "--locked",
            "--workspace",
            "--lib",
            "--bins",
            "--no-deps",
        ],
    )),
    Action::Command(Step::cargo(
        "cargo test --locked --workspace --doc",
        &["test", "--locked", "--workspace", "--doc"],
    )),
    Action::Battery,
    Action::Landing,
];

const SCOPED_COMMANDS: [ScopedCommand; 3] = [
    ScopedCommand::Clippy,
    ScopedCommand::Test,
    ScopedCommand::DocTest,
];

const BATTERY_LABEL: &str = "canonical Rust gate battery";
const STRUCTURAL_PREFLIGHT_LABEL: &str =
    "selected file-shape and routine-complexity gate preflight";
/// Direct formatting over the gate-owned complete Rust-source selection.
const FORMAT_LABEL: &str =
    "rustfmt --edition <workspace> --check --files-with-diff <gate::formatter_sources>";
/// Documentation is swept with the targets named, not with cargo's default set.
///
/// This is also the only action that resolves intra-doc links at all.
/// `cargo test --locked --workspace --doc` compiles and runs the code *inside*
/// doc comments and never resolves a link, so a link naming an item that does
/// not exist passes it. That gap is measured rather than assumed: CLI-1kKlPrCW
/// repaired one broken link, CLI-1kKlQfSH then found seven more across app,
/// proof, wire and gate, and every one had landed through a fully green verify.
///
/// `spice` and `xtask` each auto-discover a `src/lib.rs` and a same-named
/// `src/main.rs`, and both targets would write `target/doc/<name>/index.html`.
/// Left to choose, cargo settles that collision by documenting the library and
/// dropping the binary silently: no warning, exit 0. `--lib --bins` asks for
/// both, so cargo documents each binary too, prints its own
/// `rust-lang/cargo#6313` collision note, and runs rustdoc over
/// `spice/src/main.rs` and `xtask/src/main.rs` -- including the twenty-three
/// modules `spice/src/main.rs` declares that its library does not. Under
/// `-D warnings` a broken intra-doc link in either file is a hard error, and
/// under the default target set nothing in this tree ever raised it.
const DOCUMENTATION_LABEL: &str = "cargo doc --locked --workspace --lib --bins --no-deps";
/// Repository policy for the completion publication boundary.
///
/// By default the preceding supervised Cargo action supplies the current Spice
/// executable to the receipt-backed appliance audit. A source repository whose
/// outer loop owns `cargo xtask install` may explicitly suppress that audit's
/// apply guidance; verification then completes silently without reading or
/// mutating the shared appliance.
const ENTRY_POINT_LABEL: &str = "repository completion publication boundary";
/// Real-browser guard for the production Mosaic engine edge.
///
/// BROWSER-1kKSWhZk removed one of the two former surfaces. This mandatory
/// action protects the remaining `composer -> mosaic_plane -> mosaic` path.
const BROWSER_LABEL: &str = "npm run test:canvas (BROWSER-1kKSWhZk guard)";
const CANVAS_APP_LABEL: &str = "cargo xtask app (canvas compiler evidence)";
const CANVAS_CHECK_LABEL: &str = "cargo check --locked --workspace --target wasm32-unknown-unknown";
const NATIVE_CLIPPY_LABEL: &str = "cargo clippy --locked --workspace --all-targets -- -D warnings";
const RESIDENT_BUILD_LABEL: &str =
    "cargo build --locked -p spice --bin spice (freshness reference and canvas resident)";
#[cfg(not(target_arch = "wasm32"))]
const STAGED_SOURCE_STATUS_LABEL: &str =
    "git --no-optional-locks status --porcelain=v1 -z --untracked-files=all";
const FULL_VERIFIER_WORKSPACE_TEST_CONCURRENCY_CAP: usize =
    gate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP;
const WORKSPACE_TEST_LABEL: &str =
    "deterministic sharded cargo test --locked --workspace --all-targets";
/// Frozen from the canonical prebuilt 49-shard measurement recorded by
/// TESTS-1kNMfCLc: 441.78 user seconds plus 186.31 system seconds.
/// Test additions must keep the same total CPU budget by removing or reducing
/// compute elsewhere; wall-clock supervision remains independently unchanged.
const WORKSPACE_TEST_COMPUTE_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(628_090);
const WORKSPACE_TEST_PREBUILD_LABEL: &str =
    "native test and Canvas launcher prebuild (workspace-test setup)";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Step {
    label: &'static str,
    arguments: &'static [&'static str],
}

impl Step {
    const fn cargo(label: &'static str, arguments: &'static [&'static str]) -> Self {
        Self { label, arguments }
    }

    fn uses_verifier_compiler_contract(self) -> bool {
        matches!(
            self.label,
            NATIVE_CLIPPY_LABEL | CANVAS_CHECK_LABEL | CANVAS_APP_LABEL
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    EntryPoint,
    Format,
    Command(Step),
    Browser,
    WorkspaceTests,
    Battery,
    Landing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScopedCommand {
    Clippy,
    Test,
    DocTest,
}

impl ScopedCommand {
    fn label(self, package: &str) -> String {
        match self {
            Self::Clippy => {
                format!("cargo clippy --locked --package {package} --all-targets -- -D warnings")
            }
            Self::Test => {
                format!("cargo test --locked --package {package} --all-targets")
            }
            Self::DocTest => format!("cargo test --locked --package {package} --doc"),
        }
    }

    fn arguments(self, package: &str) -> Vec<String> {
        match self {
            Self::Clippy => [
                "clippy",
                "--locked",
                "--package",
                package,
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            Self::Test => ["test", "--locked", "--package", package, "--all-targets"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Self::DocTest => ["test", "--locked", "--package", package, "--doc"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Command(step) => step.label,
            Self::EntryPoint => ENTRY_POINT_LABEL,
            Self::Format => FORMAT_LABEL,
            Self::Browser => BROWSER_LABEL,
            Self::WorkspaceTests => WORKSPACE_TEST_LABEL,
            Self::Battery => BATTERY_LABEL,
            Self::Landing => landing::LABEL,
        }
    }

    const fn family(self) -> &'static str {
        match self {
            Self::EntryPoint => "in-process:entry-point-audit",
            Self::Format => "child-process:rustfmt",
            Self::Command(_) => "child-process:cargo",
            Self::Browser => "child-process:npm",
            Self::WorkspaceTests => "child-process:rust-test-coordinator",
            Self::Battery => "in-process:gate-battery",
            Self::Landing => "in-process:contract-landing",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    success: bool,
    code: Option<i32>,
}

#[derive(Debug)]
enum CommandRun {
    Exited(ProcessOutput),
    Cancelled(ProcessOutput),
    TimedOut(ProcessOutput),
}

#[derive(Debug)]
struct ProcessOutput {
    status: Status,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    usage: Option<sill::ProcessUsage>,
}

impl ProcessOutput {
    #[cfg(any(test, not(target_arch = "wasm32")))]
    fn status_only(status: Status) -> Self {
        Self {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
            usage: Some(sill::ProcessUsage::default()),
        }
    }
}

#[derive(Debug)]
enum RunError {
    Launch(io::Error),
    Cleanup(String),
}

type ActionReadiness = Arc<dyn Fn() + Send + Sync>;
type ActionProgress = Arc<dyn Fn() + Send + Sync>;

trait Runner: Sync {
    fn entry_point(&self) -> Result<String, String>;
    fn format(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
    fn format_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        let _ = progress;
        self.format(cancellation, cargo_jobs)
    }
    fn run(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
    fn run_with_progress(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        let _ = progress;
        self.run(step, cancellation, cargo_jobs)
    }
    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError>;
    fn run_with_readiness_and_progress(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: ActionReadiness,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        let _ = progress;
        self.run_with_readiness(step, cancellation, cargo_jobs, readiness)
    }
    fn run_scoped(
        &self,
        step: ScopedCommand,
        package: &str,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
    fn browser(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<BrowserRun, RunError>;
    fn browser_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<BrowserRun, RunError> {
        let _ = progress;
        self.browser(cancellation, cargo_jobs)
    }
    fn workspace_test_prebuild(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
    fn workspace_test_prebuild_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        let _ = progress;
        self.workspace_test_prebuild(cancellation, cargo_jobs)
    }
    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
    ) -> Result<CommandRun, RunError>;
    fn workspace_tests_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        let _ = progress;
        self.workspace_tests(cancellation, cargo_jobs, workspace_workers)
    }
    fn structural_preflight(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError>;
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError>;
    fn accept_battery_state(&self) -> Result<(), String>;
    fn publish_full_verifier_evidence(&self) -> Result<(), String>;
    fn landing(&self) -> Result<landing::Report, landing::Error>;
}

/// The sole human-verifier rendering selected from one typed battery report.
///
/// Keeping the alternatives disjoint prevents a refusal from retaining raw
/// report bytes that a later write could accidentally expose.
#[cfg_attr(
    target_arch = "wasm32",
    expect(
        dead_code,
        reason = "native battery acquisition is explicitly unavailable on wasm32"
    )
)]
enum BatteryRun {
    Admitted(Vec<u8>),
    Refused(Vec<u8>),
    Error(String),
    Cancelled,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(serde::Serialize)]
struct BatteryAdmission<'a> {
    version: u64,
    source: &'a wire::Digest,
    report: wire::Digest,
    report_bytes: u64,
    arms: usize,
}

#[cfg(not(target_arch = "wasm32"))]
fn encode_battery_admission(report: &gate::BatteryReport) -> Result<Vec<u8>, String> {
    let canonical = report.encode().map_err(|error| error.to_string())?;
    let report_bytes = u64::try_from(canonical.len()).map_err(|error| error.to_string())?;
    wire::encode(&BatteryAdmission {
        version: report.version(),
        source: report.context().source(),
        report: wire::Digest::compute(wire::Domain::GateBatteryReport, &canonical),
        report_bytes,
        arms: report.results().len(),
    })
    .map_err(|error| error.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
struct SystemState {
    battery: Option<spice::gate::PreparedBattery<'static>>,
    announced_source: Option<wire::Digest>,
    compiler_contracts: Vec<Arc<gate::PreparedVerifierCompiler>>,
    compiler_processes: Vec<gate::VerifierCompilerProcessEvidence>,
    native_workspace: Option<gate::RustAnalysisWorkspace>,
    cargo_jobs: Option<usize>,
    full_verifier_battery_receipt: Option<gate::BatteryReceipt>,
    full_verifier_evidence: Option<spice::gate::FullVerifierEvidence>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SystemRunner {
    state: Mutex<SystemState>,
    source_cancellation: CancellationToken,
}

#[cfg(not(target_arch = "wasm32"))]
impl SystemRunner {
    fn prepare(source_cancellation: CancellationToken) -> Result<Self, String> {
        let battery = spice::gate::prepare_strict(gate::Selector::staged())
            .map_err(|error| error.to_string())?;
        let announced_source = battery.source().clone();
        Ok(Self::from_state(
            SystemState {
                battery: Some(battery),
                announced_source: Some(announced_source),
                compiler_contracts: Vec::new(),
                compiler_processes: Vec::new(),
                native_workspace: None,
                cargo_jobs: None,
                full_verifier_battery_receipt: None,
                full_verifier_evidence: None,
            },
            source_cancellation,
        ))
    }

    fn standalone() -> Self {
        Self::from_state(
            SystemState {
                battery: None,
                announced_source: None,
                compiler_contracts: Vec::new(),
                compiler_processes: Vec::new(),
                native_workspace: None,
                cargo_jobs: None,
                full_verifier_battery_receipt: None,
                full_verifier_evidence: None,
            },
            CancellationToken::new(),
        )
    }

    fn from_state(state: SystemState, source_cancellation: CancellationToken) -> Self {
        Self {
            state: Mutex::new(state),
            source_cancellation,
        }
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, SystemState>, RunError> {
        self.state
            .lock()
            .map_err(|error| RunError::Cleanup(format!("verifier state lock poisoned: {error}")))
    }

    fn compiler_stage(step: Step) -> Option<gate::VerifierCompilerStage> {
        match step.label {
            NATIVE_CLIPPY_LABEL => Some(gate::VerifierCompilerStage::NativeClippy),
            CANVAS_CHECK_LABEL => Some(gate::VerifierCompilerStage::CanvasCheck),
            CANVAS_APP_LABEL => Some(gate::VerifierCompilerStage::CanvasApp),
            _ => None,
        }
    }

    fn announced_source(&self) -> Result<wire::Digest, String> {
        self.state
            .lock()
            .map_err(|error| format!("verifier state lock poisoned: {error}"))?
            .announced_source
            .clone()
            .ok_or_else(|| "full verifier has no announced staged source".to_owned())
    }

    fn verify_announced_capture(&self) -> Result<(), String> {
        let state = self.state().map_err(|error| match error {
            RunError::Launch(error) => error.to_string(),
            RunError::Cleanup(detail) => detail,
        })?;
        let announced = state
            .announced_source
            .as_ref()
            .ok_or_else(|| "full verifier has no announced staged source".to_owned())?;
        let prepared = state
            .battery
            .as_ref()
            .ok_or_else(|| "full verifier battery capture is unavailable".to_owned())?;
        if prepared.source() != announced {
            return Err(format!(
                "full verifier prepared source {} differs from announced staged source {announced}",
                prepared.source(),
            ));
        }
        prepared
            .verify_source()
            .map_err(|error| format!("announced staged source {announced} moved: {error}"))
    }

    fn verify_announced_source(&self) -> Result<(), String> {
        self.verify_announced_capture()?;
        let status = verify_execution::run_process(
            STAGED_SOURCE_STATUS_LABEL,
            "git",
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ],
            &self.source_cancellation,
            1,
            &[],
        )
        .map_err(|error| match error {
            RunError::Launch(error) => {
                format!("inspect announced staged source worktree: {error}")
            }
            RunError::Cleanup(detail) => detail,
        })?;
        let status = match status {
            CommandRun::Exited(output) if output.status.success => output.stdout,
            CommandRun::Exited(output) => {
                return Err(format!(
                    "inspect announced staged source worktree exited with {:?}: {}",
                    output.status.code,
                    String::from_utf8_lossy(&output.stderr).trim_end(),
                ));
            }
            CommandRun::Cancelled(_) => {
                return Err("inspect announced staged source worktree was cancelled".to_owned());
            }
            CommandRun::TimedOut(_) => {
                return Err("inspect announced staged source worktree timed out".to_owned());
            }
        };
        admit_staged_worktree_status(&status)?;
        self.verify_announced_capture()
    }

    fn compiler_contract(
        &self,
        stage: gate::VerifierCompilerStage,
        cargo_jobs: usize,
    ) -> Result<Arc<gate::PreparedVerifierCompiler>, RunError> {
        let gate_id = match stage {
            gate::VerifierCompilerStage::NativeClippy => gate::GateId::RustTypecheck,
            gate::VerifierCompilerStage::CanvasCheck | gate::VerifierCompilerStage::CanvasApp => {
                gate::GateId::CanvasWebTypecheck
            }
        };
        let mut state = self.state()?;
        match state.cargo_jobs {
            Some(current) if current != cargo_jobs.max(1) => {
                return Err(RunError::Cleanup(format!(
                    "full verifier Cargo job cap changed from {current} to {}",
                    cargo_jobs.max(1),
                )));
            }
            Some(_) => {}
            None => state.cargo_jobs = Some(cargo_jobs.max(1)),
        }
        if !state
            .compiler_contracts
            .iter()
            .any(|contract| contract.gate_id() == gate_id)
        {
            let battery = state.battery.as_mut().ok_or_else(|| {
                RunError::Cleanup("full verifier compiler evidence has no prepared battery".into())
            })?;
            let contract = battery
                .verifier_compiler_contract(gate_id, cargo_jobs)
                .map_err(|error| RunError::Launch(io::Error::other(error.to_string())))?
                .prepare_stable()
                .map_err(RunError::Launch)?;
            state.compiler_contracts.push(Arc::new(contract));
        }
        state
            .compiler_contracts
            .iter()
            .find(|contract| contract.gate_id() == gate_id)
            .map(Arc::clone)
            .ok_or_else(|| {
                RunError::Cleanup(format!("missing {} compiler contract", gate_id.as_str()))
            })
    }

    fn run_compiler_stage(
        &self,
        stage: gate::VerifierCompilerStage,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: Option<ActionReadiness>,
        progress: Option<ActionProgress>,
    ) -> Result<CommandRun, RunError> {
        let contract = self.compiler_contract(stage, cargo_jobs)?;
        let run = match (readiness, progress) {
            (Some(readiness), Some(progress)) => contract
                .run_supervised_with_readiness_and_progress(
                    stage,
                    cancellation.clone(),
                    readiness,
                    progress,
                ),
            (Some(readiness), None) => {
                contract.run_supervised_with_readiness(stage, cancellation.clone(), readiness)
            }
            (None, Some(progress)) => {
                contract.run_supervised_with_progress(stage, cancellation.clone(), progress)
            }
            (None, None) => contract.run_supervised(stage, cancellation.clone()),
        }
        .map_err(RunError::Launch)?;
        let cancelled = run.cancelled();
        let timed_out = run.timed_out();
        let reusable_workspace = (stage == gate::VerifierCompilerStage::NativeClippy
            && run.success())
        .then(|| contract.reusable_workspace())
        .flatten();
        let output = ProcessOutput {
            status: Status {
                success: run.success(),
                code: run.code(),
            },
            stdout: run.stdout().to_vec(),
            stderr: run.stderr().to_vec(),
            usage: None,
        };
        let mut state = self.state()?;
        if let Some(evidence) = run.into_evidence() {
            state.compiler_processes.push(evidence);
        }
        if let Some(workspace) = reusable_workspace {
            state.native_workspace = Some(workspace);
        }
        Ok(if cancelled {
            CommandRun::Cancelled(output)
        } else if timed_out {
            CommandRun::TimedOut(output)
        } else {
            CommandRun::Exited(output)
        })
    }

    fn run_workspace_process(
        &self,
        label: &'static str,
        arguments: impl IntoIterator<Item = String>,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: Option<ActionProgress>,
        progress_source: verify_execution::ProgressSource,
    ) -> Result<CommandRun, RunError> {
        let executable = std::env::current_exe().map_err(RunError::Launch)?;
        let workspace = { self.state()?.native_workspace.clone() };
        let workspace = workspace.ok_or_else(|| {
            RunError::Cleanup(format!(
                "full verifier {label} has no reusable native compiler workspace"
            ))
        })?;
        let overrides = [
            ("CARGO_TARGET_DIR", workspace.cargo_target().as_os_str()),
            ("CARGO_BUILD_BUILD_DIR", workspace.cargo_build().as_os_str()),
        ];
        match progress {
            Some(progress) => verify_execution::run_process_at_with_progress(
                label,
                executable,
                arguments,
                workspace.source(),
                cancellation,
                cargo_jobs,
                &overrides,
                progress_source,
                progress,
            ),
            None => verify_execution::run_process_at(
                label,
                executable,
                arguments,
                workspace.source(),
                cancellation,
                cargo_jobs,
                &overrides,
            ),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn admit_staged_worktree_status(status: &[u8]) -> Result<(), String> {
    let mut records = status
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 3 || record[2] != b' ' {
            return Err("Git returned a malformed staged/worktree status inventory".to_owned());
        }
        let code = &record[..2];
        let path = String::from_utf8_lossy(&record[3..]);
        if code == b"??" {
            return Err(format!(
                "untracked worktree path {path:?} is absent from the announced staged source",
            ));
        }
        if code[1] != b' ' {
            return Err(format!(
                "worktree path {path:?} differs from the announced staged index ({})",
                String::from_utf8_lossy(code),
            ));
        }
        if matches!(code[0], b'R' | b'C') && records.next().is_none() {
            return Err("Git omitted the original path from a staged rename or copy".to_owned());
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
struct SystemRunner;

#[cfg(target_arch = "wasm32")]
impl SystemRunner {
    fn prepare(_source_cancellation: CancellationToken) -> Result<Self, String> {
        Ok(Self)
    }

    const fn standalone() -> Self {
        Self
    }

    fn announced_source(&self) -> Result<wire::Digest, String> {
        Err("announced staged source is unavailable on wasm32".to_owned())
    }

    fn verify_announced_source(&self) -> Result<(), String> {
        Err("announced staged source is unavailable on wasm32".to_owned())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn current_spice_binary() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve the running xtask executable: {error}"))?;
    let mut directory = executable
        .parent()
        .ok_or_else(|| format!("xtask executable has no parent: {}", executable.display()))?;
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory = directory.parent().ok_or_else(|| {
            format!(
                "xtask dependency executable has no target-profile parent: {}",
                executable.display()
            )
        })?;
    }
    Ok(directory.join(format!("spice{}", std::env::consts::EXE_SUFFIX)))
}

#[cfg(not(target_arch = "wasm32"))]
fn entry_point_with_policy(
    suppress_install_apply_guidance: bool,
    audit: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    if suppress_install_apply_guidance {
        Ok(String::new())
    } else {
        audit()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn audit_entry_point(repository: &spice::repository::Repository) -> Result<String, String> {
    let current_binary = current_spice_binary()?;
    let audit = spice::install::audit_appliance(repository, &current_binary)
        .map_err(|error| error.to_string())?;
    if !audit.is_current() {
        return Err(format!(
            "installed Spice appliance is {:?}: {}; repair={:?}",
            audit.state(),
            audit.detail(),
            audit.repair(),
        ));
    }
    Ok(format!(
        "installed Spice appliance is current: {}\n",
        audit.detail()
    ))
}

impl Runner for SystemRunner {
    fn entry_point(&self) -> Result<String, String> {
        #[cfg(target_arch = "wasm32")]
        {
            Err("installed-appliance audit is unavailable on wasm32".to_owned())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let repository = spice::repository::Repository::discover(Path::new("."))
                .map_err(|error| error.to_string())?;
            let configuration = spice::config::load_repository(&repository, &[])
                .map_err(|error| error.diagnostic())?;
            entry_point_with_policy(
                spice::config::suppress_install_apply_guidance(configuration.root()),
                || audit_entry_point(&repository),
            )
        }
    }

    fn format(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let root = Path::new(".");
            let sources = gate::formatter_sources(root)
                .map_err(io::Error::other)
                .map_err(RunError::Launch)?;
            if sources.is_empty() {
                return Ok(CommandRun::Exited(ProcessOutput::status_only(Status {
                    success: true,
                    code: Some(0),
                })));
            }
            let edition = gate::formatter_edition(root)
                .map_err(io::Error::other)
                .map_err(RunError::Launch)?;
            let mut arguments = [
                "--edition",
                edition.as_str(),
                "--check",
                "--files-with-diff",
            ]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
            arguments.extend(sources.into_iter().map(std::ffi::OsString::from));
            verify_execution::run_process(
                "rustfmt",
                "rustfmt",
                arguments,
                cancellation,
                cargo_jobs,
                &[],
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, cargo_jobs);
            Err(RunError::Launch(io::Error::other(
                "repository formatting is unavailable on wasm32",
            )))
        }
    }

    fn run(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(stage) = Self::compiler_stage(step) {
            return self.run_compiler_stage(stage, cancellation, cargo_jobs, None, None);
        }
        verify_execution::run_process(
            step.label,
            "cargo",
            step.arguments,
            cancellation,
            cargo_jobs,
            &[],
        )
    }

    fn run_with_progress(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(stage) = Self::compiler_stage(step) {
            return self.run_compiler_stage(stage, cancellation, cargo_jobs, None, Some(progress));
        }
        verify_execution::run_process_with_progress(
            step.label,
            "cargo",
            step.arguments,
            cancellation,
            cargo_jobs,
            &[],
            verify_execution::ProgressSource::CargoStderr,
            progress,
        )
    }

    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stage = Self::compiler_stage(step).ok_or_else(|| {
                RunError::Launch(io::Error::other(format!(
                    "compiler readiness is unavailable for {}",
                    step.label
                )))
            })?;
            self.run_compiler_stage(stage, cancellation, cargo_jobs, Some(readiness), None)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (step, cancellation, cargo_jobs, readiness);
            Err(RunError::Launch(io::Error::other(
                "compiler readiness is unavailable on wasm32",
            )))
        }
    }

    fn run_with_readiness_and_progress(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        readiness: ActionReadiness,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stage = Self::compiler_stage(step).ok_or_else(|| {
                RunError::Launch(io::Error::other(format!(
                    "compiler readiness is unavailable for {}",
                    step.label
                )))
            })?;
            self.run_compiler_stage(
                stage,
                cancellation,
                cargo_jobs,
                Some(readiness),
                Some(progress),
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (step, cancellation, cargo_jobs, readiness, progress);
            Err(RunError::Launch(io::Error::other(
                "compiler readiness is unavailable on wasm32",
            )))
        }
    }

    fn run_scoped(
        &self,
        step: ScopedCommand,
        package: &str,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let label = step.label(package);
        verify_execution::run_process(
            &label,
            "cargo",
            step.arguments(package),
            cancellation,
            cargo_jobs,
            &[],
        )
    }

    fn browser(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<BrowserRun, RunError> {
        verify_browser::run(cancellation, cargo_jobs)
    }

    fn browser_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<BrowserRun, RunError> {
        verify_browser::run_with_progress(cancellation, cargo_jobs, progress)
    }

    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.run_workspace_process(
                WORKSPACE_TEST_LABEL,
                [
                    "__workspace-tests-prebuilt-shared".to_owned(),
                    workspace_workers.to_string(),
                ],
                cancellation,
                cargo_jobs.min(workspace_workers),
                None,
                verify_execution::ProgressSource::WorkspaceStdout,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, cargo_jobs, workspace_workers);
            Err(RunError::Launch(io::Error::other(
                "workspace tests are unavailable on wasm32",
            )))
        }
    }

    fn workspace_tests_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.run_workspace_process(
                WORKSPACE_TEST_LABEL,
                [
                    "__workspace-tests-prebuilt-shared".to_owned(),
                    workspace_workers.to_string(),
                ],
                cancellation,
                cargo_jobs.min(workspace_workers),
                Some(progress),
                verify_execution::ProgressSource::WorkspaceStdout,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, cargo_jobs, workspace_workers, progress);
            Err(RunError::Launch(io::Error::other(
                "workspace tests are unavailable on wasm32",
            )))
        }
    }

    fn workspace_test_prebuild(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.run_workspace_process(
                WORKSPACE_TEST_PREBUILD_LABEL,
                ["__workspace-tests-prebuild-shared".to_owned()],
                cancellation,
                cargo_jobs,
                None,
                verify_execution::ProgressSource::CargoStderr,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, cargo_jobs);
            Err(RunError::Launch(io::Error::other(
                "workspace-test prebuild is unavailable on wasm32",
            )))
        }
    }

    fn workspace_test_prebuild_with_progress(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        progress: ActionProgress,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.run_workspace_process(
                WORKSPACE_TEST_PREBUILD_LABEL,
                ["__workspace-tests-prebuild-shared".to_owned()],
                cancellation,
                cargo_jobs,
                Some(progress),
                verify_execution::ProgressSource::CargoStderr,
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, cargo_jobs, progress);
            Err(RunError::Launch(io::Error::other(
                "workspace-test prebuild is unavailable on wasm32",
            )))
        }
    }

    fn structural_preflight(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if cancellation.is_cancelled() {
                return Ok(BatteryRun::Cancelled);
            }
            let reports = self
                .state()?
                .battery
                .as_mut()
                .ok_or_else(|| {
                    RunError::Launch(io::Error::other(
                        "full verifier battery capture is unavailable",
                    ))
                })?
                .execute_structural_preflight()
                .map_err(|error| RunError::Launch(io::Error::other(error.to_string())))?;
            if cancellation.is_cancelled() {
                return Ok(BatteryRun::Cancelled);
            }
            if reports.iter().all(gate::FocusedArmReport::admits) {
                Ok(BatteryRun::Admitted(Vec::new()))
            } else {
                Ok(BatteryRun::Refused(
                    gate::BatteryRefusalFold::from_focused_reports(&reports).into_bytes(),
                ))
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cancellation;
            Err(RunError::Launch(io::Error::other(
                "the structural gate preflight is unavailable on wasm32",
            )))
        }
    }

    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if cancellation.is_cancelled() {
                return Ok(BatteryRun::Cancelled);
            }
            let mut state = self.state()?;
            state.full_verifier_battery_receipt = None;
            state.full_verifier_evidence = None;
            let evidence = match state
                .compiler_contracts
                .iter()
                .map(|contract| contract.complete_retained(&state.compiler_processes))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(evidence) => evidence,
                Err(error) => return Ok(BatteryRun::Error(error)),
            };
            let cargo_jobs = state.cargo_jobs.unwrap_or(1);
            let receipt = match state
                .battery
                .as_mut()
                .ok_or_else(|| {
                    RunError::Launch(io::Error::other(
                        "full verifier battery capture is unavailable",
                    ))
                })?
                .execute_prepared_compiler_receipt(&evidence, cargo_jobs)
            {
                Ok(receipt) => receipt,
                Err(error) => return Ok(BatteryRun::Error(error.to_string())),
            };
            if cancellation.is_cancelled() {
                Ok(BatteryRun::Cancelled)
            } else if receipt.run().admits() {
                let admission = match encode_battery_admission(receipt.run()) {
                    Ok(document) => document,
                    Err(error) => return Ok(BatteryRun::Error(error)),
                };
                state.full_verifier_battery_receipt = Some(receipt);
                Ok(BatteryRun::Admitted(admission))
            } else {
                Ok(BatteryRun::Refused(
                    receipt.run().refusal_fold().into_bytes(),
                ))
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cancellation;
            Err(RunError::Launch(io::Error::other(
                "the canonical native Rust gate battery is unavailable on wasm32",
            )))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn accept_battery_state(&self) -> Result<(), String> {
        self.verify_announced_source()?;
        let mut state = self.state().map_err(|error| match error {
            RunError::Launch(error) => error.to_string(),
            RunError::Cleanup(detail) => detail,
        })?;
        let receipt = state
            .full_verifier_battery_receipt
            .as_ref()
            .ok_or_else(|| "full verifier completed without a battery receipt".to_owned())?
            .clone();
        let evidence = state
            .compiler_contracts
            .iter()
            .map(|contract| contract.complete(&state.compiler_processes))
            .collect::<Result<Vec<_>, _>>()?;
        let repository = spice::repository::Repository::discover(Path::new("."))
            .map_err(|error| error.to_string())?;
        let announced_source = state
            .announced_source
            .as_ref()
            .ok_or_else(|| "full verifier has no announced staged source".to_owned())?;
        let artifact = spice::gate::FullVerifierEvidence::mint(
            &repository,
            state.cargo_jobs.unwrap_or(1),
            announced_source,
            &receipt,
            &evidence,
            state
                .battery
                .as_ref()
                .ok_or_else(|| "full verifier battery capture is unavailable".to_owned())?,
        )?;
        state.full_verifier_battery_receipt = None;
        state.full_verifier_evidence = Some(artifact);
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        self.verify_announced_source()?;
        let evidence = {
            let mut state = self.state().map_err(|error| match error {
                RunError::Launch(error) => error.to_string(),
                RunError::Cleanup(detail) => detail,
            })?;
            let evidence = state
                .full_verifier_evidence
                .clone()
                .ok_or_else(|| "full verifier completed without reusable evidence".to_owned())?;
            for contract in &mut state.compiler_contracts {
                Arc::get_mut(contract)
                    .ok_or_else(|| {
                        "full verifier compiler source still has a live process owner".to_owned()
                    })?
                    .release()
                    .map_err(|error| {
                        format!(
                            "full verifier compiler source could not return to its idle state: \
                             {error}"
                        )
                    })?;
            }
            evidence
        };
        let repository = spice::repository::Repository::discover(Path::new("."))
            .map_err(|error| error.to_string())?;
        evidence.publish(&repository)
    }

    #[cfg(target_arch = "wasm32")]
    fn accept_battery_state(&self) -> Result<(), String> {
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        Ok(())
    }

    fn landing(&self) -> Result<landing::Report, landing::Error> {
        landing::inspect()
    }
}

/// Run the complete repository verification graph with bounded concurrency.
///
/// # Errors
///
/// Returns the deterministic primary failure after cancelling and reaping any
/// running siblings. No action starts before all declared prerequisites admit.
pub fn verify(output: &mut impl Write) -> Result<(), VerifyError> {
    verify_execution::verify_supervised(output)
}

/// Run the inner full-verifier process owned by [`verify`].
///
/// This is public only so the native command binary can expose its unadvertised
/// child-process entry point. Callers use [`verify`], which owns the hard outer
/// process boundary.
///
/// # Errors
///
/// Returns the deterministic inner verifier failure after cancelling and
/// reaping its running action groups.
#[doc(hidden)]
pub fn verify_child(output: &mut impl Write) -> Result<(), VerifyError> {
    verify_execution::verify_system(output)
}

/// Run lint, test, and documentation feedback for one Cargo package.
///
/// This is deliberately not the completion battery. Its first output records
/// every full verifier action and canonical battery arm it does not execute.
/// The policy battery remains repository-wide, so executing its arms here would
/// make an unrelated baseline refusal package-local in name only. Callers must
/// still run [`verify`] before completing a task.
///
/// # Errors
///
/// Returns an invalid package name or the first output, launch, or command
/// failure.
pub fn verify_package(package: &str, output: &mut impl Write) -> Result<(), VerifyError> {
    verify_execution::verify_package_system(package, output)
}

#[cfg(test)]
mod tests;
