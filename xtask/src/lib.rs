//! Repository-local build, verification, and release commands.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use sill::CancellationToken;

mod app;
mod commit_message;
// The refresh drives `spice::install`, which the appliance itself gates off the
// wasm target; the workspace wasm check compiles this crate too.
#[cfg(not(target_arch = "wasm32"))]
mod develop;
mod hooks;
mod landing;
mod proof_image;
mod reference_transaction;
mod release;
mod verify_execution;
mod verify_timing;

use app::WASM_TARGET;
pub use app::{AppBuildError, app};
pub use commit_message::{CommitMessageError, WRAP_LIMIT as COMMIT_MESSAGE_WRAP_LIMIT};
#[cfg(not(target_arch = "wasm32"))]
pub use develop::{RefreshError, RefreshReport, RefreshRequest, refresh, refresh_install};
pub use hooks::{
    HookActivationError, activate_hooks, audit_entry_point, audit_entry_point_for_guard,
    compiled_reference_transaction_guard_digest, reference_transaction_guard_digest,
};
pub use proof_image::{ProofImageError, build_proof_image, load_proof_image};
pub use reference_transaction::{ReferenceTransactionError, WorktreeDamage};
pub use release::{CommandRecord, ReleaseEnvironment, TargetBuild, TargetBuildError, stage_target};

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
const VERIFY_ACTIONS: [Action; 11] = [
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
    Action::Browser,
    Action::Command(Step::cargo(
        "cargo test --locked --workspace --all-targets",
        &["test", "--locked", "--workspace", "--all-targets"],
    )),
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
const BROWSER_RECEIPT_FLAG: &str = "--spice-coverage-receipt";
const BROWSER_RECEIPT: &[u8] = b"BROWSER-1kKSWhZk browser coverage complete\n";
const BROWSER_BINARY_ENV: &str = "SPICE_CANVAS_BINARY"; // env-policy: allow
static NEXT_BROWSER_RECEIPT: AtomicU64 = AtomicU64::new(0);

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
            Self::Battery => "child-process:gate-battery",
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
    TimedOut,
}

#[derive(Debug)]
struct BrowserRun {
    process: CommandRun,
    completed: bool,
}

struct SystemRunner;

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
        let receipt = std::env::temp_dir().join(format!(
            "spice-browser-coverage-{}-{}",
            std::process::id(),
            NEXT_BROWSER_RECEIPT.fetch_add(1, Ordering::Relaxed),
        ));
        remove_receipt(&receipt).map_err(RunError::Launch)?;
        let root = std::env::current_dir().map_err(RunError::Launch)?;
        let canvas_root = root.join("target/verify-canvas");
        let canvas_target = canvas_root.join("target");
        let canvas_build = canvas_root.join("build");
        let canvas_binary = canvas_target.join("debug/spice");
        let process = verify_execution::run_process(
            BROWSER_LABEL,
            "npm",
            [
                std::ffi::OsString::from("run"),
                std::ffi::OsString::from("test:canvas"),
                std::ffi::OsString::from("--"),
                std::ffi::OsString::from(BROWSER_RECEIPT_FLAG),
                receipt.as_os_str().to_owned(),
            ],
            cancellation,
            cargo_jobs,
            &[
                ("CARGO_TARGET_DIR", canvas_target.as_os_str()),
                ("CARGO_BUILD_BUILD_DIR", canvas_build.as_os_str()),
                (BROWSER_BINARY_ENV, canvas_binary.as_os_str()),
            ],
        )?;
        let completed = fs::read(&receipt).is_ok_and(|body| body == BROWSER_RECEIPT);
        remove_receipt(&receipt).map_err(|error| RunError::Cleanup(error.to_string()))?;
        Ok(BrowserRun { process, completed })
    }

    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let executable = std::env::current_exe().map_err(RunError::Launch)?;
            let run = verify_execution::run_process(
                BATTERY_LABEL,
                executable,
                ["verify-action", "battery"],
                cancellation,
                1,
                &[],
            )?;
            match run {
                CommandRun::Cancelled(_) => Ok(BatteryRun::Cancelled),
                CommandRun::TimedOut(_) => Ok(BatteryRun::TimedOut),
                CommandRun::Exited(output) if output.status.success => {
                    Ok(BatteryRun::Admitted(output.stdout))
                }
                CommandRun::Exited(output)
                    if output.status.code == Some(i32::from(VERIFY_ACTION_REFUSAL_EXIT)) =>
                {
                    Ok(BatteryRun::Refused(output.stdout))
                }
                CommandRun::Exited(output) => {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    Ok(BatteryRun::Error(if detail.is_empty() {
                        format!("battery child exited {:?}", output.status.code)
                    } else {
                        detail
                    }))
                }
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

fn remove_receipt(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Execute the gate battery body for the verifier's supervised child process.
///
/// The boolean is true for admission and false for a policy refusal. The
/// caller owns the distinct process exit codes so the parent scheduler can
/// distinguish a refusal from an acquisition or encoding fault.
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
