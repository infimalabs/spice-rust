//! One-command developer refresh of the repository-embedded Spice appliance.
//!
//! `cargo xtask install` builds the current tree, republishes
//! `.spice-install/bin/spice` through the digest-bound installer, and restarts
//! the resident on the endpoint it had already published.
//!
//! The installer refuses to derive a plan while a completed receipt names
//! different executable bytes, so `install` and `install --apply` alone cannot
//! express a refresh: both answer `install::input-refused` with
//! `Spice executable selection differs from the recorded plan` the moment the
//! tree is rebuilt. A refresh is therefore `unapply` then `plan` then `apply`,
//! and [`refresh_install`] is the only place that sequence is spelled.

use std::fmt;
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use spice::install::{self, InstallError, InstallPreview};
use spice::repository::Repository;
use wire::Digest;

/// Worktree-relative home for the rollback executable and resident log.
const WORKSPACE: &str = "target/spice-develop";
/// Executable parked before a refresh so a failure can restore it.
const ROLLBACK: &str = "spice.rollback";
/// Where a restarted resident's own diagnostics land.
const RESIDENT_LOG: &str = "resident.log";
/// Longest wait for a restarted resident to publish its discovery.
const READINESS_HORIZON: Duration = Duration::from_secs(30);
/// Gap between discovery polls while waiting for readiness.
const READINESS_INTERVAL: Duration = Duration::from_millis(100);
/// Longest wait for a stopped resident to withdraw its discovery.
const SHUTDOWN_HORIZON: Duration = Duration::from_secs(15);

/// One developer refresh request.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefreshRequest {
    /// Host for the restarted resident, or the published one.
    pub host: Option<String>,
    /// Port for the restarted resident, or the published one.
    pub port: Option<u16>,
    /// Build and install the release profile rather than the debug profile.
    pub release: bool,
    /// Restart the resident after the executable is republished.
    pub restart: bool,
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
pub fn refresh(
    request: &RefreshRequest,
    output: &mut impl Write,
) -> Result<RefreshReport, RefreshError> {
    let repository = Repository::discover(Path::new("."))?;
    let built = build(request.release, output)?;
    let installed = install::installed_binary(&repository);
    let rollback = park(&repository, &installed)?;
    let published = read_endpoint(&repository);
    let endpoint = requested_endpoint(request, published.as_deref());

    let unwind = Rollback {
        installed: &installed,
        executable: rollback.as_deref(),
        published: published.as_deref(),
        restart: request.restart,
    };

    if request.restart && published.is_some() {
        stop(&repository, &installed, output)?;
    }
    let digest = match refresh_install(&repository, &built) {
        Ok(digest) => digest,
        Err(error) => return Err(restore(&repository, &unwind, output, error)),
    };
    writeln!(output, "installed {} plan={digest}", installed.display())?;

    let bound = (endpoint.0.as_deref(), endpoint.1);
    let endpoint = match restart(&repository, &installed, bound, request, output) {
        Ok(endpoint) => endpoint,
        Err(error) => return Err(restore(&repository, &unwind, output, error)),
    };
    Ok(RefreshReport {
        built,
        installed,
        digest,
        endpoint,
    })
}

/// Republish the installed executable from `binary` under one fresh plan.
///
/// A completed receipt pins the executable bytes it installed, so the plan must
/// be withdrawn before a rebuilt binary can derive a new one. Every published
/// file still crosses through the installer's own atomic rename, and the
/// receipt names the exact digest applied.
pub fn refresh_install(repository: &Repository, binary: &Path) -> Result<Digest, InstallError> {
    match install::unapply(repository) {
        Ok(_) | Err(InstallError::RecordMissing) => {}
        Err(error) => return Err(error),
    }
    let plan = install::plan(repository, Some(binary))?;
    let preview = InstallPreview::from_plan(plan)?;
    let digest = preview.digest().clone();
    install::apply(repository, Some(binary), &digest)?;
    Ok(digest)
}

/// Build `spice` and answer the executable Cargo reported for it.
fn build(release: bool, output: &mut impl Write) -> Result<PathBuf, RefreshError> {
    writeln!(output, "building spice")?;
    let mut command = Command::new("cargo");
    command.args([
        "build",
        "--locked",
        "--package",
        "spice",
        "--bin",
        "spice",
        "--message-format",
        "json-render-diagnostics",
    ]);
    if release {
        command.arg("--release");
    }
    let built = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;
    if !built.status.success() {
        return Err(RefreshError::Build);
    }
    let rendered = String::from_utf8_lossy(&built.stdout);
    built_executable(&rendered).ok_or(RefreshError::BuildOutputSilent)
}

/// Read the `spice` executable path out of Cargo's build event stream.
///
/// The path is taken from Cargo rather than assembled from a profile directory
/// because `CARGO_TARGET_DIR` and workspace layout both move it.
fn built_executable(events: &str) -> Option<PathBuf> {
    events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
        })
        .filter(|event| {
            event
                .pointer("/target/name")
                .and_then(serde_json::Value::as_str)
                == Some("spice")
        })
        .filter_map(|event| {
            event
                .get("executable")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
        })
        .next_back()
}

