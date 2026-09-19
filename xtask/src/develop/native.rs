//! The native half of one developer refresh.
//!
//! Everything here drives the surrounding machine — Cargo, the filesystem, the
//! resident process, and the digest-bound installer that [`spice::install`]
//! itself gates off the wasm target. [`super::refresh`] publishes the signature
//! on every target and delegates to [`perform`] only where these exist.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use spice::install::{self, InstallError, InstallPreview};
use spice::repository::Repository;
use wire::Digest;

/// Worktree-relative home for the rollback executable and resident log.
const WORKSPACE: &str = "target/spice-develop";
/// Executable parked before a refresh so a failure can restore it.
const ROLLBACK: &str = "spice.rollback";
use super::{RefreshError, RefreshReport, RefreshRequest};

/// Build the current tree, republish the appliance, and restart its resident.
pub(super) fn perform(
    request: &RefreshRequest,
    environment: &crate::ReleaseEnvironment,
    output: &mut impl Write,
) -> Result<RefreshReport, RefreshError> {
    let repository = Repository::discover(Path::new("."))?;
    writeln!(output, "building canvas app")?;
    crate::app(environment, output)?;
    let built = build(request.release, request.symbols, output)?;
    let installed = install::installed_binary(&repository);
    let rollback = park(&repository, &installed)?;
    let before = snapshot_install(&repository, &installed)?;
    let schema = schema_recovery_boundary(&repository, request.restart)?;
    let published = read_endpoint(&repository);
    let endpoint = requested_endpoint(request, published.as_deref());

    let unwind = Rollback {
        installed: &installed,
        executable: rollback.as_deref(),
        published: published.as_deref(),
        restart: request.restart,
    };

    if request.restart && published.is_some() {
        let stopping = Instant::now();
        stop(&repository, &installed, output)?;
        writeln!(output, "resident stopped in {:.1?}", stopping.elapsed())?;
    }
    let installing = Instant::now();
    let digest = match refresh_install(&repository, &built) {
        Ok(digest) => digest,
        Err(error) => {
            return Err(restore_if_changed(
                &repository,
                &unwind,
                &before,
                output,
                error,
            ));
        }
    };
    writeln!(
        output,
        "installed {} plan={digest} in {:.1?}",
        installed.display(),
        installing.elapsed()
    )?;

    let bound = (endpoint.0.as_deref(), endpoint.1);
    let endpoint = match restart(&repository, &installed, bound, request, output) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let Some((journal, before_schema)) = &schema else {
                return Err(error);
            };
            return Err(recover_start_failure(
                &repository,
                &unwind,
                journal,
                *before_schema,
                output,
                error,
            ));
        }
    };
    Ok(RefreshReport {
        built,
        installed,
        digest,
        endpoint,
    })
}

fn schema_recovery_boundary(
    repository: &Repository,
    restart: bool,
) -> Result<Option<(PathBuf, Option<u32>)>, RefreshError> {
    if !restart {
        return Ok(None);
    }
    let journal = repository
        .admit_state()
        .map_err(InstallError::from)?
        .journal()
        .to_path_buf();
    let schema =
        inspect_schema(&journal).map_err(|error| RefreshError::SchemaRecoveryUncertain {
            before: None,
            cause: "pre-refresh schema inspection failed".to_owned(),
            error,
        })?;
    Ok(Some((journal, schema)))
}

fn inspect_schema(path: &Path) -> Result<Option<u32>, spice::journal_schema::InspectionError> {
    if !path.exists() {
        return Ok(None);
    }
    spice::journal_schema::inspect_stored(path).map(Some)
}

fn recover_start_failure(
    repository: &Repository,
    rollback: &Rollback<'_>,
    journal: &Path,
    before_schema: Option<u32>,
    output: &mut impl Write,
    cause: RefreshError,
) -> RefreshError {
    recover_start_failure_after(
        repository,
        rollback,
        before_schema,
        inspect_schema(journal),
        output,
        cause,
    )
}

