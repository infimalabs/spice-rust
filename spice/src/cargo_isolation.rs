//! Supported Cargo isolation for supervised lanes and development commands.
//!
//! One immutable directory-source store holds external package payloads. Each
//! worktree or durable lane receives only a private Cargo home containing its
//! own Cargo locks, last-use database, ownership record, and generated source
//! replacement configuration.

use std::error::Error as StdError;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sill::Environment;
use wire::CanonicalUuid;

use crate::repository::Repository;

mod store;

#[cfg(test)]
#[path = "cargo_isolation/tests.rs"]
mod tests;

/// Child authority naming the original Cargo home used to seed immutable data.
pub const BOOTSTRAP_HOME_VARIABLE: &str = "SPICE_RS_BOOTSTRAP_CARGO_HOME"; // env-policy: allow
/// Child authority naming the native Cargo executable resolved before isolation.
pub const NATIVE_CARGO_VARIABLE: &str = "SPICE_RS_NATIVE_CARGO"; // env-policy: allow
/// Child authority naming the native rustc executable paired with Cargo.
pub const NATIVE_RUSTC_VARIABLE: &str = "SPICE_RS_NATIVE_RUSTC"; // env-policy: allow
/// Child authority naming the native rustdoc executable paired with Cargo.
pub const NATIVE_RUSTDOC_VARIABLE: &str = "SPICE_RS_NATIVE_RUSTDOC"; // env-policy: allow
/// Cargo's supported home override.
pub const CARGO_HOME_VARIABLE: &str = "CARGO_HOME";
/// Cargo's supported compiler override.
pub const RUSTC_VARIABLE: &str = "RUSTC";
/// Cargo's supported documentation compiler override.
pub const RUSTDOC_VARIABLE: &str = "RUSTDOC";
/// Lane interception state excluded from developer toolchains unless a child opts in.
pub const DEVELOPER_CHILD_EXCLUDED_VARIABLES: [&str; 3] = [
    steer::hook::BASH_ENV,
    steer::hook::ZDOTDIR,
    wire::SpiceVariable::Inbox.as_str(),
];

const STORE_RELATIVE: &str = "spice/source-store-v2";

/// Host authorities captured before an isolated Cargo home is installed.
#[derive(Clone, Eq, PartialEq)]
pub struct Host {
    bootstrap_home: PathBuf,
    cargo: PathBuf,
    rustc: PathBuf,
    rustdoc: PathBuf,
    seed_environment: Environment,
}

impl fmt::Debug for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Host")
            .field("bootstrap_home", &self.bootstrap_home)
            .field("cargo", &self.cargo)
            .field("rustc", &self.rustc)
            .field("rustdoc", &self.rustdoc)
            .field("seed_environment", &"<redacted>")
            .finish()
    }
}

