//! Repository-local build, verification, and release commands.

use std::ffi::OsString;
use std::io::{self, Write};

use sill::CancellationToken;
use spice::ambient::Ambient;

mod app;
#[cfg(not(target_arch = "wasm32"))]
mod app_demo;
mod build_cache;
mod develop;
mod gate_command;
mod proof_image;
mod release;
#[cfg(not(target_arch = "wasm32"))]
mod report;
#[cfg(not(target_arch = "wasm32"))]
mod seal_darwin;
#[cfg(not(target_arch = "wasm32"))]
mod share_darwin;
#[cfg(not(target_arch = "wasm32"))]
mod test_evidence;
mod verify_error;
mod verify_execution;
mod verify_timing;
#[cfg(not(target_arch = "wasm32"))]
mod windows_guest;
mod windows_hosted;
mod windows_msvc;
#[cfg(not(target_arch = "wasm32"))]
mod workspace_tests;

pub use app::{AppBuildError, app};
#[cfg(not(target_arch = "wasm32"))]
pub use app_demo::{AppDemoError, app_demo};
pub use build_cache::{
    BuildCacheContext, BuildCacheDisposition, BuildCacheError, BuildCacheReport, BuildCacheRequest,
    BuildCacheRootKind, BuildCacheRootReport, BuildCacheTargetDirectory, build_cache,
    collect_build_cache_at, parse_byte_quantity,
};
pub use develop::{RefreshError, RefreshReport, RefreshRequest, refresh};
pub use gate_command::{gate_arm, gate_arm_report};
use verify_error::valid_package_name;
pub use verify_error::{GateArmError, VerifyError};
// The install step drives `spice::install`, which the appliance itself gates
// off the wasm target; the workspace wasm check compiles this crate too.
#[cfg(not(target_arch = "wasm32"))]
pub use develop::refresh_install;
pub use proof_image::{ProofImageError, build_proof_image, load_proof_image};
pub use release::{
    CommandRecord, MacSymbols, ReleaseEnvironment, TargetBuild, TargetBuildError,
    build_windows_appliance, stage_target,
};
#[cfg(not(target_arch = "wasm32"))]
pub use report::report;
#[cfg(not(target_arch = "wasm32"))]
pub use seal_darwin::{PlacedExecutable, SealDarwinError, place_executable, seal_runner};
#[cfg(not(target_arch = "wasm32"))]
pub use share_darwin::{ShareDarwinError, ShareDarwinRequest, share_darwin};
#[cfg(not(target_arch = "wasm32"))]
pub use test_evidence::{TestEvidenceRunError, run as run_test_evidence};
#[cfg(not(target_arch = "wasm32"))]
pub use windows_guest::{WindowsGuestError, windows_guest};
pub use windows_hosted::{
    HostedExecution, WindowsHostedError, windows_hosted, windows_hosted_stage,
};
pub use windows_msvc::{WindowsMsvcError, windows_msvc};
#[cfg(not(target_arch = "wasm32"))]
pub use workspace_tests::{
    WorkspaceTestError, WorkspaceTestExecution,
    prebuild_shared_at as prebuild_shared_workspace_tests_at, run as run_workspace_tests,
    run_at as run_workspace_tests_at,
    run_prebuilt_shared_at as run_prebuilt_shared_workspace_tests_at,
};

/// Descriptive inventory printed by package-scoped verification.
///
/// These strings are deliberately not executable commands or scheduler arms;
/// they explain which retired repository-wide surfaces a package check omits.
const RETIRED_REPOSITORY_ACTIONS: [&str; 12] = [
    "cargo build --locked -p spice --bin spice (freshness reference and app resident)",
    "repository completion publication boundary",
    "rustfmt --edition <workspace> --check --files-with-diff <gate::formatter_sources>",
    "cargo clippy --locked --workspace --all-targets -- -D warnings",
    "cargo app-wasm-typecheck",
    "cargo check --locked --workspace --target wasm32-unknown-unknown",
    "spice dev xtask -- app (app compiler evidence)",
    "deterministic sharded cargo test --locked --workspace --all-targets",
    "cargo doc --locked --workspace --lib --bins --no-deps",
    "cargo test --locked --workspace --doc",
    "canonical Rust gate battery",
    "contract landing",
];

const SCOPED_COMMANDS: [ScopedCommand; 3] = [
    ScopedCommand::Clippy,
    ScopedCommand::Test,
    ScopedCommand::DocTest,
];

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
            Self::Test => format!("cargo test --locked --package {package} --all-targets"),
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
    #[cfg(test)]
    fn fixture(success: bool, code: Option<i32>) -> Self {
        Self {
            status: Status { success, code },
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
    fn run_scoped(
        &self,
        command: ScopedCommand,
        package: &str,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError>;
}

/// Runs scoped commands as child processes of the environment `main` captured.
struct SystemRunner<'a> {
    ambient: &'a Ambient,
}

impl Runner for SystemRunner<'_> {
    fn run_scoped(
        &self,
        command: ScopedCommand,
        package: &str,
        cancellation: &CancellationToken,
        cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        let label = command.label(package);
        let arguments = command.arguments(package).into_iter().map(OsString::from);
        verify_execution::run_process(
            &label,
            "cargo",
            arguments,
            cancellation,
            cargo_jobs,
            &[],
            self.ambient,
        )
    }
}

/// Refuse the retired repository-wide verification graph.
///
/// # Errors
///
/// Always returns [`VerifyError::RepositoryWideDisabled`]. Callers should use
/// [`verify_package`] or the staged source selector instead.
pub fn verify(_output: &mut impl Write) -> Result<(), VerifyError> {
    Err(VerifyError::RepositoryWideDisabled)
}

/// Refuse the retired private full-verifier child entry point.
///
/// This remains public only so the native command binary can retire its former
/// unadvertised child spelling through the same typed refusal as [`verify`].
///
/// # Errors
///
/// Always returns [`VerifyError::RepositoryWideDisabled`].
#[doc(hidden)]
pub fn verify_child(_output: &mut impl Write) -> Result<(), VerifyError> {
    Err(VerifyError::RepositoryWideDisabled)
}

/// Run lint, test, and documentation feedback for one Cargo package.
///
/// Its first output records every repository-wide action and canonical battery
/// arm it does not execute. Task completion relies on selected source evidence,
/// so this package-local feedback remains deliberately local.
///
/// # Errors
///
/// Returns an invalid package name or the first output, launch, or command
/// failure.
pub fn verify_package(
    package: &str,
    ambient: &Ambient,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    verify_execution::verify_package_system(package, ambient, output)
}

#[cfg(test)]
mod tests;
