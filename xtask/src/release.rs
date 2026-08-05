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
    rustup_toolchain: Option<OsString>,
    temporary: Option<OsString>,
    sdk_root: Option<OsString>,
    developer_dir: Option<OsString>,
    cargo_target_dir: Option<OsString>,
}

impl ReleaseEnvironment {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        path: Option<OsString>,
        home: Option<OsString>,
        cargo_home: Option<OsString>,
        rustup_home: Option<OsString>,
        rustup_toolchain: Option<OsString>,
        temporary: Option<OsString>,
        sdk_root: Option<OsString>,
        developer_dir: Option<OsString>,
        cargo_target_dir: Option<OsString>,
    ) -> Self {
        Self {
            path,
            home,
            cargo_home,
            rustup_home,
            rustup_toolchain,
            temporary,
            sdk_root,
            developer_dir,
            cargo_target_dir,
        }
    }

    /// Where every build this binary starts is allowed to write.
    ///
    /// The workspace is the default, and it is only the default. The sealed
    /// image mounts the workspace read-only and names its writable scratch in
    /// `CARGO_TARGET_DIR`, so a build root rooted at the workspace is not a
    /// slower path there but an impossible one. Resolved once, here, so that
    /// every directory below is a name under a single answer rather than a
    /// separate guess at where writing is permitted.
    #[must_use]
    pub fn build_root(&self, root: &Path) -> PathBuf {
        self.cargo_target_dir
            .as_ref()
            .map_or_else(|| root.join("target"), PathBuf::from)
    }

    /// The `rustc` a build started from this environment would compile with.
    ///
    /// Resolved by the same walk over the same `PATH` that the release
    /// pipeline's own program discovery uses, so the compiler asked for its
    /// release is the one a build here would run, rather than whichever one
    /// this process happens to sit beside.
    pub(crate) fn rustc(&self) -> Option<PathBuf> {
        find_program(self.path.as_deref()?, "rustc").ok()
    }

    /// The Cargo home the builds this binary starts must read crates from.
    ///
    /// `HOME` supplies the same default Cargo itself would, so the fallback is
    /// Cargo's own rule rather than a second one. Both are acquired at the rim
    /// because the answer belongs to the running process: the sealed image
    /// mounts a vendored `/cargo-home` that no compile-time constant baked on
    /// the building host could ever have named.
    #[must_use]
    pub fn cargo_home(&self) -> Option<OsString> {
        self.cargo_home.clone().or_else(|| {
            self.home
                .as_ref()
                .map(|home| PathBuf::from(home).join(".cargo").into_os_string())
        })
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
    match stage_with(
        &root,
        &environment.build_root(&root),
        target,
        &programs,
        &executor,
    ) {
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
    build_root: &Path,
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
    let target_dir = build_root.join("release-appliance");
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
        proof_container: proof::DEFAULT_IMAGE.to_owned(),
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
        proof_container: proof::DEFAULT_IMAGE.to_owned(),
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

/// The only untracked paths a release build is allowed to have produced.
const RELEASE_OUTPUT: [&str; 2] = ["dist/stage/objects/", "dist/stage/targets/"];

/// The porcelain code and separator an untracked entry is reported under.
const UNTRACKED: &str = "?? ";

fn verify_checkout<E: Executor>(executor: &E, git: &Path) -> Result<(), TargetBuildError> {
    // One porcelain status answers both questions, and it is the only spelling
    // that answers them truthfully. `git diff-index` trusts the index stat
    // cache, so a checkout whose files are byte-identical to HEAD still reads
    // as modified whenever that cache is stale -- exactly what a fresh clone
    // observed through another kernel's view of the same bytes produces, which
    // is the sealed proof image reading a mounted host checkout. `git status`
    // refreshes the index before it compares, so it reports content.
    let status = successful(executor.execute(CommandSpec::new(
        "source-status",
        git,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    ))?)?;
    let status = text(&status.stdout, "source status inventory")?;
    for entry in status.lines() {
        let Some(path) = entry.strip_prefix(UNTRACKED) else {
            return Err(TargetBuildError::SourceDirty(format!(
                "tracked source differs from HEAD: {entry}"
            )));
        };
        if !RELEASE_OUTPUT.iter().any(|root| path.starts_with(root)) {
            return Err(TargetBuildError::SourceDirty(format!(
                "untracked source path is outside release output: {path}"
            )));
        }
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
            // Without the selected toolchain a rustup proxy falls back to the
            // tracked `rust-toolchain.toml`, and honoring that file means
            // reaching the network for the components and targets it names.
            // The sealed appliance selects its one installed toolchain this
            // way, so dropping the name is what makes an offline build fail.
            ("RUSTUP_TOOLCHAIN", release.rustup_toolchain.as_ref()),
            ("TMPDIR", release.temporary.as_ref()),
            ("SDKROOT", release.sdk_root.as_ref()),
            ("DEVELOPER_DIR", release.developer_dir.as_ref()),
        ] {
            if let Some(value) = value {
                entries.push((key.into(), value.clone()));
            }
        }
        if let Some(cargo_home) = release.cargo_home() {
            entries.push(("CARGO_HOME".into(), cargo_home));
        }
        let build_root = release.build_root(root);
        entries.extend([
            // Named rather than dropped, because one of the commands below is
            // this same binary: `app-build` re-invokes `xtask check-app`, which
            // resolves its own build root from this variable at its own rim. An
            // absent value there is not a neutral default but a second answer —
            // the app rebuild would write under the workspace while everything
            // else wrote here, which is exactly the split a read-only workspace
            // makes fatal. Cargo's own builds are unaffected: every one of them
            // carries an explicit `--target-dir`, which outranks this.
            (
                OsString::from("CARGO_TARGET_DIR"),
                build_root.clone().into_os_string(),
            ),
            (
                OsString::from("CARGO_BUILD_BUILD_DIR"),
                build_root.join("release-appliance-build").into_os_string(),
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
mod tests;