impl Host {
    /// Capture the bootstrap Cargo home and one exact native Rust toolchain.
    ///
    /// # Errors
    ///
    /// Refuses missing, relative, non-directory, or recursively isolated host
    /// authorities and any Cargo, rustc, or rustdoc executable that is absent,
    /// non-executable, or outside the other tools' generation directory.
    pub fn from_environment(
        repository: &Path,
        entries: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Result<Self, IsolationError> {
        let entries = entries
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();
        let bootstrap = entries
            .get(std::ffi::OsStr::new(BOOTSTRAP_HOME_VARIABLE))
            .or_else(|| entries.get(std::ffi::OsStr::new(CARGO_HOME_VARIABLE)))
            .cloned()
            .or_else(|| default_cargo_home(entries.get(std::ffi::OsStr::new("HOME"))))
            .ok_or_else(|| {
                IsolationError::new(
                    "cargo-isolation::bootstrap-home-unavailable",
                    "neither SPICE_RS_BOOTSTRAP_CARGO_HOME, CARGO_HOME, nor HOME names the native Cargo home",
                )
            })?;
        let toolchain = resolve_native_toolchain(repository, &bootstrap, &entries)?;
        let environment = Environment::new(entries).map_err(|error| {
            IsolationError::new(
                "cargo-isolation::seed-environment-invalid",
                error.to_string(),
            )
        })?;
        Self::with_environment(PathBuf::from(bootstrap), toolchain, environment)
    }

    /// Admit explicit host authorities. This is the deterministic test and
    /// embedding seam; it performs no ambient reads.
    pub fn new(
        bootstrap_home: PathBuf,
        cargo: PathBuf,
        rustc: PathBuf,
        rustdoc: PathBuf,
    ) -> Result<Self, IsolationError> {
        Self::with_environment(
            bootstrap_home,
            NativeToolchain::admit(cargo, rustc, rustdoc)?,
            Environment::empty(),
        )
    }

    fn with_environment(
        bootstrap_home: PathBuf,
        toolchain: NativeToolchain,
        seed_environment: Environment,
    ) -> Result<Self, IsolationError> {
        require_absolute(&bootstrap_home, "bootstrap Cargo home")?;
        let bootstrap_home = fs::canonicalize(&bootstrap_home).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::bootstrap-home-unavailable",
                &bootstrap_home,
                error,
            )
        })?;
        let bootstrap_metadata = fs::metadata(&bootstrap_home).map_err(|error| {
            IsolationError::io(
                "cargo-isolation::bootstrap-home-unavailable",
                &bootstrap_home,
                error,
            )
        })?;
        if !bootstrap_metadata.is_dir() {
            return Err(IsolationError::new(
                "cargo-isolation::bootstrap-home-invalid",
                format!("bootstrap Cargo home is not a directory: {bootstrap_home:?}"),
            ));
        }
        if toolchain
            .cargo
            .starts_with(bootstrap_home.join(STORE_RELATIVE))
        {
            return Err(IsolationError::new(
                "cargo-isolation::native-cargo-recursive",
                "native Cargo executable resolves inside the generated source store",
            ));
        }
        Ok(Self {
            bootstrap_home,
            cargo: toolchain.cargo,
            rustc: toolchain.rustc,
            rustdoc: toolchain.rustdoc,
            seed_environment,
        })
    }

    /// Original Cargo home used only to seed immutable package generations.
    #[must_use]
    pub fn bootstrap_home(&self) -> &Path {
        &self.bootstrap_home
    }

    /// Native Cargo executable captured before `CARGO_HOME` changes.
    #[must_use]
    pub fn cargo(&self) -> &Path {
        &self.cargo
    }

    /// Native rustc paired with [`Self::cargo`].
    #[must_use]
    pub fn rustc(&self) -> &Path {
        &self.rustc
    }

    /// Native rustdoc paired with [`Self::cargo`].
    #[must_use]
    pub fn rustdoc(&self) -> &Path {
        &self.rustdoc
    }

    fn seed_environment(&self) -> &Environment {
        &self.seed_environment
    }

    fn store_root(&self) -> PathBuf {
        self.bootstrap_home.join(STORE_RELATIVE)
    }
}

/// One admitted isolated home and immutable lockfile generation.
#[derive(Debug)]
pub struct PreparedHome {
    home: PathBuf,
    generation: String,
    store: PathBuf,
    cargo: PathBuf,
    reused_generation: bool,
    _pin: Option<store::GenerationPin>,
}

impl PreparedHome {
    /// Private Cargo home to export to one lane or development process.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Exact lockfile generation admitted by this home.
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// Shared immutable store root.
    #[must_use]
    pub fn store(&self) -> &Path {
        &self.store
    }

    /// Pre-resolved native Cargo executable.
    #[must_use]
    pub fn cargo(&self) -> &Path {
        &self.cargo
    }

    /// Whether the immutable generation already existed before this prepare.
    #[must_use]
    pub const fn reused_generation(&self) -> bool {
        self.reused_generation
    }

    /// Arrange for a manual Cargo child to retain this generation pin even if
    /// its supervising wrapper exits first.
    ///
    /// # Errors
    ///
    /// Refuses a prepared home that lacks its required manual-reader pin or a
    /// descriptor whose inheritance contract cannot be installed.
    pub fn retain_pin_for_child(
        &mut self,
        command: &mut std::process::Command,
    ) -> Result<(), IsolationError> {
        self._pin
            .as_mut()
            .ok_or_else(|| {
                IsolationError::new(
                    "cargo-isolation::pin-unavailable",
                    "prepared manual Cargo home carries no reader pin",
                )
            })?
            .retain_for_child(command)
    }

