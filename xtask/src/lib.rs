//! Repository-local development commands.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

mod commit_message;
mod reference_transaction;
mod release;
mod succession;

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

/// The target the browser app is compiled for.
///
/// Every other step builds for the host and only for the host, so the far side
/// of each `cfg(not(target_arch = "wasm32"))` is code no host step ever
/// compiles. The app is compiled from this workspace and embedded in the binary
/// that owns the socket, which makes that far side shipped code rather than a
/// portability courtesy.
const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The path the app build's Cargo home is remapped to.
///
/// The build is told to read crates from wherever the rim resolved, and told to
/// record that it read them from here. Only the second half can be a constant:
/// the recorded spelling has to be the same on every host or the artifact stops
/// being reproducible, which is the whole reason the remapping exists.
const REMAPPED_CARGO_HOME: &str = "/cargo-home";

/// The host whose builds own the committed app stage.
///
/// Cargo folds `rustc --version --verbose`, which names the host triple, into
/// every crate's `-Cmetadata`. The same sources therefore emit different
/// wasm-bindgen disambiguator symbols and a differently hashed
/// `canvas-app.html` on every other host, so byte equality against a
/// foreign-triple rebuild is not a stricter check but an unreachable one. One
/// environment has to own these bytes; it is the sealed Linux image, whose
/// `rustc` reports `x86_64-unknown-linux-gnu`, because that is the environment
/// the release battery can reproduce with networking disabled.
const STAGING_HOST: &str = "x86_64-linux";

/// This host, spelled with the two triple fields a running program can read.
///
/// A triple's vendor and environment fields are not observable from inside the
/// program, so the comparison uses what is, and the documentation above names
/// the full triple it stands for.
fn host_spelling() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

const APP_FILES: [&str; 4] = ["app.module", "app_bg.wasm", "canvas-app.html", "style.css"];
const RELEASE_STAGE_DIRECTORIES: [&str; 2] = ["objects", "targets"];
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
///
/// Only the staging host may write the stage: a rebuild from anywhere else
/// replaces bytes the release battery reproduces with bytes it cannot.
///
/// # Errors
///
/// Returns a foreign staging host, a missing Cargo home, Trunk execution
/// failure, or a refused generated stage.
pub fn stage_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let host = host_spelling();
    if host != STAGING_HOST {
        return Err(AppBuildError::ForeignStagingHost(host));
    }
    let root = workspace_root()?;
    let stage = root.join("dist/stage");
    run_trunk(&root, environment, &stage, output)
}

/// Rebuild the browser app outside the committed stage and compare it back.
///
/// Every host rebuilds and compares the staged inventory, which is the part of
/// the answer a rebuild anywhere can give. Only the staging host goes on to
/// compare bytes, because only its compiler metadata can reproduce them.
///
/// # Errors
///
/// Returns a missing Cargo home, Trunk execution failure, an inventory that
/// differs from the rebuild, or, on the staging host, the first staged asset
/// whose bytes differ.
pub fn check_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    let temporary = environment
        .build_root(&root)
        .join(format!("xtask-canvas-check-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    let result = run_trunk(&root, environment, &temporary, output).and_then(|()| {
        let stage = root.join("dist/stage");
        compare_stage_inventory(&stage, &temporary)?;
        let host = host_spelling();
        if host == STAGING_HOST {
            return compare_stage_bytes(&stage, &temporary);
        }
        writeln!(
            output,
            "[app] {host} is not {STAGING_HOST}; compared the staged inventory \
             without asserting bytes no host but {STAGING_HOST} can reproduce"
        )
        .map_err(AppBuildError::Output)
    });
    let cleanup = fs::remove_dir_all(&temporary).map_err(|source| AppBuildError::Io {
        path: temporary,
        source,
    });
    result.and(cleanup)
}

fn run_trunk(
    root: &Path,
    environment: &ReleaseEnvironment,
    stage: &Path,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let build_root = environment.build_root(root);
    let cargo_home = environment
        .cargo_home()
        .ok_or(AppBuildError::MissingCargoHome)?;
    prepare_app_build(&build_root)?;
    writeln!(output, "[app] trunk 0.21.14 -> {}", stage.display())
        .map_err(AppBuildError::Output)?;
    let rustflags = remapped_rustflags(root, &cargo_home)?;
    let status = trunk_command(root, &build_root, &cargo_home, stage, rustflags)
        .status()
        .map_err(AppBuildError::Launch)?;
    if !status.success() {
        return Err(AppBuildError::Exit(status.code()));
    }
    finalize_stage(stage)
}