fn recover_start_failure_after(
    repository: &Repository,
    rollback: &Rollback<'_>,
    before_schema: Option<u32>,
    after_schema: Result<Option<u32>, spice::journal_schema::InspectionError>,
    output: &mut impl Write,
    cause: RefreshError,
) -> RefreshError {
    let cause_text = cause.to_string();
    let after_schema = match after_schema {
        Ok(schema) => schema,
        Err(error) => {
            return RefreshError::SchemaRecoveryUncertain {
                before: before_schema,
                cause: cause_text,
                error,
            };
        }
    };
    if schema_rollback_safe(before_schema, after_schema) {
        return restore(repository, rollback, output, cause);
    }
    let _ = writeln!(
        output,
        "journal schema moved from {before_schema:?} to {after_schema:?}; preserving new executable"
    );
    RefreshError::SchemaAdvanced {
        before: before_schema,
        after: after_schema,
        cause: cause_text,
    }
}

const fn schema_rollback_safe(before: Option<u32>, after: Option<u32>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => before == after,
        _ => false,
    }
}

/// Republish the installed executable from `binary` under one fresh plan.
///
/// A rebuilt binary over a completed receipt derives a refresh, so there is no
/// plan to withdraw first: every published file is exchanged in place through
/// the installer's own atomic rename, and the receipt names the exact digest
/// applied. Nothing here is ever absent, which is what lets a hook fire and a
/// worktree resolve `.spice-install/bin/spice` for the whole of a refresh.
pub fn refresh_install(repository: &Repository, binary: &Path) -> Result<Digest, InstallError> {
    let plan = install::plan(repository, Some(binary))?;
    let preview = InstallPreview::from_plan(plan)?;
    let digest = preview.digest().clone();
    install::apply(repository, Some(binary), &digest)?;
    Ok(digest)
}
/// Build `spice` and answer the executable Cargo reported for it.
///
/// `symbols` overrides the profile's own strip setting so the linked
/// executable keeps its debug information long enough for
/// [`generate_worktree_symbols`] to capture it, then strips the executable
/// back down to its ordinary installed shape once the dSYM is retained.
fn build(release: bool, symbols: bool, output: &mut impl Write) -> Result<PathBuf, RefreshError> {
    writeln!(output, "building spice profile={}", build_profile(release))?;
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
    if symbols {
        let profile = if release { "release" } else { "dev" };
        command
            .arg("--config")
            .arg(format!("profile.{profile}.package.spice.strip=false"));
    }
    let built = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()?;
    if !built.status.success() {
        return Err(RefreshError::Build);
    }
    let rendered = String::from_utf8_lossy(&built.stdout);
    let executable = built_executable(&rendered).ok_or(RefreshError::BuildOutputSilent)?;
    if symbols {
        generate_worktree_symbols(&executable, output)?;
    }
    Ok(executable)
}

const fn build_profile(release: bool) -> &'static str {
    if release { "release" } else { "development" }
}

