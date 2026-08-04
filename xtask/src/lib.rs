//! Repository-local development commands.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use wire::{Digest, Domain};

mod commit_message;
mod landing;
mod reference_transaction;
mod release;

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
const STAGING_HOST: &str = "x86_64-unknown-linux-gnu";

/// Whether this program was compiled for the tuple [`STAGING_HOST`] names.
///
/// This is the only test that decides ownership, and it admits one tuple or
/// none. A triple's vendor and environment fields are observable after all --
/// `std::env::consts` simply stops at arch and OS, and stopping there spells a
/// musl host and the image's gnu host identically while their `-Cmetadata`
/// differs. Every field the triple names is asserted here, so a near miss is a
/// refusal rather than a silent admission.
const fn compiled_for_staging_host() -> bool {
    cfg!(all(
        target_arch = "x86_64",
        target_vendor = "unknown",
        target_os = "linux",
        target_env = "gnu",
    ))
}

/// This host, named for a reader rather than for the comparison above.
///
/// Spelled in the same four fields as [`STAGING_HOST`] so the mismatch reads as
/// the tuple comparison it is, and the environment field, which is the one that
/// separates the owning host from its nearest neighbour, is legible. Nothing
/// decides on this string: a host it cannot spell exactly still refuses,
/// because [`compiled_for_staging_host`] answered first.
fn host_spelling() -> String {
    let vendor = if cfg!(target_vendor = "apple") {
        "apple"
    } else if cfg!(target_vendor = "unknown") {
        "unknown"
    } else {
        "other"
    };
    let environment = if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "") {
        "none"
    } else {
        "other"
    };
    format!(
        "{}-{vendor}-{}-{environment}",
        std::env::consts::ARCH,
        std::env::consts::OS,
    )
}

const APP_FILES: [&str; 4] = ["app.module", "app_bg.wasm", "canvas-app.html", "style.css"];
const RELEASE_STAGE_DIRECTORIES: [&str; 2] = ["objects", "targets"];
const GENERATED_MODULE_REFERENCES: usize = 2;

/// Everything the staged app bytes are built from, named rather than derived.
///
/// A directory stands for every regular file beneath it. `app` and `wire` are
/// the only workspace crates the browser build compiles, and `Cargo.lock` pins
/// every registry crate they link; a test refuses either manifest declaring a
/// path dependency this list does not already cover, so the closure cannot grow
/// silently. `app/tests` and `app/browser` are absent because no staged byte is
/// built from them.
const STAGE_SOURCE_INPUTS: [&str; 7] = [
    "Cargo.lock",
    "app/Cargo.toml",
    "app/index.html",
    "app/src",
    "app/style.css",
    "wire/Cargo.toml",
    "wire/src",
];

/// Where the identity of those inputs is recorded, beside the stage it explains.
const STAGE_SOURCE_LISTING: &str = "dist/stage-sources.txt";

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

/// Compile the browser app with the pinned Trunk adapter and replace the
/// committed stage with exactly those generated assets.
///
/// Only the staging host may write the stage: a rebuild from anywhere else
/// replaces bytes the release battery reproduces with bytes it cannot. The
/// identity of what was compiled is recorded beside the stage in the same
/// breath, because this is the one run that knows it first-hand.
///
/// # Errors
///
/// Returns a foreign staging host, a missing Cargo home, Trunk execution
/// failure, a refused generated stage, or an unreadable build input.
pub fn stage_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    if !compiled_for_staging_host() {
        return Err(AppBuildError::ForeignStagingHost(host_spelling()));
    }
    let root = workspace_root()?;
    let stage = root.join("dist/stage");
    run_trunk(&root, environment, &stage, output)?;
    record_stage_sources(&root)
}

