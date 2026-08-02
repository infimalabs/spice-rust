//! Concrete, bounded target-build adapter for the distribution pipeline.

use std::cell::RefCell;
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dist::{
    BuildError, Builder, FilePublisher, Pipeline, PublishError, Publisher, RequiredPins,
    StagedTarget,
};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use wire::manifest::{BuildPins, ReleaseTarget};
use wire::{Digest, Domain};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const BUILD_RUNTIME: Duration = Duration::from_secs(60 * 60);
const OUTPUT_DRAIN: Duration = Duration::from_secs(10);
const REAP: Duration = Duration::from_secs(10);
const GRACE: Duration = Duration::from_secs(5);
const WASM_TARGET: &str = "wasm32-unknown-unknown";
const PINNED_TRUNK: &str = "trunk 0.21.14";
const PINNED_MUTANTS: &str = "cargo-mutants 27.1.0";
const PINNED_PROOF_IMAGE: &str =
    "sha256:2f1f7a4c9e6d0b835a1c47e9d20b6f83c15d9a047e6b3a92c8d05e714f6a9b3c";
const APP_ARTIFACT: &str = "dist/stage/app_bg.wasm";
const EMBEDDED_ARTIFACT: &str = "dist/stage/canvas-app.html";

/// One complete bounded command observation from a target build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRecord {
    pub label: &'static str,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandRecord {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// One target payload and every command observation that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetBuild {
    pub target: ReleaseTarget,
    pub source: Digest,
    pub staged: StagedTarget,
    pub commands: Vec<CommandRecord>,
}

/// Ambient process values acquired once by the xtask binary rim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseEnvironment {
    path: Option<OsString>,
    home: Option<OsString>,
    cargo_home: Option<OsString>,
    rustup_home: Option<OsString>,
    temporary: Option<OsString>,
    sdk_root: Option<OsString>,
    developer_dir: Option<OsString>,
}

impl ReleaseEnvironment {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        path: Option<OsString>,
        home: Option<OsString>,
        cargo_home: Option<OsString>,
        rustup_home: Option<OsString>,
        temporary: Option<OsString>,
        sdk_root: Option<OsString>,
        developer_dir: Option<OsString>,
    ) -> Self {
        Self {
            path,
            home,
            cargo_home,
            rustup_home,
            temporary,
            sdk_root,
            developer_dir,
        }
    }
}

