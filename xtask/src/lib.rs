//! Repository-local development commands.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

mod app;
mod commit_message;
mod landing;
mod reference_transaction;
mod release;
mod stage_sources;
mod toolchain;

use app::WASM_TARGET;

pub use app::{AppBuildError, check_app, stage_app};
pub use commit_message::{CommitMessageError, WRAP_LIMIT as COMMIT_MESSAGE_WRAP_LIMIT};
pub use reference_transaction::ReferenceTransactionError;
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
/// reference-transaction hook protocol.
///
/// # Errors
///
/// Returns malformed hook input, Git execution failure, or a protected-history
/// refusal.
pub fn reference_transaction(
    repository: &Path,
    state: &OsStr,
    input: &str,
) -> Result<(), ReferenceTransactionError> {
    reference_transaction::apply(repository, state, input)
}

/// Every verification action, in the one order a run performs them.
///
/// The last action still runs the predecessor's external Python policy gate,
/// and the reason is coverage rather than habit. `gate/src/battery/manifest.toml`
/// names all 25 of that gate's standard steps against a compiled arm, but being
/// named is not being run: three metrics arms — `file-shape`, `complexity`, and
/// `magic-numbers` — acquire as evidence naming an input this build cannot reach
/// rather than as a measurement. The other 22 steps are measured here, the
/// fourth metrics arm `markdown-links` among them.
///
/// All three read this repository's own sources rather than passing over it:
/// `.rs` is in the predecessor's complexity and magic suffix sets, and every
/// tracked file has a shape. Dropping this action drops live coverage, so it
/// stays until those three arms can be measured.
const VERIFY_ACTIONS: [Action; 8] = [
    Action::Command(Step::cargo(
        "cargo fmt --all -- --check",
        &["fmt", "--all", "--", "--check"],
    )),
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
        "cargo check --locked --workspace --target wasm32-unknown-unknown",
        &["check", "--locked", "--workspace", "--target", WASM_TARGET],
    )),
    Action::Command(Step::cargo(
        "cargo xtask check-app",
        &["xtask", "check-app"],
    )),
    Action::Command(Step::cargo(
        "cargo test --locked --workspace --all-targets",
        &["test", "--locked", "--workspace", "--all-targets"],
    )),
    Action::Command(Step::cargo(
        "cargo test --locked --workspace --doc",
        &["test", "--locked", "--workspace", "--doc"],
    )),
    Action::Landing,
    Action::Command(Step::new(
        "spice",
        "spice dev pre-commit",
        &["dev", "pre-commit"],
    )),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Step {
    program: &'static str,
    label: &'static str,
    arguments: &'static [&'static str],
}

impl Step {
    const fn new(
        program: &'static str,
        label: &'static str,
        arguments: &'static [&'static str],
    ) -> Self {
        Self {
            program,
            label,
            arguments,
        }
    }

    const fn cargo(label: &'static str, arguments: &'static [&'static str]) -> Self {
        Self::new("cargo", label, arguments)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Command(Step),
    Landing,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Command(step) => step.label,
            Self::Landing => landing::LABEL,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    success: bool,
    code: Option<i32>,
}

trait Runner {
    fn run(&mut self, step: Step) -> io::Result<Status>;
    fn landing(&mut self) -> Result<landing::Report, landing::Error>;
}

struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, step: Step) -> io::Result<Status> {
        let status = Command::new(step.program).args(step.arguments).status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
    }

    fn landing(&mut self) -> Result<landing::Report, landing::Error> {
        landing::inspect()
    }
}

/// Run the complete repository verification battery in its canonical order.
///
/// # Errors
///
/// Returns the first output, launch, or command failure and does not start any
/// subsequent command.
pub fn verify(output: &mut impl Write) -> Result<(), VerifyError> {
    verify_with(&mut SystemRunner, output)
}