/// Copy the currently installed executable aside so a failure can restore it.
fn park(repository: &Repository, installed: &Path) -> Result<Option<PathBuf>, RefreshError> {
    if !installed.is_file() {
        return Ok(None);
    }
    let rollback = workspace(repository)?.join(ROLLBACK);
    fs::copy(installed, &rollback)?;
    Ok(Some(rollback))
}

/// What a failed refresh needs to put the appliance back the way it found it.
struct Rollback<'a> {
    /// Published executable a restored resident is started from.
    installed: &'a Path,
    /// Parked copy of the executable that was installed before the refresh.
    executable: Option<&'a Path>,
    /// Endpoint the stopped resident had published, when one was running.
    published: Option<&'a str>,
    /// Whether this refresh was the kind that stops and starts a resident.
    restart: bool,
}

/// Reinstall the parked executable, restart what was running, and report the
/// failure that forced both.
fn restore(
    repository: &Repository,
    rollback: &Rollback<'_>,
    output: &mut impl Write,
    cause: impl Into<RefreshError>,
) -> RefreshError {
    let cause = cause.into();
    let Some(executable) = rollback.executable else {
        return cause;
    };
    let _ = writeln!(output, "restoring previous executable after: {cause}");
    match refresh_install(repository, executable) {
        Ok(digest) => {
            let _ = writeln!(output, "restored plan={digest}");
        }
        Err(error) => {
            return RefreshError::RestoreFailed {
                cause: cause.to_string(),
                error,
            };
        }
    }
    restore_resident(repository, rollback, output);
    cause
}

/// Start the restored executable's resident back on the endpoint it had.
///
/// A refresh that stopped the resident and then failed leaves the appliance
/// installed but unserved, so putting the executable back is only half of what
/// was taken away. A resident that will not come back is reported rather than
/// raised, because the failure worth answering is the one that forced the
/// restore, not the restore's own aftermath.
fn restore_resident(repository: &Repository, rollback: &Rollback<'_>, output: &mut impl Write) {
    if !rollback.restart {
        return;
    }
    let Some((host, port)) = rollback.published.and_then(endpoint_parts) else {
        return;
    };
    let _ = writeln!(output, "restarting previous resident");
    let request = RefreshRequest {
        restart: true,
        ..RefreshRequest::default()
    };
    let bound = (Some(host.as_str()), Some(port));
    if let Err(error) = restart(repository, rollback.installed, bound, &request, output) {
        let _ = writeln!(output, "previous resident did not come back: {error}");
    }
}

/// Create and answer the worktree-local scratch directory this command owns.
fn workspace(repository: &Repository) -> Result<PathBuf, RefreshError> {
    let workspace = repository.worktree_root().join(WORKSPACE);
    fs::create_dir_all(&workspace)?;
    Ok(workspace)
}

/// Endpoint the published discovery names, when a resident published one.
fn read_endpoint(repository: &Repository) -> Option<String> {
    let bytes = fs::read(repository.admit_state().ok()?.canvas()).ok()?;
    let discovery = wire::decode::<canvas::Discovery>(&bytes).ok()?;
    Some(discovery.endpoint)
}

/// Host and port the restarted resident should bind, each honoured on its own.
///
/// `up` accepts `--host` and `--port` independently, so an operator who names
/// one gets it whether or not a resident had published the other. Dropping an
/// explicit port for want of a host would instead leave `up` on its own default
/// endpoint, which this repository documents as the predecessor's.
fn requested_endpoint(
    request: &RefreshRequest,
    published: Option<&str>,
) -> (Option<String>, Option<u16>) {
    let published = published.and_then(endpoint_parts);
    let host = request
        .host
        .clone()
        .or_else(|| published.as_ref().map(|(host, _)| host.clone()));
    let port = request.port.or_else(|| published.map(|(_, port)| port));
    (host, port)
}

/// Split a published origin into the host and port `up` accepts.
fn endpoint_parts(endpoint: &str) -> Option<(String, u16)> {
    let authority = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let authority = authority.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    Some((host.to_owned(), port.parse().ok()?))
}

/// Stop the running resident and wait for its discovery to withdraw.
fn stop(
    repository: &Repository,
    installed: &Path,
    output: &mut impl Write,
) -> Result<(), RefreshError> {
    writeln!(output, "stopping resident")?;
    // A published discovery outlives the process that wrote it, so `down`
    // failing is not itself a refusal: what matters is that nothing is still
    // answering on the endpoint by the time the executable is replaced.
    // `down` answers its own reply on standard output, which is this command's
    // progress stream and not a result the developer asked for. Diagnostics
    // stay inherited so a refusal is still legible.
    Command::new(installed)
        .arg("down")
        .current_dir(repository.worktree_root())
        .stdout(Stdio::null())
        .status()?;
    let deadline = Instant::now() + SHUTDOWN_HORIZON;
    while read_endpoint(repository).is_some_and(|endpoint| listening(&endpoint)) {
        if Instant::now() >= deadline {
            return Err(RefreshError::ResidentStop);
        }
        sleep(READINESS_INTERVAL);
    }
    Ok(())
}