/// Build and atomically stage one supported release target.
///
/// The command refuses missing or drifted tools and source state before the
/// first build. The source is checked again immediately before the target
/// payload pointer is published.
pub fn stage_target(
    spelling: &str,
    environment: ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<TargetBuild, TargetBuildError> {
    let target = parse_target(spelling)?;
    let root = workspace_root()?;
    let programs = Programs::discover(&environment)?;
    let executor = SystemExecutor::new(&root, &environment)?;
    match stage_with(&root, target, &programs, &executor) {
        Ok(build) => {
            render_records(output, &build)?;
            Ok(build)
        }
        Err(error) => {
            render_command_records(output, &executor.records())?;
            Err(error)
        }
    }
}

fn stage_with<E: Executor>(
    root: &Path,
    target: ReleaseTarget,
    programs: &Programs,
    executor: &E,
) -> Result<TargetBuild, TargetBuildError> {
    let source = SourceGuard::capture(executor, &programs.git)?;
    let required = required_pins();
    let observed = observe_pins(executor, programs, target)?;
    let required = RequiredPins::new(required)?;
    required.verify(&observed)?;

    let pipeline = Pipeline::new(required.clone());
    let target_dir = root.join("target/release-appliance");
    let mut builder = CommandBuilder {
        root,
        target,
        target_dir: &target_dir,
        programs,
        executor,
    };
    let mut publisher = GuardedPublisher {
        inner: FilePublisher::new(root.join("dist/stage")),
        source: &source,
        required: &required,
        programs,
        target,
        executor,
    };
    let staged = pipeline.stage(
        target,
        source.digest.clone(),
        &observed,
        &mut builder,
        &mut publisher,
    )?;
    Ok(TargetBuild {
        target,
        source: source.digest,
        staged,
        commands: executor.records(),
    })
}

fn parse_target(spelling: &str) -> Result<ReleaseTarget, TargetBuildError> {
    match spelling {
        "aarch64-apple-darwin" => Ok(ReleaseTarget::Aarch64AppleDarwin),
        "x86_64-unknown-linux-musl" => Ok(ReleaseTarget::X86_64UnknownLinuxMusl),
        _ => Err(TargetBuildError::UnsupportedTarget(spelling.to_owned())),
    }
}

fn required_pins() -> BuildPins {
    BuildPins {
        rust: format!("rustc {}", env!("CARGO_PKG_RUST_VERSION")),
        wasm_target: WASM_TARGET.to_owned(),
        trunk: PINNED_TRUNK.to_owned(),
        cargo_mutants: PINNED_MUTANTS.to_owned(),
        proof_container: PINNED_PROOF_IMAGE.to_owned(),
    }
}

fn observe_pins<E: Executor>(
    executor: &E,
    programs: &Programs,
    target: ReleaseTarget,
) -> Result<BuildPins, TargetBuildError> {
    let rust = successful(executor.execute(CommandSpec::new(
        "rust-version",
        &programs.rustc,
        ["--version"],
    ))?)?;
    let rust = first_words(&rust.stdout, 2, "rust version")?;
    let targets = successful(executor.execute(CommandSpec::new(
        "installed-targets",
        &programs.rustup,
        ["target", "list", "--installed"],
    ))?)?;
    let targets = text(&targets.stdout, "installed Rust targets")?;
    for required in [WASM_TARGET, target.as_str()] {
        if !targets.lines().any(|line| line == required) {
            return Err(TargetBuildError::TargetNotInstalled(required.to_owned()));
        }
    }
    let trunk = successful(executor.execute(CommandSpec::new(
        "trunk-version",
        &programs.trunk,
        ["--version"],
    ))?)?;
    let mutants = successful(executor.execute(CommandSpec::new(
        "mutants-version",
        &programs.mutants,
        ["mutants", "--version"],
    ))?)?;
    Ok(BuildPins {
        rust,
        wasm_target: WASM_TARGET.to_owned(),
        trunk: one_line(&trunk.stdout, "Trunk version")?,
        cargo_mutants: one_line(&mutants.stdout, "cargo-mutants version")?,
        proof_container: PINNED_PROOF_IMAGE.to_owned(),
    })
}

fn first_words(
    bytes: &[u8],
    count: usize,
    label: &'static str,
) -> Result<String, TargetBuildError> {
    let value = one_line(bytes, label)?;
    let words = value
        .split_ascii_whitespace()
        .take(count)
        .collect::<Vec<_>>();
    if words.len() != count {
        return Err(TargetBuildError::Observation {
            label,
            observed: value,
        });
    }
    Ok(words.join(" "))
}

fn one_line(bytes: &[u8], label: &'static str) -> Result<String, TargetBuildError> {
    let value = text(bytes, label)?.trim_end_matches(['\n', '\r']);
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(TargetBuildError::Observation {
            label,
            observed: value.to_owned(),
        });
    }
    Ok(value.to_owned())
}

fn text<'a>(bytes: &'a [u8], label: &'static str) -> Result<&'a str, TargetBuildError> {
    std::str::from_utf8(bytes).map_err(|_| TargetBuildError::Observation {
        label,
        observed: "non-UTF-8 output".to_owned(),
    })
}

struct CommandBuilder<'a, E> {
    root: &'a Path,
    target: ReleaseTarget,
    target_dir: &'a Path,
    programs: &'a Programs,
    executor: &'a E,
}

impl<E: Executor> Builder for CommandBuilder<'_, E> {
    fn build_app(&mut self) -> Result<Vec<u8>, BuildError> {
        self.run(CommandSpec::new(
            "app-build",
            &self.programs.xtask,
            ["check-app"],
        ))?;
        read_regular(&self.root.join(APP_ARTIFACT)).map_err(build_error)
    }

    fn embed_app(&mut self, app: &[u8]) -> Result<Vec<u8>, BuildError> {
        let observed = read_regular(&self.root.join(APP_ARTIFACT)).map_err(build_error)?;
        if observed != app {
            return Err(BuildError::new(
                "staged app changed between app and embedding commands",
            ));
        }
        self.run(CommandSpec::new(
            "embed-build",
            &self.programs.cargo,
            cargo_build_arguments("canvas", self.target, self.target_dir),
        ))?;
        read_regular(&self.root.join(EMBEDDED_ARTIFACT)).map_err(build_error)
    }

    fn build_native(
        &mut self,
        target: ReleaseTarget,
        embedded_app: &[u8],
    ) -> Result<Vec<u8>, BuildError> {
        if target != self.target {
            return Err(BuildError::new(format!(
                "builder target {} does not match requested {target}",
                self.target
            )));
        }
        let observed = read_regular(&self.root.join(EMBEDDED_ARTIFACT)).map_err(build_error)?;
        if observed != embedded_app {
            return Err(BuildError::new(
                "embedded app changed between embedding and native commands",
            ));
        }
        self.run(CommandSpec::new(
            "native-build",
            &self.programs.cargo,
            cargo_build_arguments("spice", target, self.target_dir),
        ))?;
        read_regular(&self.target_dir.join(target.as_str()).join("release/spice"))
            .map_err(build_error)
    }
}

