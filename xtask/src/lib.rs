//! Repository-local development commands.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

mod succession;

/// The target the browser app is compiled for.
///
/// Every other step builds for the host and only for the host, so the far side
/// of each `cfg(not(target_arch = "wasm32"))` is code no host step ever
/// compiles. The app is compiled from this workspace and embedded in the binary
/// that owns the socket, which makes that far side shipped code rather than a
/// portability courtesy.
const WASM_TARGET: &str = "wasm32-unknown-unknown";
const CARGO_HOME: &str = concat!(env!("HOME"), "/.cargo");

const APP_FILES: [&str; 4] = ["app.module", "app_bg.wasm", "canvas-app.html", "style.css"];
const GENERATED_MODULE_REFERENCES: usize = 2;

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
    Action::Readiness,
    Action::Command(Step::new(
        "spice",
        "spice dev pre-commit",
        &["dev", "pre-commit"],
    )),
];

/// Compile the browser app with the pinned Trunk adapter and replace the
/// committed stage with exactly those generated assets.
pub fn stage_app(output: &mut impl Write) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    let stage = root.join("dist/stage");
    run_trunk(&root, &stage, output)
}

/// Rebuild the browser app outside the committed stage and compare every byte.
pub fn check_app(output: &mut impl Write) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    let temporary = root
        .join("target")
        .join(format!("xtask-canvas-check-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    let result = run_trunk(&root, &temporary, output)
        .and_then(|()| compare_stage(&root.join("dist/stage"), &temporary));
    let cleanup = fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
        path: temporary,
        source,
    });
    result.and(cleanup)
}

fn run_trunk(root: &Path, stage: &Path, output: &mut impl Write) -> Result<(), AppBuildError> {
    writeln!(output, "[app] trunk 0.21.14 -> {}", stage.display())
        .map_err(AppBuildError::Output)?;
    let rustflags = remapped_rustflags(root)?;
    let status = Command::new("trunk")
        .current_dir(root.join("app"))
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        .env("CARGO_HOME", CARGO_HOME)
        .env("NO_COLOR", "true")
        .args([
            "build",
            "index.html",
            "--release",
            "--locked",
            "--filehash=false",
            "--html-output",
            "canvas-app.html",
            "--dist",
        ])
        .arg(stage)
        .status()
        .map_err(AppBuildError::Launch)?;
    if !status.success() {
        return Err(AppBuildError::Exit(status.code()));
    }
    finalize_stage(stage)
}

fn finalize_stage(stage: &Path) -> Result<(), AppBuildError> {
    let generated = stage.join("app.js");
    let module = stage.join("app.module");
    if module.exists() {
        return Err(AppBuildError::GeneratedStage(
            "app.module existed before generated app.js was renamed".to_owned(),
        ));
    }
    fs::rename(&generated, &module).map_err(|source| AppBuildError::Io {
        path: generated,
        source,
    })?;

    let html_path = stage.join("canvas-app.html");
    let html = fs::read_to_string(&html_path).map_err(|source| AppBuildError::Io {
        path: html_path.clone(),
        source,
    })?;
    let references = html.matches("/app.js").count();
    if references != GENERATED_MODULE_REFERENCES {
        return Err(AppBuildError::GeneratedStage(format!(
            "canvas-app.html carried {references} app.js references, \
             expected {GENERATED_MODULE_REFERENCES}"
        )));
    }
    fs::write(&html_path, html.replace("/app.js", "/app.module")).map_err(|source| {
        AppBuildError::Io {
            path: html_path,
            source,
        }
    })
}

fn remapped_rustflags(root: &Path) -> Result<String, AppBuildError> {
    let root = root
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(root.to_path_buf()))?;
    Ok([
        format!("--remap-path-prefix={root}=/workspace"),
        format!("--remap-path-prefix={CARGO_HOME}=/cargo-home"),
    ]
    .join("\u{1f}"))
}

fn compare_stage(expected: &Path, observed: &Path) -> Result<(), AppBuildError> {
    let expected_names = stage_names(expected)?;
    let observed_names = stage_names(observed)?;
    if expected_names != APP_FILES || observed_names != APP_FILES {
        return Err(AppBuildError::Inventory {
            expected: expected_names,
            observed: observed_names,
        });
    }
    for name in APP_FILES {
        let expected_path = expected.join(name);
        let observed_path = observed.join(name);
        let expected_bytes = fs::read(&expected_path).map_err(|source| AppBuildError::Io {
            path: expected_path,
            source,
        })?;
        let observed_bytes = fs::read(&observed_path).map_err(|source| AppBuildError::Io {
            path: observed_path,
            source,
        })?;
        if expected_bytes != observed_bytes {
            return Err(AppBuildError::Drift(name.to_owned()));
        }
    }
    Ok(())
}