    /// Machine-safe summary that retains non-UTF-8 paths as hexadecimal bytes.
    #[must_use]
    pub fn report(&self) -> PreparedReport {
        PreparedReport {
            home: store::encoded_path(&self.home),
            generation: self.generation.clone(),
            store: store::encoded_path(&self.store),
            cargo: store::encoded_path(&self.cargo),
            reused_generation: self.reused_generation,
        }
    }
}

/// Serializable preparation evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PreparedReport {
    home: String,
    generation: String,
    store: String,
    cargo: String,
    reused_generation: bool,
}

/// Inspectable store inventory and retained uncertainty.
pub use store::{CleanupReport, Inspection};

/// Prepare a private manual-development Cargo home for this repository.
pub fn prepare_manual(
    repository: &Repository,
    host: &Host,
) -> Result<PreparedHome, IsolationError> {
    prepare(repository, None, host)
}

/// Prepare a private Cargo home for one durable lane.
pub fn prepare_lane(
    repository: &Repository,
    lane: CanonicalUuid,
    host: &Host,
) -> Result<PreparedHome, IsolationError> {
    prepare(repository, Some(lane), host)
}

fn prepare(
    repository: &Repository,
    lane: Option<CanonicalUuid>,
    host: &Host,
) -> Result<PreparedHome, IsolationError> {
    store::prepare(repository.worktree_root(), lane, host)
}

/// Inspect the owned source store without changing it.
pub fn inspect(host: &Host) -> Result<Inspection, IsolationError> {
    store::inspect(host)
}

/// Reclaim only old recognized temporaries and unreferenced generations.
pub fn cleanup(host: &Host) -> Result<CleanupReport, IsolationError> {
    store::cleanup(host)
}

fn default_cargo_home(home: Option<&OsString>) -> Option<OsString> {
    home.map(|home| PathBuf::from(home).join(".cargo").into_os_string())
}

#[derive(Debug)]
struct NativeToolchain {
    cargo: PathBuf,
    rustc: PathBuf,
    rustdoc: PathBuf,
}