fn cargo_build_arguments(package: &str, target: ReleaseTarget, target_dir: &Path) -> Vec<OsString> {
    [
        OsString::from("build"),
        OsString::from("--locked"),
        OsString::from("--release"),
        OsString::from("-p"),
        OsString::from(package),
        OsString::from("--target"),
        OsString::from(target.as_str()),
        OsString::from("--target-dir"),
        target_dir.as_os_str().to_owned(),
    ]
    .into()
}

impl<E: Executor> CommandBuilder<'_, E> {
    fn run(&self, command: CommandSpec) -> Result<(), BuildError> {
        let record = self.executor.execute(command).map_err(build_error)?;
        successful(record).map(|_| ()).map_err(build_error)
    }
}

fn build_error(error: impl fmt::Display) -> BuildError {
    BuildError::new(error.to_string())
}

struct SourceGuard {
    commit: String,
    digest: Digest,
    git: PathBuf,
}

impl SourceGuard {
    fn capture<E: Executor>(executor: &E, git: &Path) -> Result<Self, TargetBuildError> {
        let commit = capture_commit(executor, git)?;
        verify_checkout(executor, git)?;
        let digest = Digest::compute(Domain::DistSourceCommit, commit.as_bytes());
        Ok(Self {
            commit,
            digest,
            git: git.to_path_buf(),
        })
    }

    fn verify<E: Executor>(&self, executor: &E) -> Result<(), TargetBuildError> {
        let observed = capture_commit(executor, &self.git)?;
        if observed != self.commit {
            return Err(TargetBuildError::SourceMoved {
                expected: self.commit.clone(),
                observed,
            });
        }
        verify_checkout(executor, &self.git)
    }
}

fn capture_commit<E: Executor>(executor: &E, git: &Path) -> Result<String, TargetBuildError> {
    let record = successful(executor.execute(CommandSpec::new(
        "source-commit",
        git,
        ["rev-parse", "--verify", "HEAD^{commit}"],
    ))?)?;
    let commit = one_line(&record.stdout, "Git source commit")?;
    if !matches!(commit.len(), 40 | 64)
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TargetBuildError::Observation {
            label: "Git source commit",
            observed: commit,
        });
    }
    Ok(commit)
}

fn verify_checkout<E: Executor>(executor: &E, git: &Path) -> Result<(), TargetBuildError> {
    let tracked = executor.execute(CommandSpec::new(
        "source-tracked",
        git,
        ["diff-index", "--quiet", "HEAD", "--"],
    ))?;
    if !tracked.success() {
        return Err(TargetBuildError::SourceDirty(
            "tracked source differs from HEAD".to_owned(),
        ));
    }
    let untracked = successful(executor.execute(CommandSpec::new(
        "source-untracked",
        git,
        ["ls-files", "--others", "--exclude-standard"],
    ))?)?;
    let untracked = text(&untracked.stdout, "untracked source inventory")?;
    let unexpected = untracked.lines().find(|path| {
        !path.starts_with("dist/stage/objects/") && !path.starts_with("dist/stage/targets/")
    });
    if let Some(path) = unexpected {
        return Err(TargetBuildError::SourceDirty(format!(
            "untracked source path is outside release output: {path}"
        )));
    }
    Ok(())
}

struct GuardedPublisher<'a, E> {
    inner: FilePublisher,
    source: &'a SourceGuard,
    required: &'a RequiredPins,
    programs: &'a Programs,
    target: ReleaseTarget,
    executor: &'a E,
}

impl<E: Executor> Publisher for GuardedPublisher<'_, E> {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), PublishError> {
        if path.starts_with("targets") && path.file_name() == Some(OsStr::new("payload.json")) {
            self.source
                .verify(self.executor)
                .map_err(|error| PublishError::Guard {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })?;
            let observed =
                observe_pins(self.executor, self.programs, self.target).map_err(|error| {
                    PublishError::Guard {
                        path: path.to_path_buf(),
                        reason: error.to_string(),
                    }
                })?;
            self.required
                .verify(&observed)
                .map_err(|error| PublishError::Guard {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })?;
        }
        self.inner.publish(path, mode, bytes)
    }
}

#[derive(Clone)]
struct Programs {
    cargo: PathBuf,
    rustc: PathBuf,
    rustup: PathBuf,
    trunk: PathBuf,
    mutants: PathBuf,
    git: PathBuf,
    xtask: PathBuf,
}