fn stage_names(stage: &Path) -> Result<Vec<String>, AppBuildError> {
    let mut names = fs::read_dir(stage)
        .map_err(|source| AppBuildError::Io {
            path: stage.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry
                .map_err(|source| AppBuildError::Io {
                    path: stage.to_path_buf(),
                    source,
                })
                .and_then(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| AppBuildError::NonUtf8(entry.path()))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

fn workspace_root() -> Result<PathBuf, AppBuildError> {
    std::env::current_dir().map_err(|source| AppBuildError::Io {
        path: PathBuf::from("."),
        source,
    })
}

#[derive(Debug)]
pub enum AppBuildError {
    Output(io::Error),
    Launch(io::Error),
    Exit(Option<i32>),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    NonUtf8(PathBuf),
    GeneratedStage(String),
    Inventory {
        expected: Vec<String>,
        observed: Vec<String>,
    },
    Drift(String),
}

impl fmt::Display for AppBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(error) => write!(formatter, "app build output failed: {error}"),
            Self::Launch(error) => write!(formatter, "trunk did not launch: {error}"),
            Self::Exit(Some(code)) => write!(formatter, "trunk failed with exit {code}"),
            Self::Exit(None) => formatter.write_str("trunk was terminated by a signal"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::NonUtf8(path) => write!(formatter, "stage path is not UTF-8: {}", path.display()),
            Self::GeneratedStage(reason) => {
                write!(formatter, "generated app stage refused: {reason}")
            }
            Self::Inventory { expected, observed } => write!(
                formatter,
                "staged app inventory differs: committed={expected:?}, rebuilt={observed:?}"
            ),
            Self::Drift(path) => write!(formatter, "staged app drifted at {path}"),
        }
    }
}

impl std::error::Error for AppBuildError {}

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
    Readiness,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Command(step) => step.label,
            Self::Readiness => succession::LABEL,
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
    fn readiness(&mut self) -> Result<succession::Report, succession::Error>;
}

struct SystemRunner<'a> {
    search_path: Option<&'a OsStr>,
}

impl Runner for SystemRunner<'_> {
    fn run(&mut self, step: Step) -> io::Result<Status> {
        let status = Command::new(step.program).args(step.arguments).status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
    }

    fn readiness(&mut self) -> Result<succession::Report, succession::Error> {
        let root = std::env::current_dir().map_err(|source| succession::Error::Io {
            path: PathBuf::from("."),
            source,
        })?;
        succession::inspect(&root, self.search_path)
    }
}

/// Run the complete repository verification battery in its canonical order.
///
/// # Errors
///
/// Returns the first output, launch, or command failure and does not start any
/// subsequent command.
pub fn verify(search_path: Option<&OsStr>, output: &mut impl Write) -> Result<(), VerifyError> {
    verify_with(&mut SystemRunner { search_path }, output)
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
            Action::Readiness => {
                let report = runner
                    .readiness()
                    .map_err(|source| VerifyError::Readiness(source.to_string()))?;
                serde_json::to_writer(&mut *output, &report).map_err(VerifyError::Encode)?;
                writeln!(output).map_err(VerifyError::Output)?;
                if !report.admitted() {
                    return Err(VerifyError::ReadinessRefused);
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
    Readiness(String),
    ReadinessRefused,
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
            Self::Encode(source) => write!(formatter, "[verify] readiness output failed: {source}"),
            Self::Readiness(detail) => {
                write!(formatter, "[verify] readiness acquisition failed: {detail}")
            }
            Self::ReadinessRefused => formatter.write_str("[verify] succession readiness refused"),
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
            Self::Readiness(_) | Self::ReadinessRefused | Self::Exit { .. } => None,
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
        readiness: Option<Result<succession::Report, succession::Error>>,
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

        fn readiness(&mut self) -> Result<succession::Report, succession::Error> {
            self.calls.push(succession::LABEL);
            self.readiness
                .take()
                .expect("fixture has one readiness outcome")
        }
    }

    #[test]
    fn success_runs_every_step_in_order_and_prints_exact_labels() {
        let mut runner = runner(successful_commands());
        let mut output = Vec::new();

        verify_with(&mut runner, &mut output).expect("verification succeeds");

        assert_eq!(runner.calls, labels());
        let output = String::from_utf8(output).expect("labels are UTF-8");
        let report = serde_json::to_string(&succession::fixture_report(true))
            .expect("fixture report encodes");
        let expected = VERIFY_ACTIONS
            .into_iter()
            .map(|action| match action {
                Action::Command(step) => format!("[verify] {}\n", step.label),
                Action::Readiness => format!("[verify] {}\n{report}\n", succession::LABEL),
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
            Action::Readiness,
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
        assert_eq!(VERIFY_ACTIONS[index + 1], Action::Readiness);
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
    fn readiness_refusal_stops_before_the_python_policy_step() {
        let mut runner = runner(successful_commands());
        runner.readiness = Some(Ok(succession::fixture_report(false)));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("readiness refuses");

        assert!(matches!(error, VerifyError::ReadinessRefused));
        assert_eq!(runner.calls, labels()[..7]);
    }

    #[test]
    fn readiness_acquisition_failure_stops_before_the_python_policy_step() {
        let mut runner = runner(successful_commands());
        runner.readiness = Some(Err(succession::Error::PythonLedgerBoundary));
        let mut output = Vec::new();

        let error = verify_with(&mut runner, &mut output).expect_err("readiness fails");

        assert!(matches!(error, VerifyError::Readiness(_)));
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
            readiness: Some(Ok(succession::fixture_report(true))),
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