fn verify_with<R: Runner>(runner: &mut R, output: &mut impl Write) -> Result<(), VerifyError> {
    for action in VERIFY_ACTIONS {
        writeln!(output, "[verify] {}", action.label()).map_err(VerifyError::Output)?;
        match action {
            Action::Command(step) => {
                let status = runner.run(step).map_err(|source| VerifyError::Launch {
                    label: step.label,
                    source,
                })?;
                if !status.success {
                    return Err(VerifyError::Exit {
                        label: step.label,
                        code: status.code,
                    });
                }
            }
            Action::Landing => {
                let report = runner
                    .landing()
                    .map_err(|source| VerifyError::Landing(source.to_string()))?;
                serde_json::to_writer(&mut *output, &report).map_err(VerifyError::Encode)?;
                writeln!(output).map_err(VerifyError::Output)?;
                if !report.admitted() {
                    return Err(VerifyError::LandingRefused);
                }
            }
        }
    }
    Ok(())
}

/// A bounded verifier failure that identifies the first incomplete step.
#[derive(Debug)]
pub enum VerifyError {
    Output(io::Error),
    Encode(serde_json::Error),
    Landing(String),
    LandingRefused,
    Launch {
        label: &'static str,
        source: io::Error,
    },
    Exit {
        label: &'static str,
        code: Option<i32>,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(source) => write!(formatter, "[verify] output failed: {source}"),
            Self::Encode(source) => write!(formatter, "[verify] landing output failed: {source}"),
            Self::Landing(detail) => {
                write!(formatter, "[verify] landing acquisition failed: {detail}")
            }
            Self::LandingRefused => formatter.write_str("[verify] contract landing refused"),
            Self::Launch { label, source } => {
                write!(formatter, "[verify] failed to launch: {label} ({source})")
            }
            Self::Exit {
                label,
                code: Some(code),
            } => write!(formatter, "[verify] failed: {label} (exit {code})"),
            Self::Exit { label, code: None } => {
                write!(formatter, "[verify] failed: {label} (terminated by signal)")
            }
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Output(source) | Self::Launch { source, .. } => Some(source),
            Self::Encode(source) => Some(source),
            Self::Landing(_) | Self::LandingRefused | Self::Exit { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    enum Outcome {
        Success,
        Exit(i32),
        Launch,
    }

    struct FakeRunner {
        outcomes: VecDeque<Outcome>,
        landing: Option<Result<landing::Report, landing::Error>>,
        calls: Vec<&'static str>,
    }

    impl Runner for FakeRunner {
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

        fn landing(&mut self) -> Result<landing::Report, landing::Error> {
            self.calls.push(landing::LABEL);
            self.landing
                .take()
                .expect("fixture has one landing outcome")
        }
    }

    #[test]
    fn success_runs_every_step_in_order_and_prints_exact_labels() {
        let mut runner = runner(successful_commands());
        let mut output = Vec::new();

        verify_with(&mut runner, &mut output).expect("verification succeeds");

        assert_eq!(runner.calls, labels());
        let output = String::from_utf8(output).expect("labels are UTF-8");
        let report =
            serde_json::to_string(&landing::fixture_report(true)).expect("fixture report encodes");
        let expected = VERIFY_ACTIONS
            .into_iter()
            .map(|action| match action {
                Action::Command(step) => format!("[verify] {}\n", step.label),
                Action::Landing => format!("[verify] {}\n{report}\n", landing::LABEL),
            })
            .collect::<String>();
        assert_eq!(output, expected);
    }

    #[test]
    fn verifier_steps_match_the_complete_command_guard() {
        const GUARDED_ACTIONS: [Action; 8] = [
            Action::Command(Step::cargo(
                "cargo fmt --all -- --check",
                &["fmt", "--all", "--", "--check"],
            )),
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
                "cargo check --locked --workspace --target wasm32-unknown-unknown",
                &[
                    "check",
                    "--locked",
                    "--workspace",
                    "--target",
                    "wasm32-unknown-unknown",
                ],
            )),
            Action::Command(Step::cargo(
                "cargo xtask check-app",
                &["xtask", "check-app"],
            )),
            Action::Command(Step::cargo(
                "cargo test --locked --workspace --all-targets",
                &["test", "--locked", "--workspace", "--all-targets"],
            )),
            Action::Command(Step::cargo(
                "cargo test --locked --workspace --doc",
                &["test", "--locked", "--workspace", "--doc"],
            )),
            Action::Landing,
            Action::Command(Step::new(
                "spice",
                "spice dev pre-commit",
                &["dev", "pre-commit"],
            )),
        ];

        assert_eq!(VERIFY_ACTIONS.as_slice(), GUARDED_ACTIONS.as_slice());
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
        assert_eq!(VERIFY_ACTIONS[index + 1], Action::Landing);
    }