impl Programs {
    fn discover(environment: &ReleaseEnvironment) -> Result<Self, TargetBuildError> {
        let path = environment
            .path
            .as_deref()
            .ok_or(TargetBuildError::MissingProgram("PATH"))?;
        Ok(Self {
            cargo: find_program(path, "cargo")?,
            rustc: find_program(path, "rustc")?,
            rustup: find_program(path, "rustup")?,
            trunk: find_program(path, "trunk")?,
            mutants: find_program(path, "cargo-mutants")?,
            git: find_program(path, "git")?,
            xtask: env::current_exe().map_err(|source| TargetBuildError::Io {
                path: PathBuf::from("current xtask executable"),
                source,
            })?,
        })
    }
}

fn find_program(path: &OsStr, name: &'static str) -> Result<PathBuf, TargetBuildError> {
    for directory in env::split_paths(path) {
        let candidate = directory.join(name);
        if executable(&candidate) {
            let directory = if directory.as_os_str().is_empty() {
                Path::new(".")
            } else {
                &directory
            };
            return fs::canonicalize(directory)
                .map(|directory| directory.join(name))
                .map_err(|source| TargetBuildError::Io {
                    path: directory.to_path_buf(),
                    source,
                });
        }
    }
    Err(TargetBuildError::MissingProgram(name))
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(path: &Path) -> bool {
    path.is_file()
}

#[derive(Clone)]
struct CommandSpec {
    label: &'static str,
    program: PathBuf,
    arguments: Vec<OsString>,
}

impl CommandSpec {
    fn new<I, S>(label: &'static str, program: &Path, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            label,
            program: program.to_path_buf(),
            arguments: arguments.into_iter().map(Into::into).collect(),
        }
    }
}

trait Executor {
    fn execute(&self, command: CommandSpec) -> Result<CommandRecord, TargetBuildError>;
    fn records(&self) -> Vec<CommandRecord>;
}

struct SystemExecutor {
    root: PathBuf,
    environment: Environment,
    records: RefCell<Vec<CommandRecord>>,
}

impl SystemExecutor {
    fn new(root: &Path, release: &ReleaseEnvironment) -> Result<Self, TargetBuildError> {
        let mut entries = Vec::<(OsString, OsString)>::new();
        for (key, value) in [
            ("PATH", release.path.as_ref()),
            ("HOME", release.home.as_ref()),
            ("RUSTUP_HOME", release.rustup_home.as_ref()),
            ("TMPDIR", release.temporary.as_ref()),
            ("SDKROOT", release.sdk_root.as_ref()),
            ("DEVELOPER_DIR", release.developer_dir.as_ref()),
        ] {
            if let Some(value) = value {
                entries.push((key.into(), value.clone()));
            }
        }
        let cargo_home = release.cargo_home.clone().or_else(|| {
            release
                .home
                .as_ref()
                .map(|home| PathBuf::from(home).join(".cargo").into_os_string())
        });
        if let Some(cargo_home) = cargo_home {
            entries.push(("CARGO_HOME".into(), cargo_home));
        }
        entries.extend([
            (
                OsString::from("CARGO_BUILD_BUILD_DIR"),
                root.join("target/release-appliance-build").into_os_string(),
            ),
            (OsString::from("CARGO_INCREMENTAL"), OsString::from("0")),
            (OsString::from("CARGO_NET_OFFLINE"), OsString::from("true")),
            (OsString::from("NO_COLOR"), OsString::from("true")),
            (
                OsString::from("CARGO_ENCODED_RUSTFLAGS"),
                remapped_rustflags(root)?.into(),
            ),
            (OsString::from("RUSTC_WRAPPER"), OsString::new()),
            (OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new()),
        ]);
        Ok(Self {
            root: root.to_path_buf(),
            environment: Environment::new(entries)?,
            records: RefCell::new(Vec::new()),
        })
    }
}

impl Executor for SystemExecutor {
    fn execute(&self, command: CommandSpec) -> Result<CommandRecord, TargetBuildError> {
        let spec = Spec {
            argv: Argv::new(
                command.program.as_os_str().to_owned(),
                command.arguments.clone(),
            )?,
            cwd: self.root.clone(),
            environment: self.environment.clone(),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(BUILD_RUNTIME, OUTPUT_DRAIN, REAP)?,
            wait: WaitPolicy::new(
                format!("release target {}", command.label),
                Cancellation::Never,
                RestartDisposition::Never,
            )?,
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture {
                    limit: OUTPUT_LIMIT,
                },
                StreamPolicy::Capture {
                    limit: OUTPUT_LIMIT,
                },
            ),
            redaction: Redaction::None,
            grace: GRACE,
        };
        let running = sill::spawn(spec).map_err(|source| TargetBuildError::Launch {
            label: command.label,
            source,
        })?;
        let outcome = running.wait().map_err(|source| TargetBuildError::Wait {
            label: command.label,
            source,
        })?;
        if !matches!(outcome.completion(), Completion::Exited(_)) {
            return Err(TargetBuildError::Terminated(command.label));
        }
        let stdout = complete_capture(command.label, "stdout", outcome.stdout())?;
        let stderr = complete_capture(command.label, "stderr", outcome.stderr())?;
        let record = CommandRecord {
            label: command.label,
            program: command.program,
            arguments: command.arguments,
            code: outcome.completion().status().code(),
            stdout,
            stderr,
        };
        self.records.borrow_mut().push(record.clone());
        Ok(record)
    }

    fn records(&self) -> Vec<CommandRecord> {
        self.records.borrow().clone()
    }
}