/// Start the refreshed resident and answer the endpoint it published.
fn restart(
    repository: &Repository,
    installed: &Path,
    endpoint: (Option<&str>, Option<u16>),
    request: &RefreshRequest,
    output: &mut impl Write,
) -> Result<Option<String>, RefreshError> {
    if !request.restart {
        return Ok(None);
    }
    let log = fs::File::create(workspace(repository)?.join(RESIDENT_LOG))?;
    let mut command = Command::new(installed);
    command.arg("up");
    let (host, port) = endpoint;
    if let Some(host) = host {
        command.args(["--host", host]);
    }
    if let Some(port) = port {
        command.args(["--port", &port.to_string()]);
    }
    let mut child = command
        .current_dir(repository.worktree_root())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    match ready(repository, &mut child, READINESS_HORIZON) {
        Ok(endpoint) => {
            writeln!(output, "resident ready at {endpoint}")?;
            Ok(Some(endpoint))
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            report_resident_log(repository, output);
            Err(error)
        }
    }
}

/// Surface the resident's own diagnostic for a start that did not come up.
///
/// The refusal that explains the failure is written by the resident, into the
/// log this command redirected it to; the refresh itself can only observe an
/// exit status or a horizon. Reporting the status alone leaves the developer
/// holding a number whose reason is one file away and unnamed.
fn report_resident_log(repository: &Repository, output: &mut impl Write) {
    let Ok(workspace) = workspace(repository) else {
        return;
    };
    let log = workspace.join(RESIDENT_LOG);
    let last = fs::read_to_string(&log).ok().and_then(|text| {
        text.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(str::to_owned)
    });
    let _ = match last {
        Some(line) => writeln!(output, "resident log {}: {line}", log.display()),
        None => writeln!(output, "resident log {} recorded nothing", log.display()),
    };
}

/// Wait for the resident to publish a discovery it also answers on.
///
/// A published discovery outlives the process that wrote it, so the record
/// alone is not readiness: the endpoint it names has to accept a connection,
/// and the resident has to still be running when it does.
fn ready(
    repository: &Repository,
    child: &mut std::process::Child,
    horizon: Duration,
) -> Result<String, RefreshError> {
    let deadline = Instant::now() + horizon;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(RefreshError::ResidentExited(status.code()));
        }
        if let Some(endpoint) = read_endpoint(repository)
            && listening(&endpoint)
        {
            return Ok(endpoint);
        }
        if Instant::now() >= deadline {
            return Err(RefreshError::ResidentUnready);
        }
        sleep(READINESS_INTERVAL);
    }
}

/// Whether a published endpoint accepts a connection right now.
fn listening(endpoint: &str) -> bool {
    endpoint_parts(endpoint)
        .is_some_and(|(host, port)| TcpStream::connect((host.as_str(), port)).map(drop).is_ok())
}

/// Every way one developer refresh refuses.
#[derive(Debug)]
pub enum RefreshError {
    /// The repository under the working directory did not resolve.
    Repository(spice::repository::ResolveError),
    /// `cargo build` reported failure.
    Build,
    /// `cargo build` succeeded without naming a `spice` executable.
    BuildOutputSilent,
    /// The installer refused the refresh.
    Install(InstallError),
    /// The previous executable could not be reinstalled after a failure.
    RestoreFailed {
        /// Failure that forced the restore.
        cause: String,
        /// Failure the restore itself hit.
        error: InstallError,
    },
    /// The resident did not stop.
    ResidentStop,
    /// The resident exited before it was ready.
    ResidentExited(Option<i32>),
    /// The resident never published an endpoint it answers on.
    ResidentUnready,
    /// The surrounding filesystem or process environment failed.
    Io(std::io::Error),
}

impl fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(error) => write!(formatter, "repository unresolved: {error}"),
            Self::Build => write!(formatter, "cargo build failed"),
            Self::BuildOutputSilent => {
                write!(formatter, "cargo build named no spice executable")
            }
            Self::Install(error) => write!(formatter, "install refused: {error}"),
            Self::RestoreFailed { cause, error } => write!(
                formatter,
                "install refused: {cause}; restoring the previous executable also failed: {error}"
            ),
            Self::ResidentStop => write!(formatter, "resident did not stop"),
            Self::ResidentExited(code) => match code {
                Some(code) => write!(formatter, "resident exited with status {code}"),
                None => write!(formatter, "resident exited on a signal"),
            },
            Self::ResidentUnready => write!(formatter, "resident never became ready"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RefreshError {}

impl From<spice::repository::ResolveError> for RefreshError {
    fn from(error: spice::repository::ResolveError) -> Self {
        Self::Repository(error)
    }
}

impl From<InstallError> for RefreshError {
    fn from(error: InstallError) -> Self {
        Self::Install(error)
    }
}

impl From<std::io::Error> for RefreshError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests;