/// Rebuild the browser app outside the committed stage and compare it back.
///
/// Every host first asks whether the stage was built from the sources now in
/// the tree, which is answered from recorded input identity and needs no
/// rebuild, then rebuilds and compares the staged inventory. Only the staging
/// host goes on to compare bytes, because only its compiler metadata can
/// reproduce them.
///
/// # Errors
///
/// Returns build inputs the recorded stage identity no longer describes, a
/// missing Cargo home, Trunk execution failure, an inventory that differs from
/// the rebuild, or, on the staging host, the first staged asset whose bytes
/// differ.
pub fn check_app(
    environment: &ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<(), AppBuildError> {
    let root = workspace_root()?;
    compare_stage_sources(&root)?;
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
        if compiled_for_staging_host() {
            return compare_stage_bytes(&stage, &temporary);
        }
        let host = host_spelling();
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

/// Record beside the stage the identity of the inputs this run compiled.
fn record_stage_sources(root: &Path) -> Result<(), AppBuildError> {
    let listing = stage_source_listing(root)?;
    let path = root.join(STAGE_SOURCE_LISTING);
    fs::write(&path, listing).map_err(|source| AppBuildError::Io { path, source })
}

/// Refuse a stage whose recorded inputs are not the inputs now in the tree.
///
/// This is the whole of what a host that cannot reproduce the staged bytes can
/// still settle about them, and it settles it without compiling anything: the
/// recorded listing either describes the current sources or it does not. An
/// absent listing is a refusal rather than a pass, because a stage that cannot
/// say what built it is the stage this check exists to catch.
fn compare_stage_sources(root: &Path) -> Result<(), AppBuildError> {
    let path = root.join(STAGE_SOURCE_LISTING);
    let recorded =
        fs::read_to_string(&path).map_err(|source| AppBuildError::Io { path, source })?;
    let stale = disagreeing_source_paths(&recorded, &stage_source_listing(root)?);
    if stale.is_empty() {
        return Ok(());
    }
    Err(AppBuildError::StaleStage(stale))
}

/// The content identity of every staged-app build input, one file per line.
///
/// A line is a file digest and the repository-relative path it covers, in path
/// order. The listing is kept whole rather than folded into one digest so that
/// a refusal can name the files that moved, and so a reader can see which
/// inputs are claimed without running anything.
fn stage_source_listing(root: &Path) -> Result<String, AppBuildError> {
    let mut paths = Vec::new();
    for input in STAGE_SOURCE_INPUTS {
        collect_source_paths(root, input, &mut paths)?;
    }
    paths.sort();
    let mut listing = String::new();
    for path in paths {
        let file = root.join(&path);
        let bytes = fs::read(&file).map_err(|source| AppBuildError::Io { path: file, source })?;
        let _ = writeln!(
            listing,
            "{} {path}",
            Digest::compute(Domain::GateFile, &bytes)
        );
    }
    Ok(listing)
}

/// Every regular file at or beneath one named input, repository-relative.
///
/// The relative path is carried down rather than stripped back off afterwards,
/// so the name recorded for a file is built the same way on every host and no
/// step can fail in a way that has to be interpreted. Anything that is neither
/// a regular file nor a directory refuses, because the stage is built from
/// bytes and only those two shapes say which bytes.
fn collect_source_paths(
    root: &Path,
    relative: &str,
    found: &mut Vec<String>,
) -> Result<(), AppBuildError> {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|source| AppBuildError::Io {
        path: path.clone(),
        source,
    })?;
    if metadata.is_file() {
        found.push(relative.to_owned());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(AppBuildError::SourceInput(path));
    }
    for entry in fs::read_dir(&path).map_err(|source| AppBuildError::Io {
        path: path.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| AppBuildError::Io {
            path: path.clone(),
            source,
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| AppBuildError::NonUtf8(entry.path()))?;
        collect_source_paths(root, &format!("{relative}/{name}"), found)?;
    }
    Ok(())
}

/// Which build inputs two listings disagree about, in path order.
///
/// A stage goes stale by an input changing, arriving, or leaving, and all three
/// are answered the same way: a path is stale when its recorded digest is not
/// its observed one, with absence on either side counting as a difference. A
/// line that is not a digest and a path is dropped, which leaves the path it
/// should have covered disagreeing rather than quietly agreeing.
fn disagreeing_source_paths(recorded: &str, observed: &str) -> Vec<String> {
    let read = |listing: &str| -> BTreeMap<String, String> {
        listing
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(digest, path)| (path.to_owned(), digest.to_owned()))
            .collect()
    };
    let recorded = read(recorded);
    let observed = read(observed);
    let mut paths = recorded
        .keys()
        .chain(observed.keys())
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths.retain(|path| recorded.get(path) != observed.get(path));
    paths
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
    SourceInput(PathBuf),
    StaleStage(Vec<String>),
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
                "the app stage is owned by {STAGING_HOST} builds in image {} and this host \
                 is {host}: staging here would replace bytes the sealed image reproduces \
                 with bytes no other environment can",
                proof::DEFAULT_IMAGE
            ),
            Self::GeneratedStage(reason) => {
                write!(formatter, "generated app stage refused: {reason}")
            }
            Self::Inventory { expected, observed } => write!(
                formatter,
                "staged app inventory differs: committed={expected:?}, rebuilt={observed:?}"
            ),
            Self::Drift(path) => write!(formatter, "staged app drifted at {path}"),
            Self::SourceInput(path) => write!(
                formatter,
                "a staged app build input is neither a regular file nor a directory: {}",
                path.display()
            ),
            Self::StaleStage(paths) => write!(
                formatter,
                "the committed app stage was not built from the sources now in the tree: \
                 {} changed since {STAGE_SOURCE_LISTING} recorded them; restage with \
                 cargo xtask stage-app in image {}, which is the one environment allowed \
                 to write the stage",
                paths.join(", "),
                proof::DEFAULT_IMAGE
            ),
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
    use std::collections::{BTreeMap, VecDeque};

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
    /// they are missing one. It names the image too, because an operator on an
    /// x86_64 Linux box is missing an image and not an architecture, and the
    /// digest is the only thing they can act on.
    #[test]
    fn staging_from_a_foreign_host_refuses_and_names_both_environments() {
        let refusal =
            AppBuildError::ForeignStagingHost("aarch64-apple-macos-none".to_owned()).to_string();

        assert!(
            refusal.contains("aarch64-apple-macos-none") && refusal.contains(STAGING_HOST),
            "the refusal named neither the observed host nor the owning one: {refusal}",
        );
        assert!(
            refusal.contains(proof::DEFAULT_IMAGE),
            "the refusal named no image to stage from: {refusal}",
        );
    }

    /// An x86_64 Linux host that is not the image spells its arch and OS
    /// exactly as the image does, so ownership cannot be decided on those two
    /// fields. Requiring all four is what separates the image's gnu tuple from
    /// a musl one, and the spelling has to agree with the decision or the
    /// mismatch message would name a tuple the guard did not compare.
    #[test]
    fn staging_ownership_is_decided_on_the_whole_tuple() {
        assert_eq!(
            STAGING_HOST.split('-').collect::<Vec<_>>(),
            ["x86_64", "unknown", "linux", "gnu"],
            "the owning host is spelled {STAGING_HOST}; every field is named here because a \
             count would survive dropping the environment, which is the field that separates \
             this tuple from a musl one",
        );
        assert_eq!(
            compiled_for_staging_host(),
            host_spelling() == STAGING_HOST,
            "the guard and the spelling disagree about this host: {}",
            host_spelling(),
        );
    }

    /// A stage goes stale three ways, and a comparison answering only one of
    /// them would still pass on the other two. Each class is spelled with its
    /// own path, so an answer that collapsed them cannot look complete.
    #[test]
    fn every_way_a_build_input_can_move_is_reported() {
        let recorded = "aaa app/src/lib.rs\nbbb app/style.css\nccc wire/src/lib.rs\n";
        let observed = "aaa app/src/lib.rs\nzzz app/style.css\nddd app/index.html\n";

        assert_eq!(
            disagreeing_source_paths(recorded, observed),
            ["app/index.html", "app/style.css", "wire/src/lib.rs"],
            "an input that arrived, changed, or left went unreported",
        );
        assert!(
            disagreeing_source_paths(recorded, recorded).is_empty(),
            "a listing disagreed with itself",
        );
    }

    /// The input list is written out rather than derived, so it is only
    /// complete while `app` and `wire` are the only workspace crates the
    /// browser build compiles. A third one would be linked into the stage and
    /// digested by nothing, which is a stale stage this check would call fresh.
    #[test]
    fn the_staged_app_closure_declares_no_path_dependency_off_the_list() {
        let mut declared = Vec::new();
        for manifest in ["app/Cargo.toml", "wire/Cargo.toml"] {
            let text = fs::read_to_string(repository_root().join(manifest))
                .unwrap_or_else(|error| panic!("{manifest}: {error}"));
            for line in text.lines() {
                if let Some((_, rest)) = line.split_once("path = \"") {
                    let (value, _) = rest
                        .split_once('"')
                        .unwrap_or_else(|| panic!("{manifest} left a path unterminated: {line}"));
                    declared.push(format!("{manifest} {value}"));
                }
            }
        }

        assert_eq!(
            declared,
            ["app/Cargo.toml ../wire"],
            "the staged app closure grew a workspace crate; add its sources to \
             STAGE_SOURCE_INPUTS before admitting it here",
        );
    }

    /// The listing beside the stage asserts something about bytes compiled in
    /// another environment, and it is worth nothing unless it is true of this
    /// tree. This is the call `cargo xtask check-app` makes first, made here
    /// without the rebuild that follows it.
    #[test]
    fn the_recorded_stage_identity_describes_the_committed_sources() {
        if let Err(error) = compare_stage_sources(&repository_root()) {
            panic!("{error}");
        }
    }

    /// The refusal is all a host that cannot reproduce the bytes ever sees, so
    /// it carries the whole answer: what moved, where the claim it broke is
    /// recorded, and the one command and environment that can renew it.
    #[test]
    fn a_stale_stage_names_what_moved_and_how_to_renew_it() {
        let refusal = AppBuildError::StaleStage(vec!["wire/src/ident.rs".to_owned()]).to_string();

        for expected in [
            "wire/src/ident.rs",
            STAGE_SOURCE_LISTING,
            "cargo xtask stage-app",
            proof::DEFAULT_IMAGE,
        ] {
            assert!(
                refusal.contains(expected),
                "the refusal never mentioned {expected}: {refusal}",
            );
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

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the xtask manifest directory sits inside the repository")
            .to_path_buf()
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
