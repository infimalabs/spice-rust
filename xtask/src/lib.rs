//! Repository-local build, verification, and release commands.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

mod app;
mod commit_message;
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
pub use develop::{RefreshError, RefreshReport, RefreshRequest, refresh};
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

/// Every verification action, in the one order a run performs them.
///
/// `gate/src/battery/manifest.toml` names all 25 retired standard policy checks
/// and binds each to one compiled Rust arm; its one additional C# arm is marked
/// native by carrying no retired-step name. The workspace tests exercise that
/// exact manifest and the production executor for all four families, including
/// the structural family that once lacked acquisition. The real-browser action
/// is the one non-Cargo runtime check and requires its end-of-run receipt.
/// Contract landing remains terminal: no alternate executable discovered from
/// `PATH` repeats policy after the Rust-owned checks finish.
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
const BROWSER_RECEIPT_FLAG: &str = "--spice-coverage-receipt";
const BROWSER_RECEIPT: &[u8] = b"BROWSER-1kKSWhZk browser coverage complete\n";
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

trait Runner {
    fn entry_point(&mut self) -> Result<String, String>;
    fn format(&mut self) -> io::Result<Status>;
    fn run(&mut self, step: Step) -> io::Result<Status>;
    fn run_scoped(&mut self, step: ScopedCommand, package: &str) -> io::Result<Status>;
    fn browser(&mut self) -> io::Result<BrowserRun>;
    fn battery(&mut self) -> Result<BatteryRun, String>;
    fn landing(&mut self) -> Result<landing::Report, landing::Error>;
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BrowserRun {
    status: Status,
    completed: bool,
}

struct SystemRunner;

impl Runner for SystemRunner {
    fn entry_point(&mut self) -> Result<String, String> {
        let mut report = Vec::new();
        hooks::audit_entry_point(Path::new("."), &mut report).map_err(|error| error.to_string())?;
        String::from_utf8(report).map_err(|error| error.to_string())
    }

    fn format(&mut self) -> io::Result<Status> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let root = Path::new(".");
            let sources = gate::formatter_sources(root).map_err(io::Error::other)?;
            if sources.is_empty() {
                return Ok(Status {
                    success: true,
                    code: Some(0),
                });
            }
            let edition = gate::formatter_edition(root).map_err(io::Error::other)?;
            let status = Command::new("rustfmt")
                .args([
                    "--edition",
                    edition.as_str(),
                    "--check",
                    "--files-with-diff",
                ])
                .args(sources)
                .status()?;
            Ok(Status {
                success: status.success(),
                code: status.code(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(io::Error::other(
                "repository formatting is unavailable on wasm32",
            ))
        }
    }

    fn run(&mut self, step: Step) -> io::Result<Status> {
        let status = Command::new("cargo").args(step.arguments).status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
    }

    fn run_scoped(&mut self, step: ScopedCommand, package: &str) -> io::Result<Status> {
        let status = Command::new("cargo")
            .args(step.arguments(package))
            .status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
    }

    fn browser(&mut self) -> io::Result<BrowserRun> {
        let receipt = std::env::temp_dir().join(format!(
            "spice-browser-coverage-{}-{}",
            std::process::id(),
            NEXT_BROWSER_RECEIPT.fetch_add(1, Ordering::Relaxed),
        ));
        remove_receipt(&receipt)?;
        let process = Command::new("npm")
            .args(["run", "test:canvas", "--", BROWSER_RECEIPT_FLAG])
            .arg(&receipt)
            .status()?;
        let completed = fs::read(&receipt).is_ok_and(|body| body == BROWSER_RECEIPT);
        remove_receipt(&receipt)?;
        Ok(BrowserRun {
            status: Status {
                success: process.success(),
                code: process.code(),
            },
            completed,
        })
    }

    fn battery(&mut self) -> Result<BatteryRun, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let report =
                spice::gate::run(gate::Selector::staged()).map_err(|error| error.to_string())?;
            if report.admits() {
                let document = report.encode().map_err(|error| error.to_string())?;
                Ok(BatteryRun::Admitted(document))
            } else {
                Ok(BatteryRun::Refused(report.refusal_fold().into_bytes()))
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err("the canonical native Rust gate battery is unavailable on wasm32".to_owned())
        }
    }

