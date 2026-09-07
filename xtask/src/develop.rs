//! One-command developer refresh of the repository-embedded Spice appliance.
//!
//! `spice dev xtask -- install` builds the current browser app and native tree,
//! republishes `.spice-install/bin/spice` through the digest-bound installer,
//! and restarts the resident on the endpoint it had already published.
//!
//! The installer derives a refresh when a completed receipt names different
//! executable bytes, so a rebuilt tree needs nothing withdrawn: the install step
//! is `plan` then `apply`, and every destination is exchanged in place. What
//! this command adds over those two calls is the rest of the loop — building
//! the tree, and stopping and restarting the resident on its own endpoint.
//!
//! That loop only exists where Cargo, processes, and the installer do, so it
//! lives in [`native`] behind the wasm gate. The request, the report, and every
//! refusal stay published on every target so a caller compiled for wasm sees
//! the same [`refresh`] signature and one honest [`RefreshError::Unavailable`].

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use wire::Digest;

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::refresh_install;

/// One developer refresh request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefreshRequest {
    /// Host for the restarted resident, or the published one.
    pub host: Option<String>,
    /// Port for the restarted resident, or the published one.
    pub port: Option<u16>,
    /// Build and install the release profile rather than the development profile.
    pub release: bool,
    /// Restart the resident after the executable is republished.
    pub restart: bool,
    /// Arguments forwarded verbatim to the detached `spice up` launcher.
    pub up_arguments: Vec<OsString>,
}

/// What one completed refresh published.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshReport {
    /// Build output the refresh installed.
    pub built: PathBuf,
    /// Installed executable the appliance now runs.
    pub installed: PathBuf,
    /// Identity of the plan this refresh applied.
    pub digest: Digest,
    /// Endpoint the restarted resident published, when one was started.
    pub endpoint: Option<String>,
}

/// Build the current tree, republish the appliance, and restart its resident.
///
/// Answers [`RefreshError::Unavailable`] on a target without the build,
/// process, and installer machinery the refresh drives.
pub fn refresh(
    request: &RefreshRequest,
    environment: &crate::ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<RefreshReport, RefreshError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::perform(request, environment, output)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (request, environment, output);
        Err(RefreshError::Unavailable)
    }
}

/// Every way one developer refresh refuses.
#[derive(Debug)]
pub enum RefreshError {
    /// The repository under the working directory did not resolve.
    Repository(spice::repository::ResolveError),
    /// `cargo build` reported failure.
    Build,
    /// The current browser app did not build and publish.
    App(crate::AppBuildError),
    /// `cargo build` succeeded without naming a `spice` executable.
    BuildOutputSilent,
    /// The installer refused the refresh.
    #[cfg(not(target_arch = "wasm32"))]
    Install(spice::install::InstallError),
    /// The previous executable could not be reinstalled after a failure.
    #[cfg(not(target_arch = "wasm32"))]
    RestoreFailed {
        /// Failure that forced the restore.
        cause: String,
        /// Failure the restore itself hit.
        error: spice::install::InstallError,
    },
    /// The resident did not stop.
    ResidentStop,
    /// `down` refused while the endpoint was still answering.
    ResidentStopRefused(String),
    /// The sole daemon launcher returned its exact startup refusal.
    ResidentStartRefused(String),
    /// The resident exited before it was ready.
    ResidentExited(Option<i32>),
    /// The resident never published an endpoint it answers on.
    ResidentUnready,
    /// The surrounding filesystem or process environment failed.
    Io(std::io::Error),
    /// This target carries no refresh machinery to run.
    Unavailable,
}

impl fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository unresolved: {error}"),
            Self::Build => write!(formatter, "cargo build failed"),
            Self::App(error) => write!(formatter, "canvas app build failed: {error}"),
            Self::BuildOutputSilent => {
                write!(formatter, "cargo build named no spice executable")
            }
            #[cfg(not(target_arch = "wasm32"))]
            Self::Install(error) => write!(formatter, "install refused: {error}"),
            #[cfg(not(target_arch = "wasm32"))]
            Self::RestoreFailed { cause, error } => write!(
                formatter,
                "install refused: {cause}; restoring the previous executable also failed: {error}"
            ),
            Self::ResidentStop => write!(formatter, "resident did not stop"),
            Self::ResidentStopRefused(refusal) => write!(
                formatter,
                "down refused and the endpoint is still answering: {refusal}"
            ),
            Self::ResidentStartRefused(refusal) => {
                write!(formatter, "resident start refused: {refusal}")
            }
            Self::ResidentExited(code) => match code {
                Some(code) => write!(formatter, "resident exited with status {code}"),
                None => write!(formatter, "resident exited on a signal"),
            },
            Self::ResidentUnready => write!(formatter, "resident never became ready"),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Unavailable => write!(formatter, "refresh is unavailable on this target"),
        }
    }
}

impl std::error::Error for RefreshError {}

impl From<spice::repository::ResolveError> for RefreshError {
    fn from(error: spice::repository::ResolveError) -> Self {
        Self::Repository(error)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<spice::install::InstallError> for RefreshError {
    fn from(error: spice::install::InstallError) -> Self {
        Self::Install(error)
    }
}

impl From<std::io::Error> for RefreshError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<crate::AppBuildError> for RefreshError {
    fn from(error: crate::AppBuildError) -> Self {
        Self::App(error)
    }
}