/// Generate a dSYM for `executable` beside it, then strip the executable.
///
/// [`spice::install::symbols`] resolves this exact sibling path when no
/// sealed release package is present, so an ordinary refresh gets a verified
/// symbol receipt the same way a packaged release install does, without a
/// release build or a signed payload, unless `--no-symbols` suppressed it.
///
/// The linker derives the executable's UUID from its contents, so a bundle
/// beside it that already carries that UUID was extracted from this exact
/// link. Cargo restores the unstripped executable from its deps on every
/// build, even one that relinked nothing, and extracting its DWARF again took
/// seven seconds of every refresh that changed no native source.
#[cfg(target_os = "macos")]
fn generate_worktree_symbols(
    executable: &Path,
    output: &mut impl Write,
) -> Result<(), RefreshError> {
    let bundle = worktree_dsym_path(executable);
    let executable_uuid = symbol_uuid(executable)?;
    if bundle.exists() && symbol_uuid(&bundle).is_ok_and(|uuid| uuid == executable_uuid) {
        writeln!(
            output,
            "reusing dSYM {} for UUID {executable_uuid}",
            bundle.display()
        )?;
    } else {
        writeln!(output, "generating dSYM for {}", executable.display())?;
        run_symbols_tool(
            Command::new("dsymutil")
                .arg(executable)
                .arg("-o")
                .arg(&bundle),
        )?;
        let dwarf_uuid = symbol_uuid(&bundle)?;
        if executable_uuid != dwarf_uuid {
            return Err(RefreshError::Symbols(format!(
                "executable UUID {executable_uuid} differs from dSYM UUID {dwarf_uuid}"
            )));
        }
    }
    // Plain `strip` matches `[profile.dev.package.spice] strip = "symbols"`;
    // `-S` alone only drops the debug symbol table and left the installed
    // executable at 149.8M against the ordinary 95.8M shape (measured
    // directly against this exact debug build).
    run_symbols_tool(Command::new("strip").arg(executable))?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn generate_worktree_symbols(
    _executable: &Path,
    _output: &mut impl Write,
) -> Result<(), RefreshError> {
    Err(RefreshError::Symbols(
        "dSYM generation is only available on macOS".to_owned(),
    ))
}

#[cfg(target_os = "macos")]
fn worktree_dsym_path(executable: &Path) -> PathBuf {
    let mut path = executable.as_os_str().to_owned();
    path.push(".dSYM");
    PathBuf::from(path)
}

#[cfg(target_os = "macos")]
fn symbol_uuid(path: &Path) -> Result<String, RefreshError> {
    let output = Command::new("dwarfdump")
        .arg("--uuid")
        .arg(path)
        .output()
        .map_err(|error| {
            RefreshError::Symbols(format!("dwarfdump --uuid {}: {error}", path.display()))
        })?;
    if !output.status.success() {
        return Err(RefreshError::Symbols(format!(
            "dwarfdump --uuid {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim(),
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().ok_or_else(|| {
        RefreshError::Symbols(format!(
            "dwarfdump --uuid {} produced no output",
            path.display()
        ))
    })?;
    let uuid = line
        .strip_prefix("UUID: ")
        .and_then(|rest| rest.split(' ').next())
        .ok_or_else(|| {
            RefreshError::Symbols(format!(
                "dwarfdump --uuid {} is malformed: {line:?}",
                path.display()
            ))
        })?;
    Ok(uuid.to_owned())
}

#[cfg(target_os = "macos")]
fn run_symbols_tool(command: &mut Command) -> Result<(), RefreshError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|error| RefreshError::Symbols(format!("{program} failed to start: {error}")))?;
    if !status.success() {
        return Err(RefreshError::Symbols(format!(
            "{program} exited with {status}"
        )));
    }
    Ok(())
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

/// Exact receipt and executable state immediately before one refresh attempt.
#[derive(Debug, Eq, PartialEq)]
struct InstallSnapshot {
    receipt: PathSnapshot,
    executable: PathSnapshot,
}

/// Comparison surface for one install path, including Unix permission drift.
#[derive(Debug, Eq, PartialEq)]
enum PathSnapshot {
    Missing,
    Regular {
        bytes: Vec<u8>,
        readonly: bool,
        #[cfg(unix)]
        mode: u32,
    },
    Other,
}

fn snapshot_install(
    repository: &Repository,
    installed: &Path,
) -> Result<InstallSnapshot, RefreshError> {
    let state = repository.admit_state().map_err(InstallError::from)?;
    Ok(InstallSnapshot {
        receipt: snapshot_path(state.install_record())?,
        executable: snapshot_path(installed)?,
    })
}

fn snapshot_path(path: &Path) -> Result<PathSnapshot, std::io::Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PathSnapshot::Missing);
        }
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Ok(PathSnapshot::Other);
    }
    let permissions = metadata.permissions();
    Ok(PathSnapshot::Regular {
        bytes: fs::read(path)?,
        readonly: permissions.readonly(),
        #[cfg(unix)]
        mode: {
            use std::os::unix::fs::PermissionsExt as _;
            permissions.mode() & 0o777
        },
    })
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

/// Return an untouched refusal directly; reinstall only when the receipt or
/// executable changed, while still bringing back a resident this attempt
/// stopped before the installer refused.
fn restore_if_changed(
    repository: &Repository,
    rollback: &Rollback<'_>,
    before: &InstallSnapshot,
    output: &mut impl Write,
    cause: impl Into<RefreshError>,
) -> RefreshError {
    let cause = cause.into();
    if snapshot_install(repository, rollback.installed).is_ok_and(|after| after == *before) {
        restore_resident(repository, rollback, output);
        cause
    } else {
        restore(repository, rollback, output, cause)
    }
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
    let discovery = wire::decode::<spice::canvas::Discovery>(&bytes).ok()?;
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
///
/// A published discovery outlives the process that wrote it, so `down` failing
/// is not itself a refusal: a resident that was already gone refuses the same
/// way one that is still serving does, and only the endpoint separates them.
/// The status is therefore classified against what is still answering. A clean
/// stop waits for the discovery to withdraw; a refusal with nothing answering
/// is a resident already gone and the refresh proceeds; a refusal with the
/// endpoint still live is named here, because waiting it out spends the whole
/// shutdown horizon to report a stop that was refused outright.
///
/// `down` answers its own reply on standard output, which is this command's
/// progress stream and not a result the developer asked for, so that stream is
/// captured and dropped. Its diagnostics are captured too and republished here:
/// an inherited stream puts the refusal wherever the parent's happens to go,
/// which is not this command's own report and is not what the error carries.
fn stop(
    repository: &Repository,
    installed: &Path,
    output: &mut impl Write,
) -> Result<(), RefreshError> {
    writeln!(output, "stopping resident")?;
    let answer = Command::new(installed)
        .arg("down")
        .current_dir(repository.worktree_root())
        .output()?;
    if !answer.status.success() {
        let refusal = refusal_text(&answer.stderr);
        writeln!(output, "down refused ({}): {refusal}", answer.status)?;
        if answering(repository) {
            return Err(RefreshError::ResidentStopRefused(refusal));
        }
        writeln!(output, "nothing answers the endpoint; continuing")?;
        return Ok(());
    }
    // `spice down` does not return until discovery is withdrawn and the
    // resident lock has been reacquired by its client-side release verifier.
    // This one postcondition check catches a broken contract without creating a
    // second wait mechanism in the upgrader.
    if answering(repository) {
        return Err(RefreshError::ResidentStop);
    }
    Ok(())
}

/// Whether the published discovery still names an endpoint that answers.
fn answering(repository: &Repository) -> bool {
    read_endpoint(repository).is_some_and(|endpoint| listening(&endpoint))
}

/// Everything a refused `down` said, as one line this command can report.
///
/// Joining rather than taking the last line keeps a multi-line refusal whole,
/// because the line that names the reason is not reliably the last one, and a
/// refusal reported in part is a refusal the developer has to go looking for.
fn refusal_text(diagnostics: &[u8]) -> String {
    let text = String::from_utf8_lossy(diagnostics)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    if text.is_empty() {
        "no diagnostic".to_owned()
    } else {
        text
    }
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
    let starting = Instant::now();
    let mut command = restart_command(installed, endpoint, request);
    let answer = command.current_dir(repository.worktree_root()).output()?;
    if !answer.status.success() {
        let refusal = refusal_text(&answer.stderr);
        writeln!(
            output,
            "resident start refused ({}): {refusal}",
            answer.status
        )?;
        return Err(RefreshError::ResidentStartRefused(refusal));
    }
    let ready =
        wire::decode::<wire::Outcome<wire::CanvasOpen>>(&answer.stdout).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("resident readiness reply did not decode: {error}"),
            )
        })?;
    let wire::Outcome::Reply(ready) = ready else {
        return Err(RefreshError::ResidentStartRefused(format!(
            "successful resident launcher returned {ready:?}"
        )));
    };
    writeln!(
        output,
        "resident ready at {} in {:.1?}",
        ready.body.endpoint,
        starting.elapsed()
    )?;
    Ok(Some(ready.body.endpoint))
}

