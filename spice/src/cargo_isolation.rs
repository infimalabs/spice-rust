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
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
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
/// Cargo's supported home override.
pub const CARGO_HOME_VARIABLE: &str = "CARGO_HOME";

const STORE_RELATIVE: &str = "spice/source-store-v2";

/// Host authorities captured before an isolated Cargo home is installed.
#[derive(Clone, Eq, PartialEq)]
pub struct Host {
    bootstrap_home: PathBuf,
    cargo: PathBuf,
    seed_environment: Environment,
}

impl fmt::Debug for Host {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Host")
            .field("bootstrap_home", &self.bootstrap_home)
            .field("cargo", &self.cargo)
            .field("seed_environment", &"<redacted>")
            .finish()
    }
}

impl Host {
    /// Capture the bootstrap Cargo home and exact native Cargo executable.
    ///
    /// # Errors
    ///
    /// Refuses missing, relative, non-directory, or recursively isolated host
    /// authorities and a Cargo executable that is absent or not executable.
    pub fn from_environment(
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
        let cargo = entries
            .get(std::ffi::OsStr::new(NATIVE_CARGO_VARIABLE))
            .map(PathBuf::from)
            .map_or_else(
                || {
                    resolve_cargo_from_path(
                        entries
                            .get(std::ffi::OsStr::new("PATH"))
                            .map(OsString::as_os_str),
                    )
                },
                Ok,
            )?;
        let environment = Environment::new(entries).map_err(|error| {
            IsolationError::new(
                "cargo-isolation::seed-environment-invalid",
                error.to_string(),
            )
        })?;
        Self::with_environment(PathBuf::from(bootstrap), cargo, environment)
    }

    /// Admit explicit host authorities. This is the deterministic test and
    /// embedding seam; it performs no ambient reads.
    pub fn new(bootstrap_home: PathBuf, cargo: PathBuf) -> Result<Self, IsolationError> {
        Self::with_environment(bootstrap_home, cargo, Environment::empty())
    }

    fn with_environment(
        bootstrap_home: PathBuf,
        cargo: PathBuf,
        seed_environment: Environment,
    ) -> Result<Self, IsolationError> {
        require_absolute(&bootstrap_home, "bootstrap Cargo home")?;
        require_absolute(&cargo, "native Cargo executable")?;
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
        let resolved_cargo = fs::canonicalize(&cargo).map_err(|error| {
            IsolationError::io("cargo-isolation::native-cargo-unavailable", &cargo, error)
        })?;
        if resolved_cargo.starts_with(bootstrap_home.join(STORE_RELATIVE)) {
            return Err(IsolationError::new(
                "cargo-isolation::native-cargo-recursive",
                "native Cargo executable resolves inside the generated source store",
            ));
        }
        let metadata = fs::metadata(&cargo).map_err(|error| {
            IsolationError::io("cargo-isolation::native-cargo-unavailable", &cargo, error)
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(IsolationError::new(
                "cargo-isolation::native-cargo-invalid",
                format!("native Cargo is not an executable file: {cargo:?}"),
            ));
        }
        Ok(Self {
            bootstrap_home,
            cargo,
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

fn resolve_cargo_from_path(path: Option<&std::ffi::OsStr>) -> Result<PathBuf, IsolationError> {
    let path = path.ok_or_else(|| {
        IsolationError::new(
            "cargo-isolation::native-cargo-unavailable",
            "PATH is absent and SPICE_RS_NATIVE_CARGO was not supplied",
        )
    })?;
    for directory in std::env::split_paths(path) {
        let candidate = directory.join("cargo");
        if fs::metadata(&candidate)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return std::path::absolute(&candidate).map_err(|error| {
                IsolationError::io(
                    "cargo-isolation::native-cargo-unavailable",
                    &candidate,
                    error,
                )
            });
        }
    }
    Err(IsolationError::new(
        "cargo-isolation::native-cargo-unavailable",
        "PATH contains no executable named cargo",
    ))
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
