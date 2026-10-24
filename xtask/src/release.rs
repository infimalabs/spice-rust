//! Concrete, bounded target-build adapter for the distribution pipeline.

use std::cell::RefCell;
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::app::PINNED_TRUNK;

use atomic::{RootedFiles, RootedPublish};
use dist::{BuildError, Builder, Canvas, Pipeline, RequiredPins, StagedTarget};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
};
use wire::manifest::{BuildPins, ReleaseTarget};
use wire::{Digest, Domain};

mod builder;
mod native;
#[cfg(all(test, unix))]
use builder::dsym_path;
use builder::{CommandBuilder, build_error, cargo_executable};
mod report;
pub use report::TargetBuildError;
pub(crate) use report::render_canvas;
use report::{render_command_records, render_records};
mod source;
#[cfg(test)]
use source::verify_checkout;
use source::{GuardedPublisher, SourceGuard};

mod windows_appliance;
pub use windows_appliance::build_windows_appliance;

#[cfg(unix)]
mod symbols;
#[cfg(unix)]
use symbols::SymbolBundle;

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
const BUILD_RUNTIME: Duration = Duration::from_secs(60 * 60);
const OUTPUT_DRAIN: Duration = Duration::from_secs(10);
const REAP: Duration = Duration::from_secs(10);
const GRACE: Duration = Duration::from_secs(5);
const WASM_TARGET: &str = "wasm32-unknown-unknown";
const PINNED_MUTANTS: &str = "cargo-mutants 27.1.0";
#[cfg(unix)]
const MACHO_UUID_LENGTH: usize = 36;
#[cfg(unix)]
const MACHO_UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

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
    /// The final linked, stripped executable the staged payload names.
    pub executable: PathBuf,
    pub symbols: Option<MacSymbols>,
    /// Uncompressed app files the payload no longer carries beside the
    /// executable, which links them as the canvas archive instead.
    pub app_tree: u64,
    pub commands: Vec<CommandRecord>,
}

/// One verified macOS symbol bundle derived from the exact linked executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacSymbols {
    bundle: PathBuf,
    uuid: String,
}

#[cfg(unix)]
struct DerivedMacSymbols {
    public: MacSymbols,
    capture: SymbolBundle,
}

impl MacSymbols {
    #[must_use]
    pub fn bundle(&self) -> &Path {
        &self.bundle
    }

    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.uuid
    }
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
    rustc: Option<OsString>,
    build_rustc: Option<OsString>,
    stage_root: Option<OsString>,
}