/// Construct the detached launcher command, with explicit forwarded endpoint
/// options taking precedence over the endpoint inherited from discovery.
fn restart_command(
    installed: &Path,
    endpoint: (Option<&str>, Option<u16>),
    request: &RefreshRequest,
) -> Command {
    let mut command = Command::new(installed);
    command.args(["--json", "up"]);
    let (host, port) = endpoint;
    if !forwards_option(&request.up_arguments, "--host")
        && let Some(host) = host
    {
        command.args(["--host", host]);
    }
    if !forwards_option(&request.up_arguments, "--port")
        && let Some(port) = port
    {
        command.args(["--port", &port.to_string()]);
    }
    command.args(&request.up_arguments);
    command
}

/// Whether a forwarded argv names one long option, either separately or with
/// its value joined by `=`. Only these two spellings suppress inherited state.
fn forwards_option(arguments: &[OsString], option: &str) -> bool {
    let option_os = OsStr::new(option);
    arguments.iter().any(|argument| {
        argument == option_os
            || argument
                .to_str()
                .and_then(|argument| argument.strip_prefix(option))
                .is_some_and(|suffix| suffix.starts_with('='))
    })
}

/// Whether a published endpoint accepts a connection right now.
fn listening(endpoint: &str) -> bool {
    endpoint_parts(endpoint)
        .is_some_and(|(host, port)| TcpStream::connect((host.as_str(), port)).map(drop).is_ok())
}
#[cfg(test)]
mod tests;