fn complete_capture(
    label: &'static str,
    stream: &'static str,
    capture: Option<&sill::CapturedOutput>,
) -> Result<Vec<u8>, TargetBuildError> {
    let Some(capture) = capture else {
        return Err(TargetBuildError::IncompleteOutput { label, stream });
    };
    if capture.truncated() || !capture.complete() {
        return Err(TargetBuildError::IncompleteOutput { label, stream });
    }
    Ok(capture.bytes().to_vec())
}

fn successful(record: CommandRecord) -> Result<CommandRecord, TargetBuildError> {
    if record.success() {
        Ok(record)
    } else {
        Err(TargetBuildError::CommandFailed(record))
    }
}

fn read_regular(path: &Path) -> Result<Vec<u8>, TargetBuildError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| TargetBuildError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(TargetBuildError::Artifact(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| TargetBuildError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remapped_rustflags(root: &Path) -> Result<String, TargetBuildError> {
    let root = root
        .to_str()
        .ok_or_else(|| TargetBuildError::Artifact(root.to_path_buf()))?;
    Ok(format!("--remap-path-prefix={root}=/workspace"))
}

fn workspace_root() -> Result<PathBuf, TargetBuildError> {
    let root = env::current_dir().map_err(|source| TargetBuildError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    if !root.join("Cargo.toml").is_file() || !root.join("dist/stage").is_dir() {
        return Err(TargetBuildError::Workspace(root));
    }
    Ok(root)
}

fn render_records(output: &mut impl Write, build: &TargetBuild) -> Result<(), TargetBuildError> {
    render_command_records(output, &build.commands)?;
    writeln!(
        output,
        "[release] target={} source={} payload={} path={}",
        build.target,
        build.source,
        build.staged.payload_digest,
        build.staged.payload_path.display()
    )
    .map_err(TargetBuildError::Output)
}

fn render_command_records(
    output: &mut impl Write,
    records: &[CommandRecord],
) -> Result<(), TargetBuildError> {
    for record in records {
        writeln!(
            output,
            "[release:{}] {} {:?} exit={:?}",
            record.label,
            record.program.display(),
            record.arguments,
            record.code
        )
        .map_err(TargetBuildError::Output)?;
        output
            .write_all(&record.stdout)
            .map_err(TargetBuildError::Output)?;
        output
            .write_all(&record.stderr)
            .map_err(TargetBuildError::Output)?;
    }
    Ok(())
}

/// Why the concrete target build did not publish a current pointer.
#[derive(Debug)]
pub enum TargetBuildError {
    UnsupportedTarget(String),
    TargetNotInstalled(String),
    MissingProgram(&'static str),
    Workspace(PathBuf),
    Artifact(PathBuf),
    Observation {
        label: &'static str,
        observed: String,
    },
    SourceDirty(String),
    SourceMoved {
        expected: String,
        observed: String,
    },
    Launch {
        label: &'static str,
        source: io::Error,
    },
    Wait {
        label: &'static str,
        source: sill::WaitError,
    },
    Terminated(&'static str),
    IncompleteOutput {
        label: &'static str,
        stream: &'static str,
    },
    CommandFailed(CommandRecord),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Output(io::Error),
    Spec(sill::SpecError),
    Pins(dist::PinError),
    Stage(dist::StageError),
}

impl fmt::Display for TargetBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTarget(target) => {
                write!(formatter, "unsupported release target {target:?}")
            }
            Self::TargetNotInstalled(target) => {
                write!(formatter, "required Rust target is not installed: {target}")
            }
            Self::MissingProgram(program) => {
                write!(formatter, "required build program is missing: {program}")
            }
            Self::Workspace(path) => write!(
                formatter,
                "not the Spice workspace root: {}",
                path.display()
            ),
            Self::Artifact(path) => write!(
                formatter,
                "build artifact is not one regular file: {}",
                path.display()
            ),
            Self::Observation { label, observed } => {
                write!(formatter, "{label} observation is invalid: {observed:?}")
            }
            Self::SourceDirty(reason) => {
                write!(formatter, "release source is not immutable: {reason}")
            }
            Self::SourceMoved { expected, observed } => write!(
                formatter,
                "release source moved: expected {expected}, observed {observed}"
            ),
            Self::Launch { label, source } => write!(formatter, "{label} did not launch: {source}"),
            Self::Wait { label, source } => write!(formatter, "{label} wait failed: {source}"),
            Self::Terminated(label) => write!(formatter, "{label} did not exit normally"),
            Self::IncompleteOutput { label, stream } => write!(
                formatter,
                "{label} {stream} exceeded or escaped its capture bound"
            ),
            Self::CommandFailed(record) => write!(
                formatter,
                "{} failed with exit {:?}: stdout={:?} stderr={:?}",
                record.label,
                record.code,
                String::from_utf8_lossy(&record.stdout),
                String::from_utf8_lossy(&record.stderr)
            ),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Output(source) => write!(formatter, "release build report failed: {source}"),
            Self::Spec(source) => write!(
                formatter,
                "release process specification is invalid: {source}"
            ),
            Self::Pins(source) => write!(formatter, "release pin refused: {source}"),
            Self::Stage(source) => write!(formatter, "release stage failed: {source}"),
        }
    }
}

impl Error for TargetBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Launch { source, .. } | Self::Io { source, .. } | Self::Output(source) => {
                Some(source)
            }
            Self::Wait { source, .. } => Some(source),
            Self::Spec(source) => Some(source),
            Self::Pins(source) => Some(source),
            Self::Stage(source) => Some(source),
            _ => None,
        }
    }
}