    /// The battery builds for a second target, and says which one.
    ///
    /// Every other step is a host build, so without this one the far side of
    /// each `cfg(not(target_arch = "wasm32"))` is never compiled by anything
    /// and breaks in a build no step runs. The label carries the triple because
    /// the same cargo command fails for host reasons too, and a reader seeing
    /// only "check failed" would look for a defect that is not there.
    #[test]
    fn verifier_builds_the_workspace_for_the_browser_target() {
        let step = VERIFY_ACTIONS
            .iter()
            .find_map(|action| match action {
                Action::Command(step) if step.arguments.contains(&"--target") => Some(step),
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

    #[test]
    fn command_failure_stops_before_the_next_step() {
        let mut runner = runner([Outcome::Success, Outcome::Exit(9), Outcome::Success]);
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("clippy failure surfaces");

        assert!(matches!(error, VerifyError::Exit { code: Some(9), .. }));
        assert_eq!(runner.calls, labels()[..2]);
    }

    #[test]
    fn launch_failure_stops_before_the_next_step() {
        let mut runner = runner([Outcome::Launch, Outcome::Success, Outcome::Success]);
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("launch refusal surfaces");

        assert!(matches!(error, VerifyError::Launch { .. }));
        assert_eq!(runner.calls, labels()[..1]);
    }

    #[test]
    fn landing_refusal_stops_before_the_python_policy_step() {
        let mut runner = runner(successful_commands());
        runner.landing = Some(Ok(landing::fixture_report(false)));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("landing refuses");

        assert!(matches!(error, VerifyError::LandingRefused));
        assert_eq!(runner.calls, labels()[..7]);
    }

    #[test]
    fn landing_acquisition_failure_stops_before_the_python_policy_step() {
        let mut runner = runner(successful_commands());
        runner.landing = Some(Err(landing::fixture_error()));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("landing fails");

        assert!(matches!(error, VerifyError::Landing(_)));
        assert_eq!(runner.calls, labels()[..7]);
    }

    #[test]
    fn installed_python_policy_is_last_and_its_failure_propagates() {
        let mut outcomes = successful_commands();
        *outcomes.last_mut().expect("policy outcome exists") = Outcome::Exit(7);
        let mut runner = runner(outcomes);
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("policy fails");

        assert!(matches!(error, VerifyError::Exit { code: Some(7), .. }));
        assert_eq!(runner.calls, labels());
        let Action::Command(policy) = VERIFY_ACTIONS[7] else {
            panic!("policy must be an external command");
        };
        assert_eq!(
            (policy.program, policy.arguments),
            ("spice", &["dev", "pre-commit"][..])
        );
    }

    fn runner(outcomes: impl IntoIterator<Item = Outcome>) -> FakeRunner {
        FakeRunner {
            outcomes: outcomes.into_iter().collect(),
            landing: Some(Ok(landing::fixture_report(true))),
            calls: Vec::new(),
        }
    }

    fn successful_commands() -> Vec<Outcome> {
        VERIFY_ACTIONS
            .iter()
            .filter(|action| matches!(action, Action::Command(_)))
            .map(|_| Outcome::Success)
            .collect()
    }

    fn labels() -> Vec<&'static str> {
        VERIFY_ACTIONS.iter().map(|action| action.label()).collect()
    }
}