    fn landing(&mut self) -> Result<landing::Report, landing::Error> {
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

/// Run the complete repository verification battery in its canonical order.
///
/// # Errors
///
/// Returns the first output, launch, or command failure and does not start any
/// subsequent command.
pub fn verify(output: &mut impl Write) -> Result<(), VerifyError> {
    verify_execution::verify_with(&mut SystemRunner, output)
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
    verify_execution::verify_package_with(package, &mut SystemRunner, output)
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
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Output(source)
            | Self::Launch { source, .. }
            | Self::ScopedLaunch { source, .. }
            | Self::BrowserLaunch { source } => Some(source),
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
            | Self::ScopedExit { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::verify_execution::{verify_package_with, verify_with};

    enum Outcome {
        Success,
        Exit(i32),
        Launch,
    }

    /// The report a current entry point produces, so the order test can pin
    /// that the audit ran without reaching a real repository.
    const ENTRY_POINT_FIXTURE: &str = "fixture guard matches this build\n";

    #[derive(Default)]
    struct RefuseDiagnosticWrite {
        accepted: Vec<u8>,
    }

    impl Write for RefuseDiagnosticWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.starts_with(b"bounded battery refusal") {
                Err(io::Error::other("fixture diagnostic output refusal"))
            } else {
                self.accepted.extend_from_slice(bytes);
                Ok(bytes.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FakeRunner {
        outcomes: VecDeque<Outcome>,
        scoped_outcomes: VecDeque<Outcome>,
        entry_point: Option<Result<String, String>>,
        browser: Option<io::Result<BrowserRun>>,
        battery: Option<Result<BatteryRun, String>>,
        landing: Option<Result<landing::Report, landing::Error>>,
        calls: Vec<&'static str>,
        scoped_calls: Vec<(ScopedCommand, String)>,
    }

    impl Runner for FakeRunner {
        fn entry_point(&mut self) -> Result<String, String> {
            self.calls.push(ENTRY_POINT_LABEL);
            self.entry_point
                .take()
                .expect("fixture has one entry-point outcome")
        }

        fn format(&mut self) -> io::Result<Status> {
            self.calls.push(FORMAT_LABEL);
            match self.outcomes.pop_front().expect("fixture has an outcome") {
                Outcome::Success => Ok(Status {
                    success: true,
                    code: Some(0),
                }),
                Outcome::Exit(code) => Ok(Status {
                    success: false,
                    code: Some(code),
                }),
                Outcome::Launch => Err(io::Error::other("fixture formatter launch refusal")),
            }
        }

        fn run(&mut self, step: Step) -> io::Result<Status> {
            self.calls.push(step.label);
            match self.outcomes.pop_front().expect("fixture has an outcome") {
                Outcome::Success => Ok(Status {
                    success: true,
                    code: Some(0),
                }),
                Outcome::Exit(code) => Ok(Status {
                    success: false,
                    code: Some(code),
                }),
                Outcome::Launch => Err(io::Error::other("fixture launch refusal")),
            }
        }

        fn run_scoped(&mut self, step: ScopedCommand, package: &str) -> io::Result<Status> {
            self.scoped_calls.push((step, package.to_owned()));
            match self
                .scoped_outcomes
                .pop_front()
                .expect("fixture has a scoped outcome")
            {
                Outcome::Success => Ok(Status {
                    success: true,
                    code: Some(0),
                }),
                Outcome::Exit(code) => Ok(Status {
                    success: false,
                    code: Some(code),
                }),
                Outcome::Launch => Err(io::Error::other("fixture scoped launch refusal")),
            }
        }

        fn browser(&mut self) -> io::Result<BrowserRun> {
            self.calls.push(BROWSER_LABEL);
            self.browser
                .take()
                .expect("fixture has one browser outcome")
        }

        fn battery(&mut self) -> Result<BatteryRun, String> {
            self.calls.push(BATTERY_LABEL);
            self.battery
                .take()
                .expect("fixture has one battery outcome")
        }

        fn landing(&mut self) -> Result<landing::Report, landing::Error> {
            self.calls.push(landing::LABEL);
            self.landing
                .take()
                .expect("fixture has one landing outcome")
        }
    }

    #[test]
    fn success_runs_every_step_in_order_and_prints_exact_lifecycle_records() {
        let mut runner = runner(successful_commands());
        let mut output = Vec::new();

        verify_with(&mut runner, &mut output).expect("verification succeeds");

        assert_eq!(runner.calls, labels());
        let output = String::from_utf8(output).expect("labels are UTF-8");
        let report =
            serde_json::to_string(&landing::fixture_report(true)).expect("fixture report encodes");
        assert_eq!(output.matches("stage=start").count(), VERIFY_ACTIONS.len());
        assert_eq!(
            output.matches("stage=terminal").count(),
            VERIFY_ACTIONS.len()
        );
        assert!(!output.contains("stage=progress"));

        let mut prior_terminal = 0;
        for (index, action) in VERIFY_ACTIONS.into_iter().enumerate() {
            let ordinal = format!("ordinal={}/{}", index + 1, VERIFY_ACTIONS.len());
            let identity = format!("identity={:?}", action.label());
            let start = output
                .lines()
                .position(|line| {
                    line.contains("stage=start")
                        && line.contains(&ordinal)
                        && line.contains(&identity)
                        && line.contains(&format!("family={}", action.family()))
                })
                .expect("action has one exact start record");
            let terminal = output
                .lines()
                .position(|line| {
                    line.contains("stage=terminal")
                        && line.contains(&ordinal)
                        && line.contains(&identity)
                })
                .expect("action has one exact terminal record");
            assert!(
                start >= prior_terminal,
                "action starts after its predecessor"
            );
            assert!(terminal > start, "action terminates after it starts");
            prior_terminal = terminal;
        }
        assert!(output.contains(ENTRY_POINT_FIXTURE));
        assert!(output.contains("{\"admitted\":true}\n"));
        assert!(output.contains(&format!("{report}\n")));
    }

    #[test]
    fn scoped_verify_runs_only_package_commands_and_names_every_omission() {
        let mut runner = scoped_runner();
        let mut output = Vec::new();

        verify_package_with("gate", &mut runner, &mut output)
            .expect("scoped package verification succeeds");

        assert_eq!(runner.calls, [ENTRY_POINT_LABEL]);
        let expected_calls = SCOPED_COMMANDS
            .into_iter()
            .map(|command| (command, "gate".to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(runner.scoped_calls, expected_calls);

        let output = String::from_utf8(output).expect("scoped verifier output is UTF-8");
        assert!(output.starts_with(concat!(
            "[verify] scoped package gate; feedback only: ",
            "run `cargo xtask verify` before task completion\n"
        )));
        let skipped_actions = VERIFY_ACTIONS
            .iter()
            .filter(|action| !matches!(action, Action::EntryPoint))
            .map(|action| action.label())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(output.contains(&format!(
            "[verify] scoped full actions skipped: {skipped_actions}\n"
        )));
        assert!(output.contains("[verify] scoped gate arms run: none\n"));
        let skipped_arms = gate::GateId::ALL
            .into_iter()
            .map(|gate_id| gate_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(output.contains(&format!(
            "[verify] scoped gate arms skipped: {skipped_arms}\n"
        )));
        assert_eq!(output.matches("stage=start").count(), 4);
        assert_eq!(output.matches("stage=terminal").count(), 4);
        assert!(!runner.calls.contains(&BROWSER_LABEL));
        assert!(!runner.calls.contains(&BATTERY_LABEL));
        assert!(!runner.calls.contains(&landing::LABEL));
    }

    #[test]
    fn scoped_verify_refuses_an_injected_or_empty_package_name_before_output() {
        for package in ["", "gate\n[verify] forged", "gate/path", "gate package"] {
            let mut runner = scoped_runner();
            let mut output = Vec::new();

            let error = verify_package_with(package, &mut runner, &mut output)
                .expect_err("invalid package scope refuses");

            assert!(matches!(error, VerifyError::Package(_)));
            assert!(runner.calls.is_empty());
            assert!(runner.scoped_calls.is_empty());
            assert!(output.is_empty());
        }
    }

    #[test]
    fn scoped_command_failure_stops_before_later_package_commands() {
        let mut runner = scoped_runner();
        runner.scoped_outcomes = [Outcome::Exit(17)].into_iter().collect();
        let mut output = Vec::new();

        let error = verify_package_with("gate", &mut runner, &mut output)
            .expect_err("scoped Clippy refusal surfaces");

        assert!(matches!(
            error,
            VerifyError::ScopedExit { code: Some(17), .. }
        ));
        assert_eq!(runner.calls, [ENTRY_POINT_LABEL]);
        assert_eq!(
            runner.scoped_calls,
            [(ScopedCommand::Clippy, "gate".to_owned())]
        );
    }

    #[test]
    fn verifier_steps_match_the_complete_command_guard() {
        const GUARDED_ACTIONS: [Action; 11] = [
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
                    "wasm32-unknown-unknown",
                    "--",
                    "-D",
                    "warnings",
                ],
            )),
            Action::Command(Step::cargo(
                "cargo check --locked --workspace --target wasm32-unknown-unknown",
                &[
                    "check",
                    "--locked",
                    "--workspace",
                    "--target",
                    "wasm32-unknown-unknown",
                ],
            )),
            Action::Browser,
            Action::Command(Step::cargo(
                "cargo test --locked --workspace --all-targets",
                &["test", "--locked", "--workspace", "--all-targets"],
            )),
            Action::Command(Step::cargo(
                "cargo doc --locked --workspace --lib --bins --no-deps",
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

        assert_eq!(VERIFY_ACTIONS.as_slice(), GUARDED_ACTIONS.as_slice());
    }

    /// Cargo's default target set documents `spice` and `xtask` as libraries
    /// only: each has a binary whose name collides with its library
    /// (`rust-lang/cargo#6313`) and cargo drops it without a word. Both
    /// `main.rs` files carry doc comments, so the sweep has to name its targets
    /// or those two files are never rustdoc-linted at all.
    #[test]
    fn documentation_names_its_targets_so_the_colliding_binaries_are_linted() {
        let sweep = VERIFY_ACTIONS
            .iter()
            .find_map(|action| match action {
                Action::Command(step) if step.label == DOCUMENTATION_LABEL => Some(step.arguments),
                _ => None,
            })
            .expect("the verifier has a documentation step");

        assert!(sweep.contains(&"--bins"), "{sweep:?} selects no binaries");
        assert!(sweep.contains(&"--lib"), "{sweep:?} selects no libraries");
        assert!(
            sweep.contains(&"--workspace"),
            "{sweep:?} leaves packages out of the sweep"
        );
    }

    #[test]
    fn verifier_explicitly_runs_workspace_doctests() {
        let (index, step) = VERIFY_ACTIONS
            .iter()
            .enumerate()
            .find_map(|(index, action)| match action {
                Action::Command(step) if step.label == "cargo test --locked --workspace --doc" => {
                    Some((index, step))
                }
                _ => None,
            })
            .expect("the verifier has a doctest step");

        assert_eq!(step.arguments, ["test", "--locked", "--workspace", "--doc"]);
        assert_eq!(VERIFY_ACTIONS[index + 1], Action::Battery);
        assert_eq!(VERIFY_ACTIONS[index + 2], Action::Landing);
    }

    #[test]
    fn verifier_owns_exactly_one_battery_action() {
        assert_eq!(
            VERIFY_ACTIONS
                .iter()
                .filter(|action| matches!(action, Action::Battery))
                .count(),
            1
        );
    }

    /// BROWSER-1kKSWhZk removed one former surface, so real Chromium guards
    /// the final production edge immediately after the browser-target build.
    #[test]
    fn verifier_makes_the_real_browser_guard_mandatory_after_the_wasm_build() {
        let browser = VERIFY_ACTIONS
            .iter()
            .position(|action| *action == Action::Browser)
            .expect("the verifier has a browser action");

        assert_eq!(browser, 5);
        assert!(
            matches!(VERIFY_ACTIONS[browser - 1], Action::Command(step) if step.arguments.contains(&WASM_TARGET))
        );
        assert_eq!(
            BROWSER_RECEIPT,
            b"BROWSER-1kKSWhZk browser coverage complete\n"
        );
        assert!(BROWSER_LABEL.contains("BROWSER-1kKSWhZk"));
    }

    #[test]
    fn zero_exit_without_the_browser_receipt_is_not_a_pass() {
        let mut runner = runner(successful_commands());
        runner.browser = Some(Ok(BrowserRun {
            status: Status {
                success: true,
                code: Some(0),
            },
            completed: false,
        }));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output)
            .expect_err("a skipped browser body emits no completion receipt");

        assert!(matches!(error, VerifyError::BrowserIncomplete));
        assert_eq!(runner.calls, labels()[..=5]);
    }

    #[test]
    fn browser_dependency_and_launch_failures_are_not_incomplete_coverage() {
        let mut missing_npm = runner(successful_commands());
        missing_npm.browser = Some(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "npm unavailable",
        )));
        let launch = verify_with(&mut missing_npm, &mut Vec::new())
            .expect_err("a missing browser launcher fails");
        assert!(matches!(launch, VerifyError::BrowserLaunch { .. }));

        let mut missing_browser = runner(successful_commands());
        missing_browser.browser = Some(Ok(BrowserRun {
            status: Status {
                success: false,
                code: Some(127),
            },
            completed: false,
        }));
        let dependency = verify_with(&mut missing_browser, &mut Vec::new())
            .expect_err("a missing Node or browser dependency fails its process");
        assert!(matches!(
            dependency,
            VerifyError::BrowserExit { code: Some(127) }
        ));
    }

    #[test]
    fn verifier_lints_the_browser_crate_for_the_wasm_target() {
        let step = VERIFY_ACTIONS
            .iter()
            .find_map(|action| match action {
                Action::Command(step)
                    if step.arguments.first() == Some(&"clippy")
                        && step.arguments.contains(&WASM_TARGET) =>
                {
                    Some(step)
                }
                _ => None,
            })
            .expect("the verifier has a wasm Clippy step");

        assert_eq!(
            step.arguments,
            [
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
        );
        assert!(step.label.contains(WASM_TARGET));
    }

    /// The battery builds the whole workspace for a second target and says
    /// which one.
    ///
    /// The app-only Clippy step reaches browser code; this distinct check also
    /// reaches every other workspace crate's target-specific surface. The
    /// label carries the triple because the same Cargo command can fail for
    /// host reasons too, and a reader seeing only "check failed" would look
    /// for a defect that is not there.
    #[test]
    fn verifier_builds_the_workspace_for_the_browser_target() {
        let step = VERIFY_ACTIONS
            .iter()
            .find_map(|action| match action {
                Action::Command(step) if step.arguments.first() == Some(&"check") => Some(step),
                _ => None,
            })
            .expect("the verifier has a cross-target step");

        assert_eq!(
            step.arguments,
            ["check", "--locked", "--workspace", "--target", WASM_TARGET],
        );
        assert!(
            step.label.contains(WASM_TARGET),
            "the label must name the target it builds for: {}",
            step.label,
        );
    }

    /// The audit sits in front of the battery to stop it, not to narrate it, so
    /// a refusal has to end the run before the first build rather than be
    /// printed and walked past.
    ///
    /// Ordering alone would not settle this: an action that reports and admits
    /// occupies the same slot and passes every order assertion here. What is
    /// pinned is that the labels stop at the first one and that the refusal
    /// reaches the operator verbatim, because the state it names is one no
    /// amount of re-running the battery clears.
    #[test]
    fn a_stale_published_guard_stops_verify_before_the_first_build() {
        let mut runner = runner(successful_commands());
        runner.entry_point = Some(Err("published guard is not this build's".to_owned()));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("a stale guard surfaces");

        assert!(matches!(error, VerifyError::EntryPoint(_)));
        assert_eq!(
            error.to_string(),
            "[verify] published guard is not this build's"
        );
        assert_eq!(runner.calls, labels()[..1]);
        let output = String::from_utf8(output).expect("labels are UTF-8");
        assert_eq!(output.matches("stage=start").count(), 1);
        assert_eq!(output.matches("stage=terminal").count(), 1);
        assert!(output.contains("disposition=error"));
        assert!(output.contains(&format!("identity={ENTRY_POINT_LABEL:?}")));
        assert!(!output.contains("ordinal=2/11"));
    }

    #[test]
    fn command_failure_stops_before_the_next_step() {
        let mut runner = runner([Outcome::Success, Outcome::Exit(9), Outcome::Success]);
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("clippy failure surfaces");

        assert!(matches!(error, VerifyError::Exit { code: Some(9), .. }));
        assert_eq!(runner.calls, labels()[..3]);
    }

    #[test]
    fn launch_failure_stops_before_the_next_step() {
        let mut runner = runner([Outcome::Launch, Outcome::Success, Outcome::Success]);
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("launch refusal surfaces");

        assert!(matches!(error, VerifyError::Launch { .. }));
        assert_eq!(runner.calls, labels()[..2]);
    }

    #[test]
    fn landing_refusal_is_terminal() {
        let mut runner = runner(successful_commands());
        runner.landing = Some(Ok(landing::fixture_report(false)));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("landing refuses");

        assert!(matches!(error, VerifyError::LandingRefused));
        assert_eq!(runner.calls, labels());
    }

    #[test]
    fn battery_refusal_is_terminal_before_landing() {
        let mut runner = runner(successful_commands());
        runner.battery = Some(Ok(BatteryRun::Refused(
            b"bounded battery refusal\n".to_vec(),
        )));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("battery refusal surfaces");

        assert!(matches!(error, VerifyError::BatteryRefused));
        assert_eq!(runner.calls, labels()[..labels().len() - 1]);
        let output = String::from_utf8(output).expect("fixture diagnostic is UTF-8");
        assert!(output.contains("bounded battery refusal\n"));
        assert!(output.contains(concat!("stage=terminal ordinal=10/11", " elapsed_ms=")));
        assert!(output.contains("disposition=refused family=in-process:gate-battery"));
        assert!(!output.contains("ordinal=11/11"));
    }

    #[test]
    fn battery_render_failure_emits_a_terminal_without_hiding_the_original_error() {
        let mut runner = runner(successful_commands());
        runner.battery = Some(Ok(BatteryRun::Refused(
            b"bounded battery refusal\n".to_vec(),
        )));
        let mut output = RefuseDiagnosticWrite::default();

        let error =
            verify_with(&mut runner, &mut output).expect_err("diagnostic write refusal surfaces");

        assert!(matches!(error, VerifyError::Output(_)));
        assert_eq!(
            error.to_string(),
            "[verify] output failed: fixture diagnostic output refusal"
        );
        assert_eq!(runner.calls, labels()[..labels().len() - 1]);
        let output = String::from_utf8(output.accepted).expect("lifecycle output is UTF-8");
        let terminal = output
            .lines()
            .filter(|line| line.contains("stage=terminal ordinal=10/11"))
            .collect::<Vec<_>>();
        assert_eq!(
            terminal.len(),
            1,
            "failed action has one terminal: {output}"
        );
        assert!(terminal[0].contains("disposition=render-error"));
        assert!(!output.contains("ordinal=11/11"));
    }

    #[test]
    fn landing_acquisition_failure_is_terminal() {
        let mut runner = runner(successful_commands());
        runner.landing = Some(Err(landing::fixture_error()));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("landing fails");

        assert!(matches!(error, VerifyError::Landing(_)));
        assert_eq!(runner.calls, labels());
    }

    #[test]
    fn compiled_contract_landing_is_the_verifier_terminus() {
        assert_eq!(VERIFY_ACTIONS.last(), Some(&Action::Landing));
        assert_eq!(VERIFY_ACTIONS[VERIFY_ACTIONS.len() - 2], Action::Battery);
        assert_eq!(
            VERIFY_ACTIONS
                .iter()
                .filter(|action| matches!(action, Action::Command(_)))
                .count(),
            6,
            "exactly six declared Cargo commands precede compiled landing"
        );
        assert_eq!(
            VERIFY_ACTIONS
                .iter()
                .filter(|action| matches!(action, Action::Format))
                .count(),
            1,
            "one gate-selected formatter action precedes compiled landing"
        );
        assert_eq!(
            VERIFY_ACTIONS
                .iter()
                .filter(|action| matches!(action, Action::Browser))
                .count(),
            1,
            "one mandatory browser action precedes compiled landing"
        );
    }

    fn runner(outcomes: impl IntoIterator<Item = Outcome>) -> FakeRunner {
        FakeRunner {
            outcomes: outcomes.into_iter().collect(),
            scoped_outcomes: VecDeque::new(),
            entry_point: Some(Ok(ENTRY_POINT_FIXTURE.to_owned())),
            browser: Some(Ok(BrowserRun {
                status: Status {
                    success: true,
                    code: Some(0),
                },
                completed: true,
            })),
            battery: Some(Ok(BatteryRun::Admitted(br#"{"admitted":true}"#.to_vec()))),
            landing: Some(Ok(landing::fixture_report(true))),
            calls: Vec::new(),
            scoped_calls: Vec::new(),
        }
    }

    fn scoped_runner() -> FakeRunner {
        let mut runner = runner([]);
        runner.scoped_outcomes = SCOPED_COMMANDS.iter().map(|_| Outcome::Success).collect();
        runner
    }

    fn successful_commands() -> Vec<Outcome> {
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Format | Action::Command(_)))
            .map(|_| Outcome::Success)
            .collect()
    }

    fn labels() -> Vec<&'static str> {
        VERIFY_ACTIONS.iter().map(|action| action.label()).collect()
    }
}