impl NativeToolchain {
    fn admit(cargo: PathBuf, rustc: PathBuf, rustdoc: PathBuf) -> Result<Self, IsolationError> {
        let cargo = admit_native_executable(cargo, "Cargo")?;
        let rustc = admit_native_executable(rustc, "rustc")?;
        let rustdoc = admit_native_executable(rustdoc, "rustdoc")?;
        let cargo_directory = cargo.parent();
        if cargo_directory != rustc.parent() || cargo_directory != rustdoc.parent() {
            return Err(IsolationError::new(
                "cargo-isolation::native-toolchain-mixed",
                format!(
                    "native Cargo, rustc, and rustdoc do not resolve inside one toolchain directory: {cargo:?}, {rustc:?}, {rustdoc:?}"
                ),
            ));
        }
        Ok(Self {
            cargo,
            rustc,
            rustdoc,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrackedToolchain {
    toolchain: TrackedToolchainBody,
}

#[derive(Deserialize)]
struct TrackedToolchainBody {
    channel: String,
}

fn resolve_native_toolchain(
    repository: &Path,
    bootstrap_home: &std::ffi::OsStr,
    entries: &std::collections::BTreeMap<OsString, OsString>,
) -> Result<NativeToolchain, IsolationError> {
    let declared = [
        entries.get(std::ffi::OsStr::new(NATIVE_CARGO_VARIABLE)),
        entries.get(std::ffi::OsStr::new(NATIVE_RUSTC_VARIABLE)),
        entries.get(std::ffi::OsStr::new(NATIVE_RUSTDOC_VARIABLE)),
    ];
    match declared.iter().filter(|value| value.is_some()).count() {
        3 => NativeToolchain::admit(
            PathBuf::from(declared[0].expect("complete native trio has Cargo")),
            PathBuf::from(declared[1].expect("complete native trio has rustc")),
            PathBuf::from(declared[2].expect("complete native trio has rustdoc")),
        ),
        0 => resolve_tracked_toolchain(repository, Path::new(bootstrap_home), entries),
        _ => Err(IsolationError::new(
            "cargo-isolation::native-toolchain-incomplete",
            "SPICE_RS_NATIVE_CARGO, SPICE_RS_NATIVE_RUSTC, and SPICE_RS_NATIVE_RUSTDOC must be supplied together",
        )),
    }
}

fn resolve_tracked_toolchain(
    repository: &Path,
    bootstrap_home: &Path,
    entries: &std::collections::BTreeMap<OsString, OsString>,
) -> Result<NativeToolchain, IsolationError> {
    let declaration = repository.join("rust-toolchain.toml");
    let bytes = fs::read(&declaration).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::tracked-toolchain-unavailable",
            &declaration,
            error,
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        IsolationError::new(
            "cargo-isolation::tracked-toolchain-invalid",
            format!("{} is not UTF-8: {error}", declaration.display()),
        )
    })?;
    let tracked = toml::from_str::<TrackedToolchain>(text).map_err(|error| {
        IsolationError::new(
            "cargo-isolation::tracked-toolchain-invalid",
            format!("{} is invalid: {error}", declaration.display()),
        )
    })?;
    if tracked.toolchain.channel.trim().is_empty() {
        return Err(IsolationError::new(
            "cargo-isolation::tracked-toolchain-invalid",
            format!("{} declares an empty channel", declaration.display()),
        ));
    }
    let rustup = admit_native_executable(bootstrap_home.join("bin/rustup"), "rustup")?;
    let rustup_home = entries
        .get(std::ffi::OsStr::new("RUSTUP_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            entries
                .get(std::ffi::OsStr::new("HOME"))
                .map(|home| PathBuf::from(home).join(".rustup"))
        })
        .ok_or_else(|| {
            IsolationError::new(
                "cargo-isolation::rustup-home-unavailable",
                "neither RUSTUP_HOME nor HOME names the tracked toolchain store",
            )
        })?;
    require_absolute(&rustup_home, "Rustup home")?;
    NativeToolchain::admit(
        rustup_which(&rustup, &rustup_home, &tracked.toolchain.channel, "cargo")?,
        rustup_which(&rustup, &rustup_home, &tracked.toolchain.channel, "rustc")?,
        rustup_which(&rustup, &rustup_home, &tracked.toolchain.channel, "rustdoc")?,
    )
}

fn rustup_which(
    rustup: &Path,
    rustup_home: &Path,
    channel: &str,
    tool: &str,
) -> Result<PathBuf, IsolationError> {
    let output = Command::new(rustup)
        .args(["which", "--toolchain", channel, tool])
        .env_clear()
        .env("RUSTUP_HOME", rustup_home)
        .output()
        .map_err(|error| {
            IsolationError::io(
                "cargo-isolation::tracked-toolchain-resolution-failed",
                rustup,
                error,
            )
        })?;
    if !output.status.success() {
        return Err(IsolationError::new(
            "cargo-isolation::tracked-toolchain-resolution-failed",
            format!(
                "{} could not resolve {tool} for tracked channel {channel:?}: {}",
                rustup.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let mut bytes = output.stdout;
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(IsolationError::new(
            "cargo-isolation::tracked-toolchain-resolution-failed",
            format!(
                "{} returned no path for {tool} in tracked channel {channel:?}",
                rustup.display()
            ),
        ));
    }
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn admit_native_executable(path: PathBuf, name: &str) -> Result<PathBuf, IsolationError> {
    require_absolute(&path, &format!("native {name} executable"))?;
    let resolved = fs::canonicalize(&path).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::native-toolchain-unavailable",
            &path,
            error,
        )
    })?;
    let metadata = fs::metadata(&resolved).map_err(|error| {
        IsolationError::io(
            "cargo-isolation::native-toolchain-unavailable",
            &resolved,
            error,
        )
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(IsolationError::new(
            "cargo-isolation::native-toolchain-invalid",
            format!("native {name} is not an executable file: {resolved:?}"),
        ));
    }
    Ok(resolved)
}

fn require_absolute(path: &Path, label: &str) -> Result<(), IsolationError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(IsolationError::new(
            "cargo-isolation::authority-relative",
            format!("{label} must be absolute: {path:?}"),
        ))
    }
}

/// Typed Cargo-isolation refusal or environment fault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsolationError {
    reason: &'static str,
    detail: String,
}

impl IsolationError {
    pub(super) fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }

    pub(super) fn io(reason: &'static str, path: &Path, error: std::io::Error) -> Self {
        Self::new(reason, format!("{path:?}: {error}"))
    }

    /// Stable refusal/fault identifier.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }

    /// Exact observed detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for IsolationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl StdError for IsolationError {}