impl From<sill::SpecError> for TargetBuildError {
    fn from(source: sill::SpecError) -> Self {
        Self::Spec(source)
    }
}

impl From<dist::PinError> for TargetBuildError {
    fn from(source: dist::PinError) -> Self {
        Self::Pins(source)
    }
}

impl From<dist::StageError> for TargetBuildError {
    fn from(source: dist::StageError) -> Self {
        Self::Stage(source)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const COMMIT: &str = "1111111111111111111111111111111111111111";
    const MOVED: &str = "2222222222222222222222222222222222222222";

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "spice-release-builder-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("dist/stage")).expect("fixture stage directory");
            fs::write(
                root.join(APP_ARTIFACT),
                include_bytes!("../../dist/stage/app_bg.wasm"),
            )
            .expect("copy real staged Wasm");
            fs::write(
                root.join(EMBEDDED_ARTIFACT),
                include_bytes!("../../dist/stage/canvas-app.html"),
            )
            .expect("copy real embedded shell");
            Self { root }
        }

        fn programs(&self) -> Programs {
            let program = |name: &str| self.root.join("programs").join(name);
            Programs {
                cargo: program("cargo"),
                rustc: program("rustc"),
                rustup: program("rustup"),
                trunk: program("trunk"),
                mutants: program("cargo-mutants"),
                git: program("git"),
                xtask: program("xtask"),
            }
        }