impl ReleaseEnvironment {
    /// The two compiler overrides come last because appending cannot silently
    /// permute the arguments an existing caller already passes.
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
        rustc: Option<OsString>,
        build_rustc: Option<OsString>,
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
            rustc,
            build_rustc,
            stage_root: None,
        }
    }

    /// Select an external stage root for an isolated hosted build.
    #[must_use]
    pub fn with_stage_root(mut self, stage_root: Option<OsString>) -> Self {
        self.stage_root = stage_root;
        self
    }

    fn stage_root(&self, root: &Path) -> PathBuf {
        self.stage_root
            .as_ref()
            .map_or_else(|| root.join("dist/stage"), PathBuf::from)
    }

    /// Whether this process is the isolated guest half of hosted staging.
    #[must_use]
    pub const fn is_hosted_guest(&self) -> bool {
        self.stage_root.is_some()
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

    /// The `rustc` a build started in `build_dir` from this environment would
    /// compile with.
    ///
    /// `PATH` is the last question, not the only one, and the environment is
    /// not the only place a compiler is named. Cargo settles this in five
    /// ranks and all five are asked here in that order: `RUSTC`, then
    /// `CARGO_BUILD_RUSTC`, then the `build.rustc` key of the Cargo config
    /// found by walking up from the build directory nearest-first, then that
    /// same key in the `CARGO_HOME` config, and only then the `PATH` walk.
    /// Measured against the installed Cargo 1.97.1, one build per rank, each
    /// dying on the compiler Cargo chose so the winner is named in the error
    /// rather than deduced: `RUSTC` beat `CARGO_BUILD_RUSTC`,
    /// `CARGO_BUILD_RUSTC` beat every config file, a nearer config beat a
    /// farther one, every ancestor config beat the `CARGO_HOME` one, and the
    /// config layer as a whole beat `PATH`.
    ///
    /// The order is written out rather than delegated because Cargo cannot be
    /// asked for it: `cargo config get` is nightly-only and this toolchain is
    /// stable. Each caller supplies the working directory of the build whose
    /// compiler it is settling. The app surface supplies its `app` directory;
    /// the release pipeline supplies the workspace root used by its executor,
    /// then freezes that answer as `RUSTC` for every child build.
    ///
    /// The config ranks are about bytes and not tidiness. The sealed image
    /// ships a Cargo config at the filesystem root deliberately, because that
    /// is the one location every Cargo in it reads, and the workspace tracks
    /// one of its own. A compiler named in either would have built the app
    /// stage while the release admitted here came from `PATH`, and that stage
    /// is bytes only one compiler release reproduces.
    ///
    /// A name is taken as written, because that is what Cargo hands the
    /// operating system; a name that resolves to nothing is a compiler that
    /// cannot be read rather than an environment that has none. `Ok(None)` is
    /// for an environment that names no compiler at any of the five ranks.
    ///
    /// # Errors
    ///
    /// Returns a Cargo config that exists and cannot be opened, that names
    /// `build.rustc` as something other than one quoted string, or that puts a
    /// bare `rustc` key somewhere [`named_rustc`] will not place it. Such a
    /// file is refused rather than stepped over, because stepping over it would
    /// answer with the compiler Cargo was not going to run.
    pub(crate) fn rustc(&self, build_dir: &Path) -> Result<Option<PathBuf>, TargetBuildError> {
        if let Some(overridden) = self.rustc.as_ref().or(self.build_rustc.as_ref()) {
            return Ok(Some(PathBuf::from(overridden)));
        }
        if let Some(configured) = self.configured_rustc(build_dir)? {
            return Ok(Some(configured));
        }
        let Some(path) = self.path.as_deref() else {
            return Ok(None);
        };
        match find_program(path, "rustc") {
            Ok(rustc) => Ok(Some(rustc)),
            Err(TargetBuildError::MissingProgram("rustc")) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The compiler a Cargo config names for a build started in `build_dir`.
    ///
    /// Nearest directory first and the `CARGO_HOME` config last, which is the
    /// order measured above. A config that exists without the key does not end
    /// the walk: Cargo merges these files key by key, so a parent still answers
    /// for a key the nearer file leaves unset.
    fn configured_rustc(&self, build_dir: &Path) -> Result<Option<PathBuf>, TargetBuildError> {
        for directory in build_dir.ancestors() {
            if let Some(named) = configured_rustc_in(&directory.join(".cargo"))? {
                return Ok(Some(named));
            }
        }
        match self.cargo_home() {
            Some(cargo_home) => configured_rustc_in(Path::new(&cargo_home)),
            None => Ok(None),
        }
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
    let programs = Programs::discover(&root, &environment, target)?;
    let executor = SystemExecutor::new(&root, &environment, &programs.rustc)?;
    match stage_with_roots(
        &root,
        &environment.build_root(&root),
        &environment.stage_root(&root),
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

#[cfg(test)]
fn stage_with<E: Executor>(
    root: &Path,
    build_root: &Path,
    target: ReleaseTarget,
    programs: &Programs,
    executor: &E,
) -> Result<TargetBuild, TargetBuildError> {
    stage_with_roots(
        root,
        build_root,
        &root.join("dist/stage"),
        target,
        programs,
        executor,
    )
}

fn stage_with_roots<E: Executor>(
    root: &Path,
    build_root: &Path,
    stage_root: &Path,
    target: ReleaseTarget,
    programs: &Programs,
    executor: &E,
) -> Result<TargetBuild, TargetBuildError> {
    let source = SourceGuard::capture(executor, &programs.git)?;
    let required = required_pins();
    let observed = observe_pins(executor, programs, target)?;
    let required = RequiredPins::new(required)?;
    required.verify(&observed)?;

    let pipeline = Pipeline::new(required.clone(), dist::current_product_version()?);
    let target_dir = build_root.join("release-appliance");
    let mut builder = CommandBuilder {
        root,
        target,
        target_dir: &target_dir,
        programs,
        executor,
        #[cfg(unix)]
        source: &source,
        symbols: None,
        symbol_artifact: None,
        windows_debug_artifact: None,
        canvas: None,
        executable: None,
    };
    let mut publisher = GuardedPublisher {
        inner: RootedFiles::new(stage_root),
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
    let symbols = builder.symbols.take();
    let executable = builder
        .executable
        .take()
        .expect("a staged target built its executable");
    let app_tree = builder
        .canvas
        .as_ref()
        .expect("a staged target read its canvas")
        .tree();
    Ok(TargetBuild {
        target,
        source: source.digest,
        staged,
        executable,
        app_tree,
        symbols,
        commands: executor.records(),
    })
}

fn parse_target(spelling: &str) -> Result<ReleaseTarget, TargetBuildError> {
    match spelling {
        "aarch64-apple-darwin" => Ok(ReleaseTarget::Aarch64AppleDarwin),
        "x86_64-unknown-linux-musl" => Ok(ReleaseTarget::X86_64UnknownLinuxMusl),
        "x86_64-pc-windows-msvc" => Ok(ReleaseTarget::X86_64PcWindowsMsvc),
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

#[derive(Clone)]
struct Programs {
    cargo: PathBuf,
    rustc: PathBuf,
    rustup: PathBuf,
    trunk: PathBuf,
    mutants: PathBuf,
    git: PathBuf,
    xtask: PathBuf,
    #[cfg(unix)]
    macos: Option<MacPrograms>,
}

#[cfg(unix)]
#[derive(Clone)]
struct MacPrograms {
    dsymutil: PathBuf,
    dwarfdump: PathBuf,
    strip: PathBuf,
}

impl Programs {
    /// Resolve the programs one release build will execute.
    ///
    /// Every ordinary tool comes from the acquired `PATH`. The compiler is the
    /// exception because Cargo has four higher-ranked doors: resolve it through
    /// [`ReleaseEnvironment::rustc`] at the workspace-root cwd and selected
    /// Cargo home, then let [`SystemExecutor`] freeze that answer as `RUSTC`.
    fn discover(
        root: &Path,
        environment: &ReleaseEnvironment,
        target: ReleaseTarget,
    ) -> Result<Self, TargetBuildError> {
        let path = environment
            .path
            .as_deref()
            .ok_or(TargetBuildError::MissingProgram("PATH"))?;
        let rustc = environment
            .rustc(root)?
            .ok_or(TargetBuildError::MissingProgram("rustc"))?;
        #[cfg(not(unix))]
        let _ = target;
        #[cfg(unix)]
        let macos = if target == ReleaseTarget::Aarch64AppleDarwin {
            Some(MacPrograms {
                dsymutil: find_program(path, "dsymutil")?,
                dwarfdump: find_program(path, "dwarfdump")?,
                strip: find_program(path, "strip")?,
            })
        } else {
            None
        };
        Ok(Self {
            cargo: find_program(path, "cargo")?,
            rustc,
            rustup: find_program(path, "rustup")?,
            trunk: find_program(path, "trunk")?,
            mutants: find_program(path, "cargo-mutants")?,
            git: find_program(path, "git")?,
            xtask: env::current_exe().map_err(|source| TargetBuildError::Io {
                path: PathBuf::from("current xtask executable"),
                source,
            })?,
            #[cfg(unix)]
            macos,
        })
    }
}

/// The two names one Cargo config directory can carry, in Cargo's own order.
///
/// The extension-less spelling is deprecated and still wins: measured against
/// the installed Cargo, a directory holding both warns that `config` is
/// deprecated in favour of `config.toml` and then uses `config` anyway. A
/// reader that opened only `config.toml` would be reading the file Cargo did
/// not read, which is the whole failure this exists to prevent.
const CARGO_CONFIG_NAMES: [&str; 2] = ["config", "config.toml"];

/// What `build.rustc` says in the first Cargo config `directory` holds.
///
/// The first name found is the only one read, because that is what Cargo does
/// with a directory holding both. `None` means a file was read and left the key
/// unset, or that the directory holds no config at all; the caller continues
/// outward either way, since Cargo would too.
fn configured_rustc_in(directory: &Path) -> Result<Option<PathBuf>, TargetBuildError> {
    for name in CARGO_CONFIG_NAMES {
        let path = directory.join(name);
        let document = match fs::read_to_string(&path) {
            Ok(document) => document,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(TargetBuildError::Io { path, source }),
        };
        return named_rustc(&document, &path);
    }
    Ok(None)
}

/// The `build.rustc` string one Cargo config document names, if it names one.
///
/// One key out of a document, read as text the way [`crate::toolchain`] reads
/// the pin document rather than by admitting a TOML parser into this crate to
/// reach a single line. Unlike that document this repository does not write
/// these -- one of them is the developer's own `~/.cargo/config.toml`, which is
/// an ancestor of every worktree here -- so this reads them under one rule: no
/// line naming a bare `rustc` key is ever walked past. Either it can be placed,
/// and it answers, or it cannot, and the file is refused.
///
/// That rule is what makes the skipping safe. Every spelling that names a
/// compiler contains `=`, every line containing `=` is placed here, and so a
/// line holding neither a header nor a `=` cannot be the key however it is
/// indented -- the interior of a multi-line array is the ordinary case, and
/// stopping on it would refuse a `rustflags` list every Cargo reads happily.
/// What a skip cannot do is hide the key, which is the whole point: a missed
/// `build.rustc` stages bytes under a compiler nobody chose, while a refusal
/// names the file and quotes whatever stopped it.
fn named_rustc(document: &str, path: &Path) -> Result<Option<PathBuf>, TargetBuildError> {
    let refuse = |detail: String| TargetBuildError::CargoConfig {
        path: path.to_path_buf(),
        detail,
    };
    let mut table = String::new();
    for line in document.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let Some(header) = header.strip_suffix(']') else {
                return Err(refuse(format!("a table header does not close: {line}")));
            };
            table = header.trim().to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let dotted = key_path(&table, key);
        if dotted == ["build"] {
            return Err(refuse(format!(
                "build is given as one value, so whether it names a rustc cannot be read: {line}"
            )));
        }
        if dotted != ["build", "rustc"] {
            if names_a_rustc(key) {
                return Err(refuse(format!(
                    "a key names rustc somewhere this reader cannot place it: {line}"
                )));
            }
            continue;
        }
        let Some(named) = quoted(value) else {
            return Err(refuse(format!(
                "build.rustc is not one quoted string: {value}"
            )));
        };
        return Ok(Some(PathBuf::from(named)));
    }
    Ok(None)
}

/// The dotted key one line writes, with the header it sits under in front.
///
/// Both halves are split, because Cargo reads `rustc` under a `[build]` header
/// and a top-level `build.rustc` as the same key, and a reader that knew only
/// one spelling would miss the other.
fn key_path(table: &str, key: &str) -> Vec<String> {
    table
        .split('.')
        .chain(key.split('.'))
        .map(|segment| unquoted(segment.trim()).to_owned())
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// Whether a key names `rustc` itself rather than a longer key starting with it.
///
/// `rustc-wrapper` and `rustc-workspace-wrapper` are ordinary `[build]` keys
/// that decide nothing about which compiler runs, so they are let past; a bare
/// `rustc` this reader could not place under `build` is not.
fn names_a_rustc(key: &str) -> bool {
    key.match_indices("rustc").any(|(at, found)| {
        let identifier = |character: char| character.is_alphanumeric() || "-_".contains(character);
        !key[..at].ends_with(identifier) && !key[at + found.len()..].starts_with(identifier)
    })
}

/// The contents of one quoted TOML string, if that is the whole value.
fn quoted(value: &str) -> Option<&str> {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
}

/// One key segment with the quotes TOML allows around it removed.
fn unquoted(segment: &str) -> &str {
    quoted(segment).unwrap_or(segment)
}

fn find_program(path: &OsStr, name: &'static str) -> Result<PathBuf, TargetBuildError> {
    #[cfg(windows)]
    let file_name = format!("{name}.exe");
    #[cfg(not(windows))]
    let file_name = name;
    for directory in env::split_paths(path) {
        let candidate = directory.join(file_name);
        if executable(&candidate) {
            let directory = if directory.as_os_str().is_empty() {
                Path::new(".")
            } else {
                &directory
            };
            return fs::canonicalize(directory)
                .map(|directory| directory.join(file_name))
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
    /// Seal every release command inside the environment whose pins were read.
    ///
    /// `rustc` is the five-rank answer [`ReleaseEnvironment::rustc`] resolved
    /// at this workspace-root working directory. Naming it as `RUSTC` freezes
    /// that answer at Cargo's highest rank, so app-build and native-build
    /// cannot select a compiler different from either
    /// [`observe_pins`] call. `CARGO_BUILD_RUSTC` is intentionally not copied:
    /// the resolved `RUSTC` outranks it and already incorporates its answer.
    fn new(
        root: &Path,
        release: &ReleaseEnvironment,
        rustc: &Path,
    ) -> Result<Self, TargetBuildError> {
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
            (OsString::from("RUSTC"), rustc.as_os_str().to_owned()),
            // Named rather than dropped, because one of the commands below is
            // this same binary: `app-build` re-invokes the sole `xtask app`
            // path, which resolves its build root from this variable at its rim. An
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

    fn ambient(root: &Path, environment: Environment) -> Self {
        Self {
            root: root.to_path_buf(),
            environment,
            records: RefCell::new(Vec::new()),
        }
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
    read_regular_with(path, || {})
}

fn read_regular_with(
    path: &Path,
    after_admission: impl FnOnce(),
) -> Result<Vec<u8>, TargetBuildError> {
    let io = |source| TargetBuildError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = fs::File::open(path).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err(TargetBuildError::Artifact(path.to_path_buf()));
    }
    let admitted = sill::authority::file_identity(&file).map_err(io)?;
    if sill::authority::path_identity(path).map_err(io)? != admitted {
        return Err(TargetBuildError::Artifact(path.to_path_buf()));
    }
    let length = usize::try_from(admitted.bytes())
        .map_err(|_| TargetBuildError::Artifact(path.to_path_buf()))?;
    let mut bytes = vec![0; length];
    after_admission();
    file.read_exact(&mut bytes).map_err(io)?;
    let finished = sill::authority::file_identity(&file).map_err(io)?;
    let named = sill::authority::path_identity(path).map_err(io)?;
    if finished != admitted || named != admitted {
        return Err(TargetBuildError::Artifact(path.to_path_buf()));
    }
    Ok(bytes)
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
    if !root.join("Cargo.toml").is_file() || !root.join("dist").is_dir() {
        return Err(TargetBuildError::Workspace(root));
    }
    Ok(root)
}

#[cfg(test)]
mod tests;