fn app_build_directory(build_root: &Path) -> PathBuf {
    build_root.join("xtask-app-build")
}

fn prepare_app_build(build_root: &Path) -> Result<(), AppBuildError> {
    let build = app_build_directory(build_root);
    fs::create_dir_all(&build).map_err(|source| AppBuildError::Io {
        path: build,
        source,
    })
}

fn trunk_command(
    root: &Path,
    build_root: &Path,
    cargo_home: &OsStr,
    stage: &Path,
    rustflags: String,
) -> Command {
    let target = build_root.join("xtask-app");
    let build = app_build_directory(build_root);
    let mut command = Command::new("trunk");
    command
        .current_dir(root.join("app"))
        // Cargo exports `CARGO` as the absolute path of the binary running
        // this command, and Trunk builds with that binary rather than the one
        // the environment puts on `PATH`. In the sealed image those are not
        // the same program: `PATH` names a wrapper that supplies the vendored
        // source replacement, so an inherited `CARGO` reaches past it to a
        // Cargo with no offline sources and the build fails resolving crates
        // that are sitting right there. Dropped for the same reason the
        // wrappers below are: the app build chooses its own toolchain.
        .env_remove("CARGO")
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        .env("CARGO_HOME", cargo_home)
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

fn remapped_rustflags(root: &Path, cargo_home: &OsStr) -> Result<String, AppBuildError> {
    let root = root
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(root.to_path_buf()))?;
    let cargo_home = cargo_home
        .to_str()
        .ok_or_else(|| AppBuildError::NonUtf8(PathBuf::from(cargo_home)))?;
    Ok([
        format!("--remap-path-prefix={root}=/workspace"),
        format!("--remap-path-prefix={cargo_home}={REMAPPED_CARGO_HOME}"),
    ]
    .join("\u{1f}"))
}

