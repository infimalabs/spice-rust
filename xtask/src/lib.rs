//! Repository-local build, verification, and release commands.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Write};
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

use sill::CancellationToken;

mod app;
mod build_cache;
mod commit_message;
mod develop;
mod hooks;
mod landing;
mod proof_image;
mod reference_transaction;
mod release;
mod verify_browser;
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
pub use commit_message::{CommitMessageError, WRAP_LIMIT as COMMIT_MESSAGE_WRAP_LIMIT};
pub use develop::{RefreshError, RefreshReport, RefreshRequest, refresh};
use verify_browser::BrowserRun;
#[cfg(test)]
const BROWSER_RECEIPT: &[u8] = verify_browser::RECEIPT;
// The install step drives `spice::install`, which the appliance itself gates
// off the wasm target; the workspace wasm check compiles this crate too.
#[cfg(not(target_arch = "wasm32"))]
pub use develop::refresh_install;
pub use hooks::{
    HookActivationError, activate_hooks, audit_entry_point, audit_entry_point_for_guard,
    compiled_reference_transaction_guard_digest, reference_transaction_guard_digest,
};
pub use proof_image::{ProofImageError, build_proof_image, load_proof_image};
pub use reference_transaction::{ReferenceTransactionError, WorktreeDamage};
pub use release::{CommandRecord, ReleaseEnvironment, TargetBuild, TargetBuildError, stage_target};
#[cfg(not(target_arch = "wasm32"))]
pub use workspace_tests::{WorkspaceTestError, run as run_workspace_tests};

/// Apply the repository's native commit-message law to one Git message file.
///
/// The boolean answer says whether long body prose was folded in place.
///
/// # Errors
///
/// Returns an I/O failure or the first commit-message policy refusal.
pub fn commit_message(
    path: &Path,
    policy: &gate::CommitMessagePolicy,
) -> Result<bool, CommitMessageError> {
    commit_message::apply(path, policy)
}

/// Guard a Git reference transaction from abandoning upstream-merged commits.
///
/// States other than `prepared` are intentionally inert, as required by Git's
/// reference-transaction hook protocol. A rewind whose declared upstream
/// resolves to no commit is refused rather than permitted: a guard that could
/// not read the upstream has cleared nothing, and passing it would report what
/// it reports after checking one.
///
/// # Errors
///
/// Returns malformed hook input, Git execution failure, a protected-history
/// refusal, or an upstream this guard could not resolve.
pub fn reference_transaction(
    repository: &Path,
    state: &OsStr,
    input: &str,
) -> Result<(), ReferenceTransactionError> {
    reference_transaction::apply(repository, state, input)
}

