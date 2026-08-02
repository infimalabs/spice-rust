//! Repository-local development commands.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

mod release;

pub use release::{CommandRecord, ReleaseEnvironment, TargetBuild, TargetBuildError, stage_target};

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

const VERIFY_STEPS: [Step; 6] = [
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
    Step::new("cargo xtask check-app", &["xtask", "check-app"]),
    Step::new(
        "cargo test --locked --workspace --all-targets",
        &["test", "--locked", "--workspace", "--all-targets"],
    ),
    Step::new(
        "cargo test --locked --workspace --doc",
        &["test", "--locked", "--workspace", "--doc"],
    ),
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
    let status = trunk_command(root, stage, rustflags)
        .status()
        .map_err(AppBuildError::Launch)?;
    if !status.success() {
        return Err(AppBuildError::Exit(status.code()));
    }
    finalize_stage(stage)
}

fn trunk_command(root: &Path, stage: &Path, rustflags: String) -> Command {
    let target = root.join("target/xtask-app");
    let build = root.join("target/xtask-app-build");
    let mut command = Command::new("trunk");
    command
        .current_dir(root.join("app"))
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        .env("CARGO_HOME", CARGO_HOME)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", target)
        .env("NO_COLOR", "true")
        .env("RUSTC_WORKSPACE_WRAPPER", "")
        .env("RUSTC_WRAPPER", "")
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
        .arg(stage);
    command
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
    use std::collections::{BTreeMap, VecDeque};

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
    fn app_build_command_owns_cache_and_wrapper_inputs() {
        let root = Path::new("/workspace");
        let stage = Path::new("/stage");
        let command = trunk_command(root, stage, "remapped-flags".to_owned());
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment,
            BTreeMap::from([
                (
                    "CARGO_BUILD_BUILD_DIR".to_owned(),
                    Some("/workspace/target/xtask-app-build".to_owned()),
                ),
                (
                    "CARGO_ENCODED_RUSTFLAGS".to_owned(),
                    Some("remapped-flags".to_owned()),
                ),
                ("CARGO_HOME".to_owned(), Some(CARGO_HOME.to_owned())),
                ("CARGO_INCREMENTAL".to_owned(), Some("0".to_owned())),
                (
                    "CARGO_TARGET_DIR".to_owned(),
                    Some("/workspace/target/xtask-app".to_owned()),
                ),
                ("NO_COLOR".to_owned(), Some("true".to_owned())),
                ("RUSTC_WORKSPACE_WRAPPER".to_owned(), Some(String::new())),
                ("RUSTC_WRAPPER".to_owned(), Some(String::new())),
            ])
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/workspace/app")));
        assert_eq!(command.get_program(), "trunk");
        assert_eq!(
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "build",
                "index.html",
                "--release",
                "--locked",
                "--filehash=false",
                "--html-output",
                "canvas-app.html",
                "--dist",
                "/stage",
            ]
        );
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
    fn verifier_steps_match_the_complete_command_guard() {
        const GUARDED_STEPS: [Step; 6] = [
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
                &[
                    "check",
                    "--locked",
                    "--workspace",
                    "--target",
                    "wasm32-unknown-unknown",
                ],
            ),
            Step::new("cargo xtask check-app", &["xtask", "check-app"]),
            Step::new(
                "cargo test --locked --workspace --all-targets",
                &["test", "--locked", "--workspace", "--all-targets"],
            ),
            Step::new(
                "cargo test --locked --workspace --doc",
                &["test", "--locked", "--workspace", "--doc"],
            ),
        ];

        assert_eq!(VERIFY_STEPS.as_slice(), GUARDED_STEPS.as_slice());
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