        fn pointer(&self, target: ReleaseTarget) -> PathBuf {
            self.root
                .join("dist/stage/targets")
                .join(target.as_str())
                .join("payload.json")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("fixture cleanup");
        }
    }

    struct FakeExecutor {
        root: PathBuf,
        target: ReleaseTarget,
        trunk: RefCell<String>,
        trunk_drift_after_native: Cell<bool>,
        tracked_dirty: Cell<bool>,
        moved_after_native: Cell<bool>,
        native_ran: Cell<bool>,
        records: RefCell<Vec<CommandRecord>>,
    }

    impl FakeExecutor {
        fn new(root: &Path, target: ReleaseTarget) -> Self {
            Self {
                root: root.to_path_buf(),
                target,
                trunk: RefCell::new(PINNED_TRUNK.to_owned()),
                trunk_drift_after_native: Cell::new(false),
                tracked_dirty: Cell::new(false),
                moved_after_native: Cell::new(false),
                native_ran: Cell::new(false),
                records: RefCell::new(Vec::new()),
            }
        }

        fn labels(&self) -> Vec<&'static str> {
            self.records.borrow().iter().map(|row| row.label).collect()
        }
    }

    impl Executor for FakeExecutor {
        fn execute(&self, command: CommandSpec) -> Result<CommandRecord, TargetBuildError> {
            if command.label == "native-build" {
                let destination = self
                    .root
                    .join("target/release-appliance")
                    .join(self.target.as_str())
                    .join("release/spice");
                fs::create_dir_all(destination.parent().expect("native parent"))
                    .expect("native directory");
                fs::copy(env::current_exe().expect("test executable"), &destination)
                    .expect("copy real compiled executable");
                self.native_ran.set(true);
            }
            let stdout = match command.label {
                "source-commit" if self.moved_after_native.get() && self.native_ran.get() => {
                    format!("{MOVED}\n").into_bytes()
                }
                "source-commit" => format!("{COMMIT}\n").into_bytes(),
                "rust-version" => b"rustc 1.97.1 (fixture)\n".to_vec(),
                "installed-targets" => format!("{WASM_TARGET}\n{}\n", self.target).into_bytes(),
                "trunk-version" if self.trunk_drift_after_native.get() && self.native_ran.get() => {
                    b"trunk 0.21.13\n".to_vec()
                }
                "trunk-version" => format!("{}\n", self.trunk.borrow()).into_bytes(),
                "mutants-version" => format!("{PINNED_MUTANTS}\n").into_bytes(),
                "app-build" | "embed-build" | "native-build" => {
                    format!("{} complete\n", command.label).into_bytes()
                }
                "source-tracked" | "source-untracked" => Vec::new(),
                label => panic!("unexpected command label {label}"),
            };
            let record = CommandRecord {
                label: command.label,
                program: command.program,
                arguments: command.arguments,
                code: if command.label == "source-tracked" && self.tracked_dirty.get() {
                    Some(1)
                } else {
                    Some(0)
                },
                stdout,
                stderr: Vec::new(),
            };
            self.records.borrow_mut().push(record.clone());
            Ok(record)
        }

        fn records(&self) -> Vec<CommandRecord> {
            self.records.borrow().clone()
        }
    }

    #[test]
    fn both_targets_use_the_fixed_commands_and_real_artifact_bytes() {
        for target in [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
        ] {
            let fixture = Fixture::new();
            let programs = fixture.programs();
            let executor = FakeExecutor::new(&fixture.root, target);

            let build = stage_with(&fixture.root, target, &programs, &executor)
                .expect("target stage succeeds");

            assert_eq!(
                executor.labels(),
                [
                    "source-commit",
                    "source-tracked",
                    "source-untracked",
                    "rust-version",
                    "installed-targets",
                    "trunk-version",
                    "mutants-version",
                    "app-build",
                    "embed-build",
                    "native-build",
                    "source-commit",
                    "source-tracked",
                    "source-untracked",
                    "rust-version",
                    "installed-targets",
                    "trunk-version",
                    "mutants-version",
                ]
            );
            let build_records = build
                .commands
                .iter()
                .filter(|record| record.label.ends_with("-build"))
                .collect::<Vec<_>>();
            assert_eq!(
                build_records
                    .iter()
                    .map(|record| record.label)
                    .collect::<Vec<_>>(),
                ["app-build", "embed-build", "native-build"]
            );
            assert_eq!(build_records[0].arguments, [OsString::from("check-app")]);
            assert_eq!(
                build_records[1].arguments,
                cargo_arguments(&fixture.root, "canvas", target)
            );
            assert_eq!(
                build_records[2].arguments,
                cargo_arguments(&fixture.root, "spice", target)
            );
            let mutants = build
                .commands
                .iter()
                .find(|record| record.label == "mutants-version")
                .expect("cargo-mutants pin observation");
            assert_eq!(
                mutants.arguments,
                [OsString::from("mutants"), OsString::from("--version")]
            );
            assert!(build_records.iter().all(|record| {
                record.success() && !record.stdout.is_empty() && record.stderr.is_empty()
            }));
            assert_eq!(
                build.staged.payload.app,
                Digest::compute(
                    Domain::DistArtifact,
                    include_bytes!("../../dist/stage/app_bg.wasm")
                )
            );
            assert_eq!(
                build.staged.payload.embedded_app,
                Digest::compute(
                    Domain::DistArtifact,
                    include_bytes!("../../dist/stage/canvas-app.html")
                )
            );
            let native = fs::read(env::current_exe().expect("test executable"))
                .expect("read real compiled executable");
            assert_eq!(
                build.staged.payload.native,
                Digest::compute(Domain::DistArtifact, &native)
            );
            assert_eq!(
                fs::read(fixture.pointer(target)).expect("target pointer reads"),
                build.staged.payload_bytes
            );
        }
    }

    #[test]
    fn system_executor_overrides_machine_local_cargo_build_state() {
        let fixture = Fixture::new();
        let release = ReleaseEnvironment::new(
            Some(OsString::from("/tool/bin")),
            Some(OsString::from("/home/release")),
            Some(OsString::from("/cargo/home")),
            None,
            None,
            None,
            None,
        );

        let executor = SystemExecutor::new(&fixture.root, &release).expect("release executor");
        let entries = executor.environment.entries();
        let value = |name: &str| {
            entries
                .iter()
                .find_map(|(key, value)| (key == name).then_some(value.as_os_str()))
                .unwrap_or_else(|| panic!("missing release environment {name}"))
        };

        assert_eq!(
            value("CARGO_BUILD_BUILD_DIR"),
            fixture
                .root
                .join("target/release-appliance-build")
                .as_os_str()
        );
        assert_eq!(value("CARGO_INCREMENTAL"), "0");
        assert!(
            entries
                .iter()
                .all(|(key, _)| key != OsStr::new("CARGO_TARGET_DIR"))
        );
        assert_eq!(value("RUSTC_WRAPPER"), "");
        assert_eq!(value("RUSTC_WORKSPACE_WRAPPER"), "");
    }

    #[cfg(unix)]
    #[test]
    fn program_discovery_preserves_a_multicall_proxy_name() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let fixture = Fixture::new();
        let programs = fixture.root.join("programs");
        fs::create_dir(&programs).expect("program directory");
        let rustup = programs.join("rustup");
        fs::write(&rustup, b"#!/bin/sh\n").expect("rustup fixture");
        fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755))
            .expect("rustup executable mode");
        symlink("rustup", programs.join("cargo")).expect("cargo proxy symlink");

        let cargo = find_program(programs.as_os_str(), "cargo").expect("cargo proxy");

        assert_eq!(cargo, fs::canonicalize(&programs).unwrap().join("cargo"));
        assert_eq!(cargo.file_name(), Some(OsStr::new("cargo")));
        assert!(
            fs::symlink_metadata(cargo)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn tool_drift_refuses_before_the_first_build_or_publication() {
        let fixture = Fixture::new();
        let target = ReleaseTarget::Aarch64AppleDarwin;
        let programs = fixture.programs();
        let executor = FakeExecutor::new(&fixture.root, target);
        *executor.trunk.borrow_mut() = "trunk 0.21.13".to_owned();

        let error = stage_with(&fixture.root, target, &programs, &executor)
            .expect_err("tool drift refuses");

        assert!(matches!(error, TargetBuildError::Pins(_)));
        assert!(
            !executor
                .labels()
                .iter()
                .any(|label| label.ends_with("-build"))
        );
        assert!(!fixture.pointer(target).exists());
    }

    #[test]
    fn dirty_source_refuses_before_pin_observation_build_or_publication() {
        let fixture = Fixture::new();
        let target = ReleaseTarget::Aarch64AppleDarwin;
        let programs = fixture.programs();
        let executor = FakeExecutor::new(&fixture.root, target);
        executor.tracked_dirty.set(true);

        let error = stage_with(&fixture.root, target, &programs, &executor)
            .expect_err("dirty source refuses");

        assert!(matches!(error, TargetBuildError::SourceDirty(_)));
        assert_eq!(executor.labels(), ["source-commit", "source-tracked"]);
        assert!(!fixture.pointer(target).exists());
    }

    #[test]
    fn source_movement_after_build_preserves_the_previous_target_pointer() {
        let fixture = Fixture::new();
        let target = ReleaseTarget::Aarch64AppleDarwin;
        let programs = fixture.programs();
        let executor = FakeExecutor::new(&fixture.root, target);
        executor.moved_after_native.set(true);
        let pointer = fixture.pointer(target);
        fs::create_dir_all(pointer.parent().expect("pointer parent")).expect("target directory");
        fs::write(&pointer, b"previous complete target").expect("previous pointer");

        let error = stage_with(&fixture.root, target, &programs, &executor)
            .expect_err("source movement refuses pointer");

        assert!(matches!(error, TargetBuildError::Stage(_)));
        assert_eq!(
            fs::read(pointer).expect("previous pointer remains"),
            b"previous complete target"
        );
        assert_eq!(
            executor
                .labels()
                .into_iter()
                .filter(|label| label.ends_with("-build"))
                .collect::<Vec<_>>(),
            ["app-build", "embed-build", "native-build"]
        );
    }

    #[test]
    fn tool_drift_after_build_preserves_the_previous_target_pointer() {
        let fixture = Fixture::new();
        let target = ReleaseTarget::Aarch64AppleDarwin;
        let programs = fixture.programs();
        let executor = FakeExecutor::new(&fixture.root, target);
        executor.trunk_drift_after_native.set(true);
        let pointer = fixture.pointer(target);
        fs::create_dir_all(pointer.parent().expect("pointer parent")).expect("target directory");
        fs::write(&pointer, b"previous complete target").expect("previous pointer");

        let error = stage_with(&fixture.root, target, &programs, &executor)
            .expect_err("tool drift refuses pointer");

        assert!(matches!(error, TargetBuildError::Stage(_)));
        assert_eq!(
            fs::read(pointer).expect("previous pointer remains"),
            b"previous complete target"
        );
        assert_eq!(
            executor
                .labels()
                .into_iter()
                .filter(|label| label.ends_with("-build"))
                .collect::<Vec<_>>(),
            ["app-build", "embed-build", "native-build"]
        );
    }

    fn cargo_arguments(root: &Path, package: &str, target: ReleaseTarget) -> Vec<OsString> {
        cargo_build_arguments(package, target, &root.join("target/release-appliance"))
    }
}