/// Every verification action, in stable report order.
///
/// `gate/src/battery/manifest.toml` names all 25 retired standard policy checks
/// and binds each to one compiled Rust arm; its one additional C# arm is marked
/// native by carrying no retired-step name. The workspace tests exercise that
/// exact manifest and the production executor for all four families, including
/// the structural family that once lacked acquisition. The real-browser action
/// is the one non-Cargo runtime check and requires its end-of-run receipt.
/// Contract landing remains terminal: no alternate executable discovered from
/// `PATH` repeats policy after the Rust-owned checks finish. Execution order is
/// constrained by the explicit graph in `verify_execution`, not this report
/// order.
const VERIFY_ACTIONS: [Action; 12] = [
    Action::EntryPoint,
    Action::Format,
    Action::Command(Step::cargo(
        "cargo clippy --locked --workspace --all-targets -- -D warnings",
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
        "cargo check --locked --workspace --target wasm32-unknown-unknown",
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
/// Precondition rather than a check of this tree, which is why it runs first.
///
/// Every other action asks whether the tree is correct. This one asks whether
/// the guard Git is enforcing right now is the one this tree describes, and a
/// battery that passes while eleven worktrees enforce an older guard is green
/// about the wrong repository. It costs one file read and no build.
const ENTRY_POINT_LABEL: &str = "published reference-transaction guard is this build's";
/// Real-browser guard for the production Mosaic engine edge.
///
/// BROWSER-1kKSWhZk removed one of the two former surfaces. This mandatory
/// action protects the remaining `composer -> mosaic_plane -> mosaic` path.
const BROWSER_LABEL: &str = "npm run test:canvas (BROWSER-1kKSWhZk guard)";
/// Exit spelling for a completed verifier action that refused publication.
pub const VERIFY_ACTION_REFUSAL_EXIT: u8 = 3;
const CANVAS_APP_LABEL: &str = "cargo xtask app (canvas compiler evidence)";
const WORKSPACE_TEST_CONCURRENCY_CAP: usize = 4;
const WORKSPACE_TEST_LABEL: &str =
    "deterministic sharded cargo test --locked --workspace --all-targets";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Step {
    label: &'static str,
    arguments: &'static [&'static str],
}

impl Step {
    const fn cargo(label: &'static str, arguments: &'static [&'static str]) -> Self {
        Self { label, arguments }
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
}

impl ProcessOutput {
    #[cfg(any(test, not(target_arch = "wasm32")))]
    fn status_only(status: Status) -> Self {
        Self {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

#[derive(Debug)]
enum RunError {
    Launch(io::Error),
    Cleanup(String),
}

trait Runner: Sync {
    fn entry_point(&self) -> Result<String, String>;
    fn format(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
    fn run(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
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
    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
        workspace_workers: usize,
    ) -> Result<CommandRun, RunError> {
        self.run(
            Step::cargo(WORKSPACE_TEST_LABEL, &[]),
            cancellation,
            cargo_jobs.min(workspace_workers),
        )
    }
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError>;
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
struct SystemState {
    battery: Option<spice::gate::PreparedBattery>,
    compiler_contracts: Vec<gate::VerifierCompilerContract>,
    compiler_processes: Vec<gate::VerifierCompilerProcessEvidence>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SystemRunner {
    state: Mutex<SystemState>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SystemRunner {
    fn prepare() -> Result<Self, String> {
        let battery =
            spice::gate::prepare(gate::Selector::staged()).map_err(|error| error.to_string())?;
        Ok(Self::from_state(SystemState {
            battery: Some(battery),
            compiler_contracts: Vec::new(),
            compiler_processes: Vec::new(),
        }))
    }

    fn standalone() -> Self {
        Self::from_state(SystemState {
            battery: None,
            compiler_contracts: Vec::new(),
            compiler_processes: Vec::new(),
        })
    }

    fn from_state(state: SystemState) -> Self {
        Self {
            state: Mutex::new(state),
        }
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, SystemState>, RunError> {
        self.state
            .lock()
            .map_err(|error| RunError::Cleanup(format!("verifier state lock poisoned: {error}")))
    }

    fn compiler_stage(step: Step) -> Option<gate::VerifierCompilerStage> {
        match step.label {
            "cargo clippy --locked --workspace --all-targets -- -D warnings" => {
                Some(gate::VerifierCompilerStage::NativeClippy)
            }
            "cargo check --locked --workspace --target wasm32-unknown-unknown" => {
                Some(gate::VerifierCompilerStage::CanvasCheck)
            }
            CANVAS_APP_LABEL => Some(gate::VerifierCompilerStage::CanvasApp),
            _ => None,
        }
    }

    fn compiler_contract(
        &self,
        stage: gate::VerifierCompilerStage,
        cargo_jobs: usize,
    ) -> Result<gate::VerifierCompilerContract, RunError> {
        let gate_id = match stage {
            gate::VerifierCompilerStage::NativeClippy => gate::GateId::RustTypecheck,
            gate::VerifierCompilerStage::CanvasCheck | gate::VerifierCompilerStage::CanvasApp => {
                gate::GateId::CanvasWebTypecheck
            }
        };
        let mut state = self.state()?;
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
                .map_err(|error| RunError::Launch(io::Error::other(error.to_string())))?;
            state.compiler_contracts.push(contract);
        }
        state
            .compiler_contracts
            .iter()
            .find(|contract| contract.gate_id() == gate_id)
            .cloned()
            .ok_or_else(|| {
                RunError::Cleanup(format!("missing {} compiler contract", gate_id.as_str()))
            })
    }

    fn run_compiler_stage(
        &self,
        stage: gate::VerifierCompilerStage,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let contract = self.compiler_contract(stage, cargo_jobs)?;
        let run = contract
            .run_supervised(stage, cancellation.clone())
            .map_err(RunError::Launch)?;
        let cancelled = run.cancelled();
        let timed_out = run.timed_out();
        let output = ProcessOutput {
            status: Status {
                success: run.success(),
                code: run.code(),
            },
            stdout: run.stdout().to_vec(),
            stderr: run.stderr().to_vec(),
        };
        if let Some(evidence) = run.into_evidence() {
            self.state()?.compiler_processes.push(evidence);
        }
        Ok(if cancelled {
            CommandRun::Cancelled(output)
        } else if timed_out {
            CommandRun::TimedOut(output)
        } else {
            CommandRun::Exited(output)
        })
    }
}

#[cfg(target_arch = "wasm32")]
struct SystemRunner;

#[cfg(target_arch = "wasm32")]
impl SystemRunner {
    fn prepare() -> Result<Self, String> {
        Ok(Self)
    }

    const fn standalone() -> Self {
        Self
    }
}

impl Runner for SystemRunner {
    fn entry_point(&self) -> Result<String, String> {
        let mut report = Vec::new();
        hooks::audit_entry_point(Path::new("."), &mut report).map_err(|error| error.to_string())?;
        String::from_utf8(report).map_err(|error| error.to_string())
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
            return self.run_compiler_stage(stage, cancellation, cargo_jobs);
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

    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        _cargo_jobs: usize,
        workspace_workers: usize,
    ) -> Result<CommandRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let executable = std::env::current_exe().map_err(RunError::Launch)?;
            verify_execution::run_process(
                WORKSPACE_TEST_LABEL,
                executable,
                [
                    "__workspace-tests".to_owned(),
                    workspace_workers.to_string(),
                ],
                cancellation,
                1,
                &[],
            )
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cancellation, workspace_workers);
            Err(RunError::Launch(io::Error::other(
                "workspace tests are unavailable on wasm32",
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
            let evidence = match state
                .compiler_contracts
                .iter()
                .map(|contract| contract.complete(&state.compiler_processes))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(evidence) => evidence,
                Err(error) => return Ok(BatteryRun::Error(error)),
            };
            let report = match state
                .battery
                .as_mut()
                .ok_or_else(|| {
                    RunError::Launch(io::Error::other(
                        "full verifier battery capture is unavailable",
                    ))
                })?
                .execute(&evidence)
            {
                Ok(report) => report,
                Err(error) => return Ok(BatteryRun::Error(error.to_string())),
            };
            if cancellation.is_cancelled() {
                Ok(BatteryRun::Cancelled)
            } else if report.admits() {
                match report.encode() {
                    Ok(document) => Ok(BatteryRun::Admitted(document)),
                    Err(error) => Ok(BatteryRun::Error(error.to_string())),
                }
            } else {
                Ok(BatteryRun::Refused(report.refusal_fold().into_bytes()))
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

    fn landing(&self) -> Result<landing::Report, landing::Error> {
        landing::inspect()
    }
}

/// Execute the public partial gate-battery action.
///
/// This action owns a fresh capture and deliberately does not share the full
/// verifier's retained compiler evidence. The boolean is true for admission
/// and false for a policy refusal; the CLI owns their distinct exit codes.
///
/// # Errors
///
/// Returns gate acquisition, report encoding, or output failures.
pub fn verify_battery_child(output: &mut impl Write) -> Result<bool, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let report =
            spice::gate::run(gate::Selector::staged()).map_err(|error| error.to_string())?;
        if report.admits() {
            let document = report.encode().map_err(|error| error.to_string())?;
            output
                .write_all(&document)
                .map_err(|error| error.to_string())?;
            Ok(true)
        } else {
            output
                .write_all(report.refusal_fold().as_bytes())
                .map_err(|error| error.to_string())?;
            Ok(false)
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = output;
        Err("the canonical native Rust gate battery is unavailable on wasm32".to_owned())
    }
}

/// Run one canonical gate arm against the staged worktree and emit its JSON row.
///
/// # Errors
///
/// Returns acquisition, encoding, output, or non-admitting arm failures.
pub fn gate_arm(gate_id: gate::GateId, output: &mut impl Write) -> Result<(), GateArmError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let result =
            spice::gate::run_arm(gate::Selector::staged(), gate_id).map_err(GateArmError::Run)?;
        serde_json::to_writer(&mut *output, &result).map_err(GateArmError::Encode)?;
        writeln!(output).map_err(GateArmError::Output)?;
        if !result.state().admits() {
            return Err(GateArmError::Refused(gate_id));
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (gate_id, output);
        Err(GateArmError::Unavailable)
    }
}

/// Run the complete repository verification graph with bounded concurrency.
///
/// # Errors
///
/// Returns the deterministic primary failure after cancelling and reaping any
/// running siblings. No action starts before all declared prerequisites admit.
pub fn verify(output: &mut impl Write) -> Result<(), VerifyError> {
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

/// One selected gate-arm command failure.
#[derive(Debug)]
pub enum GateArmError {
    #[cfg(not(target_arch = "wasm32"))]
    Run(spice::gate::RunError),
    Encode(serde_json::Error),
    Output(io::Error),
    Refused(gate::GateId),
    Unavailable,
}

impl fmt::Display for GateArmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Run(source) => write!(formatter, "[gate] acquisition failed: {source}"),
            Self::Encode(source) => write!(formatter, "[gate] result encoding failed: {source}"),
            Self::Output(source) => write!(formatter, "[gate] output failed: {source}"),
            Self::Refused(gate_id) => {
                write!(formatter, "[gate] {gate_id} refused publication")
            }
            Self::Unavailable => formatter.write_str("[gate] unavailable on wasm32"),
        }
    }
}

impl std::error::Error for GateArmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Run(source) => Some(source),
            Self::Encode(source) => Some(source),
            Self::Output(source) => Some(source),
            Self::Refused(_) | Self::Unavailable => None,
        }
    }
}

fn valid_package_name(package: &str) -> bool {
    !package.is_empty()
        && package
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A bounded verifier failure that identifies the first incomplete step.
#[derive(Debug)]
pub enum VerifyError {
    Output(io::Error),
    Encode(serde_json::Error),
    Package(String),
    EntryPoint(String),
    Landing(String),
    LandingRefused,
    Battery(String),
    BatteryRefused,
    BrowserLaunch {
        source: io::Error,
    },
    BrowserExit {
        code: Option<i32>,
    },
    BrowserIncomplete,
    Launch {
        label: &'static str,
        source: io::Error,
    },
    ScopedLaunch {
        label: String,
        source: io::Error,
    },
    Exit {
        label: &'static str,
        code: Option<i32>,
    },
    ScopedExit {
        label: String,
        code: Option<i32>,
    },
    Cleanup {
        label: String,
        detail: String,
    },
    Cancelled {
        label: String,
    },
    TimedOut {
        label: String,
    },
    WorkerPanic {
        label: &'static str,
        detail: String,
    },
    InterruptInstall(io::Error),
    Interrupted,
}

impl From<io::Error> for VerifyError {
    fn from(source: io::Error) -> Self {
        Self::Output(source)
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(source) => write!(formatter, "[verify] output failed: {source}"),
            Self::Encode(source) => write!(formatter, "[verify] landing output failed: {source}"),
            Self::Package(package) => write!(
                formatter,
                "[verify] invalid package scope {package:?}: use one Cargo package name"
            ),
            Self::EntryPoint(detail) => write!(formatter, "[verify] {detail}"),
            Self::Landing(detail) => {
                write!(formatter, "[verify] landing acquisition failed: {detail}")
            }
            Self::LandingRefused => formatter.write_str("[verify] contract landing refused"),
            Self::Battery(detail) => write!(formatter, "[verify] battery failed: {detail}"),
            Self::BatteryRefused => formatter.write_str("[verify] battery refused publication"),
            Self::BrowserLaunch { source } => {
                write!(
                    formatter,
                    "[verify] failed to launch: {BROWSER_LABEL} ({source})"
                )
            }
            Self::BrowserExit { code: Some(code) } => {
                write!(formatter, "[verify] failed: {BROWSER_LABEL} (exit {code})")
            }
            Self::BrowserExit { code: None } => {
                write!(
                    formatter,
                    "[verify] failed: {BROWSER_LABEL} (terminated by signal)"
                )
            }
            Self::BrowserIncomplete => write!(
                formatter,
                "[verify] browser coverage did not complete: {BROWSER_LABEL} emitted no receipt"
            ),
            Self::Launch { label, source } => {
                write!(formatter, "[verify] failed to launch: {label} ({source})")
            }
            Self::ScopedLaunch { label, source } => {
                write!(formatter, "[verify] failed to launch: {label} ({source})")
            }
            Self::Exit {
                label,
                code: Some(code),
            } => write!(formatter, "[verify] failed: {label} (exit {code})"),
            Self::Exit { label, code: None } => {
                write!(formatter, "[verify] failed: {label} (terminated by signal)")
            }
            Self::ScopedExit {
                label,
                code: Some(code),
            } => write!(formatter, "[verify] failed: {label} (exit {code})"),
            Self::ScopedExit { label, code: None } => {
                write!(formatter, "[verify] failed: {label} (terminated by signal)")
            }
            Self::Cleanup { label, detail } => {
                write!(formatter, "[verify] cleanup failed: {label} ({detail})")
            }
            Self::Cancelled { label } => write!(formatter, "[verify] cancelled: {label}"),
            Self::TimedOut { label } => write!(formatter, "[verify] timed out: {label}"),
            Self::WorkerPanic { label, detail } => {
                write!(formatter, "[verify] action panicked: {label} ({detail})")
            }
            Self::InterruptInstall(source) => {
                write!(
                    formatter,
                    "[verify] failed to install interrupt handling: {source}"
                )
            }
            Self::Interrupted => formatter.write_str("[verify] interrupted by operator"),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Output(source)
            | Self::Launch { source, .. }
            | Self::ScopedLaunch { source, .. }
            | Self::BrowserLaunch { source }
            | Self::InterruptInstall(source) => Some(source),
            Self::Encode(source) => Some(source),
            Self::Package(_)
            | Self::EntryPoint(_)
            | Self::Landing(_)
            | Self::LandingRefused
            | Self::Battery(_)
            | Self::BatteryRefused
            | Self::BrowserExit { .. }
            | Self::BrowserIncomplete
            | Self::Exit { .. }
            | Self::ScopedExit { .. }
            | Self::Cleanup { .. }
            | Self::Cancelled { .. }
            | Self::TimedOut { .. }
            | Self::WorkerPanic { .. }
            | Self::Interrupted => None,
        }
    }
}

#[cfg(test)]
include!("lib_tests.rs");