fn compare_stage_inventory(expected: &Path, observed: &Path) -> Result<(), AppBuildError> {
    let expected_names = stage_names(expected)?;
    let observed_names = stage_names(observed)?;
    let expected_app_names = expected_names
        .iter()
        .filter(|name| APP_FILES.contains(&name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut expected_release_shape_is_valid = true;
    for name in expected_names
        .iter()
        .filter(|name| !APP_FILES.contains(&name.as_str()))
    {
        if !RELEASE_STAGE_DIRECTORIES.contains(&name.as_str()) {
            expected_release_shape_is_valid = false;
            continue;
        }
        let path = expected.join(name);
        let metadata =
            fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io { path, source })?;
        expected_release_shape_is_valid &= metadata.file_type().is_dir();
    }
    if expected_app_names != APP_FILES
        || observed_names != APP_FILES
        || !expected_release_shape_is_valid
    {
        return Err(AppBuildError::Inventory {
            expected: expected_names,
            observed: observed_names,
        });
    }
    Ok(())
}

fn compare_stage_bytes(expected: &Path, observed: &Path) -> Result<(), AppBuildError> {
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
    MissingCargoHome,
    ForeignStagingHost(String),
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
            Self::MissingCargoHome => formatter.write_str(
                "the app build needs CARGO_HOME or HOME to locate the crates it compiles",
            ),
            Self::ForeignStagingHost(host) => write!(
                formatter,
                "the app stage is owned by {STAGING_HOST} builds and this host is {host}: \
                 staging here would replace bytes the sealed image reproduces with bytes \
                 no other environment can"
            ),
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

struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, step: Step) -> io::Result<Status> {
        let status = Command::new(step.program).args(step.arguments).status()?;
        Ok(Status {
            success: status.success(),
            code: status.code(),
        })
    }

    fn readiness(&mut self) -> Result<succession::Report, succession::Error> {
        succession::inspect()
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
    use std::collections::{BTreeMap, VecDeque};

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

    /// The build root here is deliberately outside the workspace, which is how
    /// the sealed image presents it: sources on a read-only mount, writes on a
    /// separate scratch. Every writable name the command carries must land on
    /// the scratch, while the directory it reads sources from must not move.
    #[test]
    fn app_build_command_owns_cache_and_wrapper_inputs() {
        let root = Path::new("/workspace");
        let build_root = Path::new("/cargo-target");
        let stage = Path::new("/stage");
        let command = trunk_command(
            root,
            build_root,
            OsStr::new("/cargo-home"),
            stage,
            "remapped-flags".to_owned(),
        );
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
                // Removed, not emptied: Trunk falls back to `PATH` only when
                // `CARGO` is absent, and an empty value would name a program
                // that does not exist.
                ("CARGO".to_owned(), None),
                (
                    "CARGO_BUILD_BUILD_DIR".to_owned(),
                    Some("/cargo-target/xtask-app-build".to_owned()),
                ),
                (
                    "CARGO_ENCODED_RUSTFLAGS".to_owned(),
                    Some("remapped-flags".to_owned()),
                ),
                ("CARGO_HOME".to_owned(), Some("/cargo-home".to_owned())),
                ("CARGO_INCREMENTAL".to_owned(), Some("0".to_owned())),
                (
                    "CARGO_TARGET_DIR".to_owned(),
                    Some("/cargo-target/xtask-app".to_owned()),
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
    fn app_build_preparation_creates_the_dedicated_build_directory() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-build-preparation-{}",
            std::process::id(),
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale app-build fixture");
        }

        prepare_app_build(&root).expect("prepare the app-build directory");

        let build = app_build_directory(&root);
        assert!(
            build.is_dir(),
            "the dedicated Cargo build directory is absent"
        );
        fs::remove_dir_all(root).expect("remove app-build fixture");
    }

    #[test]
    fn app_comparison_admits_only_release_owned_stage_directories() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-stage-inventory-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let expected = root.join("expected");
        let observed = root.join("observed");
        fs::create_dir_all(&expected).expect("create expected stage");
        fs::create_dir_all(&observed).expect("create observed stage");
        for name in APP_FILES {
            fs::write(expected.join(name), name).expect("write expected app file");
            fs::write(observed.join(name), name).expect("write observed app file");
        }
        for name in RELEASE_STAGE_DIRECTORIES {
            fs::create_dir(expected.join(name)).expect("create release stage directory");
        }

        compare_stage_inventory(&expected, &observed)
            .expect("release directories are separate ownership");

        fs::write(expected.join("unowned"), b"bytes").expect("write unowned stage entry");
        assert!(matches!(
            compare_stage_inventory(&expected, &observed),
            Err(AppBuildError::Inventory { .. })
        ));
        fs::remove_file(expected.join("unowned")).expect("remove unowned stage entry");
        fs::remove_dir(expected.join("objects")).expect("remove release stage directory");
        fs::write(expected.join("objects"), b"not a directory")
            .expect("replace release directory with file");
        assert!(matches!(
            compare_stage_inventory(&expected, &observed),
            Err(AppBuildError::Inventory { .. })
        ));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// The two comparisons have to answer separately, because every host runs
    /// the first and only the staging host runs the second. A byte difference
    /// must therefore be invisible to the inventory question and fatal to the
    /// byte question; if the inventory check still noticed bytes, a foreign
    /// host would fail on exactly the difference it is excused from having.
    #[test]
    fn a_byte_difference_is_invisible_to_the_inventory_and_fatal_to_the_bytes() {
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-app-stage-bytes-{}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale fixture");
        }
        let expected = root.join("expected");
        let observed = root.join("observed");
        fs::create_dir_all(&expected).expect("create expected stage");
        fs::create_dir_all(&observed).expect("create observed stage");
        for name in APP_FILES {
            fs::write(expected.join(name), name).expect("write expected app file");
            fs::write(observed.join(name), name).expect("write observed app file");
        }

        compare_stage_bytes(&expected, &observed).expect("identical stages agree byte for byte");

        // One case per staged asset, so a comparison that only ever read the
        // first entry would be caught rather than pass on the rest.
        for name in APP_FILES {
            fs::write(observed.join(name), "foreign-metadata").expect("rewrite observed app file");

            compare_stage_inventory(&expected, &observed)
                .expect("differing bytes must not change the inventory answer");
            assert!(
                matches!(
                    compare_stage_bytes(&expected, &observed),
                    Err(AppBuildError::Drift(drifted)) if drifted == name
                ),
                "{name} drifted without being named as the drifted asset",
            );

            fs::write(observed.join(name), name).expect("restore observed app file");
        }

        fs::remove_dir_all(root).expect("remove fixture");
    }

    /// The refusal has to name both hosts, because the operator's next move
    /// depends on which environment they are missing, not on the fact that
    /// they are missing one.
    #[test]
    fn staging_from_a_foreign_host_refuses_and_names_both_environments() {
        let refusal = AppBuildError::ForeignStagingHost("aarch64-macos".to_owned()).to_string();

        assert!(
            refusal.contains("aarch64-macos") && refusal.contains(STAGING_HOST),
            "the refusal named neither the observed host nor the owning one: {refusal}",
        );
        assert_ne!(
            host_spelling(),
            String::new(),
            "a host with no spelling could never be compared to the owning one",
        );
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
        runner.readiness = Some(Err(succession::fixture_error()));
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
