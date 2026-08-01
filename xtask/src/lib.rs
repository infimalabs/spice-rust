//! Repository-local development commands.

use std::fmt;
use std::io::{self, Write};
use std::process::Command;

/// The target the browser app is compiled for.
///
/// Every other step builds for the host and only for the host, so the far side
/// of each `cfg(not(target_arch = "wasm32"))` is code no host step ever
/// compiles. The app is compiled from this workspace and embedded in the binary
/// that owns the socket, which makes that far side shipped code rather than a
/// portability courtesy.
const WASM_TARGET: &str = "wasm32-unknown-unknown";

const VERIFY_STEPS: [Step; 5] = [
    Step::new(
        "cargo fmt --all -- --check",
        &["fmt", "--all", "--", "--check"],
    ),
    Step::new(
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
    ),
    Step::new(
        "cargo check --locked --workspace --target wasm32-unknown-unknown",
        &["check", "--locked", "--workspace", "--target", WASM_TARGET],
    ),
    Step::new(
        "cargo test --locked --workspace --all-targets",
        &["test", "--locked", "--workspace", "--all-targets"],
    ),
    Step::new(
        "cargo test --locked --workspace --doc",
        &["test", "--locked", "--workspace", "--doc"],
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Step {
    label: &'static str,
    arguments: &'static [&'static str],
}

impl Step {
    const fn new(label: &'static str, arguments: &'static [&'static str]) -> Self {
        Self { label, arguments }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Status {
    success: bool,
    code: Option<i32>,
}

trait Runner {
    fn run(&mut self, step: Step) -> io::Result<Status>;
}

struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, step: Step) -> io::Result<Status> {
        let status = Command::new("cargo").args(step.arguments).status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
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
    for step in VERIFY_STEPS {
        writeln!(output, "[verify] {}", step.label).map_err(VerifyError::Output)?;
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
    Ok(())
}

/// A bounded verifier failure that identifies the first incomplete step.
#[derive(Debug)]
pub enum VerifyError {
    Output(io::Error),
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
            Self::Exit { .. } => None,
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
    }

    #[test]
    fn success_runs_every_step_in_order_and_prints_exact_labels() {
        let mut runner = runner(VERIFY_STEPS.map(|_| Outcome::Success));
        let mut output = Vec::new();

        verify_with(&mut runner, &mut output).expect("verification succeeds");

        assert_eq!(runner.calls, labels());
        assert_eq!(
            String::from_utf8(output).expect("labels are UTF-8"),
            labels()
                .into_iter()
                .map(|label| format!("[verify] {label}\n"))
                .collect::<String>()
        );
    }

    #[test]
    fn verifier_explicitly_runs_workspace_doctests() {
        let step = VERIFY_STEPS
            .iter()
            .find(|step| step.label == "cargo test --locked --workspace --doc")
            .expect("the verifier has a doctest step");

        assert_eq!(step.arguments, ["test", "--locked", "--workspace", "--doc"]);
        assert_eq!(VERIFY_STEPS.last(), Some(step));
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
        let step = VERIFY_STEPS
            .iter()
            .find(|step| step.arguments.contains(&"--target"))
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

    fn runner(outcomes: impl IntoIterator<Item = Outcome>) -> FakeRunner {
        FakeRunner {
            outcomes: outcomes.into_iter().collect(),
            calls: Vec::new(),
        }
    }

    fn labels() -> Vec<&'static str> {
        VERIFY_STEPS.iter().map(|step| step.label).collect()
    }
}
