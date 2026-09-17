//! The detached `spice up` launcher and its one-shot child handoff.

use super::*;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
pub(super) mod observer;

#[cfg(unix)]
use observer::{DirectObserver, Signal, wait_for_change, wait_for_exit};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

const HANDOFF_FD: i32 = 9;
const HANDOFF_MAGIC: &[u8] = b"SPICE-RS-DAEMON\0";
const MAX_HANDOFF_BYTES: usize = 16 * 1024;
const MAX_BOOTSTRAP_BYTES: u64 = 64 * 1024;
const ARRIVAL_HORIZON: Duration = Duration::from_secs(60);
/// When an unfinished wait logs a warning rather than refusing outright.
const ARRIVAL_WARN_HORIZON: Duration = Duration::from_secs(30);
const PROBE_BOUND: Duration = Duration::from_secs(20);
const TERMINATION_HORIZON: Duration = Duration::from_secs(5);

/// The only secret crossing from the launcher into its child.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HandoffRequest {
    token: Option<String>,
}

/// The detached child's half of the launch channel.
#[derive(Clone)]
pub(crate) struct ChildStatus {
    #[cfg(unix)]
    stream: Arc<Mutex<Option<UnixStream>>>,
}

/// Authentication and status state captured by the controlled foreground child.
pub(crate) struct ChildLaunch {
    pub(crate) token: Option<String>,
    pub(crate) status: ChildStatus,
}

impl ChildLaunch {
    /// Capture a launcher-created handoff, if this is a detached child.
    pub(crate) fn capture() -> Result<Option<Self>, Outcome<()>> {
        #[cfg(not(unix))]
        {
            return Ok(None);
        }
        #[cfg(unix)]
        {
            if !handoff_present(HANDOFF_FD)? {
                return Ok(None);
            }
            // SAFETY: the launcher exclusively transfers ownership of this
            // inherited descriptor to the controlled child.
            let mut stream = unsafe { UnixStream::from_raw_fd(HANDOFF_FD) };
            stream
                .set_read_timeout(Some(PROBE_BOUND))
                .map_err(|error| {
                    environment_fault("resident::daemon-handoff-unavailable", &error.to_string())
                })?;
            let request = read_framed(&mut stream)?;
            let request: HandoffRequest = wire::decode(&request).map_err(|error| {
                environment_fault("resident::daemon-handoff-invalid", &error.to_string())
            })?;
            let _ = stream.shutdown(Shutdown::Read);
            Ok(Some(Self {
                token: request.token,
                status: ChildStatus {
                    stream: Arc::new(Mutex::new(Some(stream))),
                },
            }))
        }
    }
}

impl ChildStatus {
    /// Close the last launcher channel as soon as discovery is published.
    pub(crate) fn published(self) {
        #[cfg(unix)]
        drop(self.stream.lock().expect("daemon status mutex").take());
    }

    /// Return an exact typed startup outcome when publication never happened.
    pub(crate) fn failed(self, outcome: &Outcome<()>) {
        #[cfg(unix)]
        if let Some(mut stream) = self.stream.lock().expect("daemon status mutex").take() {
            if let Ok(bytes) = wire::encode(outcome) {
                let _ = write_framed(&mut stream, &bytes);
            }
            let _ = stream.shutdown(Shutdown::Both);
        }
        #[cfg(not(unix))]
        let _ = outcome;
    }
}

/// Start this exact executable as a detached resident and prove it is ready.
pub(crate) fn launch(leaf: &clap::ArgMatches) -> Outcome<wire::CanvasOpen> {
    #[cfg(not(unix))]
    {
        let _ = leaf;
        return map_failure(environment_fault(
            "resident::daemon-unsupported",
            "detached resident launch requires Unix process groups and descriptor inheritance",
        ));
    }
    #[cfg(unix)]
    launch_unix(leaf)
}

#[cfg(unix)]
fn launch_unix(leaf: &clap::ArgMatches) -> Outcome<wire::CanvasOpen> {
    match PreparedLaunch::admit(leaf).and_then(|launch| launch.spawn(leaf)) {
        Ok(launch) => launch.wait(),
        Err(outcome) => map_failure(outcome),
    }
}

#[cfg(unix)]
struct PreparedLaunch {
    listen: SocketAddr,
    token: Option<String>,
    current: PathBuf,
    state: spice::repository::StatePaths,
    baseline: Option<Vec<u8>>,
    observer: DirectObserver,
    executable: PathBuf,
    log: File,
    log_offset: u64,
    parent_handoff: UnixStream,
    child_handoff: UnixStream,
}

#[cfg(unix)]
impl PreparedLaunch {
    fn admit(leaf: &clap::ArgMatches) -> Result<Self, Outcome<()>> {
        let listen = requested_listen(leaf)?;
        let token = requested_access(leaf, listen)?.token;
        let current = std::env::current_dir()
            .map_err(|error| daemon_fault("resident::cwd-unavailable", error))?;
        let repository = Repository::discover(&current)
            .map_err(|error| daemon_fault("resident::git-unavailable", error))?;
        let state = repository
            .admit_resident_state()
            .map_err(|error| crate::state_failure(&error).outcome())?;
        fs::create_dir_all(state.runtime())
            .map_err(|error| daemon_fault("resident::runtime-unavailable", error))?;
        let baseline = read_optional(state.canvas())
            .map_err(|error| daemon_fault("resident::daemon-discovery-unavailable", error))?;
        let observer = DirectObserver::arm(state.runtime())
            .map_err(|error| daemon_fault("resident::daemon-observer-unavailable", error))?;
        let executable = std::env::current_exe()
            .map_err(|error| daemon_fault("resident::executable-unavailable", error))?;
        let log = open_log(state.resident_log())
            .map_err(|error| daemon_fault("resident::log-unavailable", error))?;
        let log_offset = log.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let (parent_handoff, child_handoff) = prepare_handoff(token.clone())?;
        Ok(Self {
            listen,
            token,
            current,
            state,
            baseline,
            observer,
            executable,
            log,
            log_offset,
            parent_handoff,
            child_handoff,
        })
    }

    fn spawn(self, leaf: &clap::ArgMatches) -> Result<SpawnedLaunch, Outcome<()>> {
        let Self {
            listen,
            token,
            current,
            state,
            baseline,
            observer,
            executable,
            log,
            log_offset,
            parent_handoff,
            child_handoff,
        } = self;
        let stdout = log
            .try_clone()
            .map_err(|error| daemon_fault("resident::log-unavailable", error))?;
        let mut command = ProcessCommand::new(&executable);
        command
            .args([
                "--json",
                "up",
                "--foreground",
                "--host",
                &listen.ip().to_string(),
                "--port",
                &listen.port().to_string(),
            ])
            .current_dir(&current)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(log)
            .process_group(0);
        if leaf.get_flag("allow-insecure-bind") {
            command.arg("--allow-insecure-bind");
        }
        if leaf.get_flag("open") {
            command.arg("--open");
        }
        inherit_handoff(&mut command, child_handoff.as_raw_fd());
        let child = command
            .spawn()
            .map_err(|error| daemon_fault("resident::daemon-launch-failed", error))?;
        drop(child_handoff);
        Ok(SpawnedLaunch {
            token,
            state,
            baseline,
            observer,
            log_offset,
            parent_handoff,
            child,
        })
    }
}

#[cfg(unix)]
struct SpawnedLaunch {
    token: Option<String>,
    state: spice::repository::StatePaths,
    baseline: Option<Vec<u8>>,
    observer: DirectObserver,
    log_offset: u64,
    parent_handoff: UnixStream,
    child: Child,
}

#[cfg(unix)]
impl SpawnedLaunch {
    fn wait(mut self) -> Outcome<wire::CanvasOpen> {
        let ready = wait_for_ready(
            &mut self.child,
            self.observer,
            self.state.runtime(),
            self.state.canvas(),
            self.baseline.as_deref(),
            self.token.as_deref(),
            self.state.resident_log(),
            &mut self.parent_handoff,
        );
        match ready {
            Ok(discovery) => Outcome::Reply(Reply {
                position: ReplyPosition::Direct {},
                body: wire::CanvasOpen {
                    endpoint: discovery.endpoint,
                    epoch: discovery.epoch,
                },
            }),
            Err(outcome) => {
                terminate_group(&mut self.child);
                outcome.map_or_else(
                    || {
                        map_failure(environment_fault(
                            "resident::daemon-child-exited",
                            &child_diagnostic(self.state.resident_log(), self.log_offset),
                        ))
                    },
                    map_failure,
                )
            }
        }
    }
}

#[cfg(unix)]
fn daemon_fault(reason: &str, error: impl std::fmt::Display) -> Outcome<()> {
    environment_fault(reason, &error.to_string())
}

#[cfg(unix)]
fn prepare_handoff(token: Option<String>) -> Result<(UnixStream, UnixStream), Outcome<()>> {
    let (mut parent, child) = UnixStream::pair()
        .map_err(|error| daemon_fault("resident::daemon-handoff-unavailable", error))?;
    let request = HandoffRequest { token };
    let encoded = wire::encode(&request).expect("daemon handoff request encodes");
    write_framed(&mut parent, &encoded)
        .map_err(|error| daemon_fault("resident::daemon-handoff-unavailable", error))?;
    let _ = parent.shutdown(Shutdown::Write);
    Ok((parent, child))
}

#[cfg(unix)]
fn inherit_handoff(command: &mut ProcessCommand, source: RawFd) {
    // SAFETY: dup2 and fcntl are async-signal-safe, and the closure touches
    // only copied descriptor integers before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(source, HANDOFF_FD) == -1 {
                return Err(io::Error::last_os_error());
            }
            let flags = libc::fcntl(HANDOFF_FD, libc::F_GETFD);
            if flags == -1
                || libc::fcntl(HANDOFF_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Note an unfinished readiness wait in the resident log, best-effort.
///
/// A missed write here must not fail the wait itself: this is an operator
/// aid for a still-running launch, not a condition the caller can act on.
#[cfg(unix)]
fn warn_readiness_pending(log_path: &Path, warn_horizon: Duration, horizon: Duration) {
    let Ok(mut log) = open_log(log_path) else {
        return;
    };
    let _ = writeln!(
        log,
        "resident readiness still pending after {warn_horizon:?}; continuing to wait up to {horizon:?}"
    );
}

#[cfg(unix)]
fn open_log(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .mode(0o600)
        .open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(unix)]
#[allow(
    clippy::too_many_arguments,
    reason = "the waiter owns one fixed launch observation"
)]
fn wait_for_ready(
    child: &mut Child,
    observer: DirectObserver,
    watched: &Path,
    canvas: &Path,
    baseline: Option<&[u8]>,
    token: Option<&str>,
    log_path: &Path,
    status: &mut UnixStream,
) -> Result<canvas::Discovery, Option<Outcome<()>>> {
    let horizon = arrival_horizon(watched);
    let started = Instant::now();
    let deadline = started + horizon;
    let warn_at = started + horizon.min(ARRIVAL_WARN_HORIZON);
    let mut warned = false;
    let (sender, receiver) = mpsc::channel();
    wait_for_change(observer, sender.clone(), Signal::woke);
    wait_for_exit(child.id(), sender.clone());

    if let Some(discovery) = candidate(canvas, baseline, token)? {
        return Ok(discovery);
    }
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Some(environment_fault(
                "resident::daemon-readiness-elapsed",
                &format!("resident did not become ready within {horizon:?}"),
            )));
        }
        if !warned && Instant::now() >= warn_at {
            warned = true;
            warn_readiness_pending(log_path, ARRIVAL_WARN_HORIZON, horizon);
        }
        let wait_bound = remaining.min(warn_at.saturating_duration_since(Instant::now()));
        let wait_bound = if wait_bound.is_zero() {
            remaining
        } else {
            wait_bound
        };
        match receiver.recv_timeout(wait_bound) {
            Ok(Signal::Changed) => {
                let next = DirectObserver::arm(watched).map_err(|error| {
                    Some(environment_fault(
                        "resident::daemon-observer-unavailable",
                        &error.to_string(),
                    ))
                })?;
                if let Some(discovery) = candidate(canvas, baseline, token)? {
                    return Ok(discovery);
                }
                wait_for_change(next, sender.clone(), Signal::woke);
            }
            Ok(Signal::Exited) => {
                if let Some(discovery) = candidate(canvas, baseline, token)? {
                    return Ok(discovery);
                }
                let _ = child.wait();
                let bytes = read_status(status).map_err(|error| {
                    Some(environment_fault(
                        "resident::daemon-handoff-unavailable",
                        &error.to_string(),
                    ))
                })?;
                if bytes.is_empty() {
                    return Err(None);
                }
                let outcome = wire::decode::<Outcome<()>>(&bytes).map_err(|error| {
                    Some(environment_fault(
                        "resident::daemon-outcome-invalid",
                        &error.to_string(),
                    ))
                })?;
                return Err(Some(outcome));
            }
            Ok(Signal::ObserverFailed(error)) => {
                return Err(Some(environment_fault(
                    "resident::daemon-observer-failed",
                    &error,
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // This bound is the warn point, not necessarily the real
                // deadline: loop back so the top of the loop re-checks
                // `remaining` and raises the readiness-elapsed refusal only
                // once the full horizon has actually passed.
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Some(environment_fault(
                    "resident::daemon-observer-failed",
                    "resident readiness completion channel disconnected",
                )));
            }
        }
    }
}

#[cfg(unix)]
fn arrival_horizon(runtime: &Path) -> Duration {
    // The feature only makes the selected negative fixture marker legible. A
    // shared featured appliance with no marker retains production readiness.
    #[cfg(feature = "test-short-horizons")]
    if runtime.join("test-short-daemon-arrival").is_file() {
        return Duration::from_secs(3);
    }
    #[cfg(not(feature = "test-short-horizons"))]
    let _ = runtime;
    ARRIVAL_HORIZON
}

#[cfg(unix)]
fn candidate(
    canvas: &Path,
    baseline: Option<&[u8]>,
    token: Option<&str>,
) -> Result<Option<canvas::Discovery>, Option<Outcome<()>>> {
    let Some(bytes) = read_optional(canvas).map_err(|error| {
        Some(environment_fault(
            "resident::daemon-discovery-unavailable",
            &error.to_string(),
        ))
    })?
    else {
        return Ok(None);
    };
    if baseline == Some(bytes.as_slice()) {
        return Ok(None);
    }
    let discovery = wire::decode::<canvas::Discovery>(&bytes).map_err(|error| {
        Some(environment_fault(
            "resident::daemon-discovery-invalid",
            &error.to_string(),
        ))
    })?;
    probe(&discovery, token).map_err(Some)?;
    Ok(Some(discovery))
}

fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn probe(discovery: &canvas::Discovery, token: Option<&str>) -> CommandResult<()> {
    let authority = discovery.endpoint.strip_prefix("http://").ok_or_else(|| {
        environment_fault(
            "resident::daemon-endpoint-invalid",
            "published resident endpoint is not an HTTP origin",
        )
    })?;
    let address = authority.parse::<SocketAddr>().map_err(|error| {
        environment_fault("resident::daemon-endpoint-invalid", &error.to_string())
    })?;
    let mut stream = TcpStream::connect_timeout(&address, PROBE_BOUND)
        .map_err(|error| environment_fault("resident::daemon-probe-failed", &error.to_string()))?;
    stream
        .set_read_timeout(Some(PROBE_BOUND))
        .map_err(|error| environment_fault("resident::daemon-probe-failed", &error.to_string()))?;
    stream
        .set_write_timeout(Some(PROBE_BOUND))
        .map_err(|error| environment_fault("resident::daemon-probe-failed", &error.to_string()))?;
    let route = token.map_or_else(
        || "/bootstrap".to_owned(),
        |token| format!("/bootstrap?auth={token}"),
    );
    write!(
        stream,
        "GET {route} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .and_then(|()| stream.flush())
    .map_err(|error| environment_fault("resident::daemon-probe-failed", &error.to_string()))?;
    let mut response = Vec::new();
    stream
        .take(MAX_BOOTSTRAP_BYTES)
        .read_to_end(&mut response)
        .map_err(|error| environment_fault("resident::daemon-probe-failed", &error.to_string()))?;
    if !response.starts_with(b"HTTP/1.1 200 OK\r\n") {
        let status = response
            .split(|byte| *byte == b'\n')
            .next()
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        return Err(environment_fault(
            "resident::daemon-probe-refused",
            status.trim(),
        ));
    }
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| {
            environment_fault(
                "resident::daemon-probe-invalid",
                "bootstrap response had no header terminator",
            )
        })?;
    let bootstrap = wire::decode::<wire::CanvasBootstrap>(&response[split..])
        .map_err(|error| environment_fault("resident::daemon-probe-invalid", &error.to_string()))?;
    if bootstrap.epoch != discovery.epoch
        || bootstrap.capability != discovery.capability
        || bootstrap.endpoint != discovery.endpoint
    {
        return Err(environment_fault(
            "resident::daemon-generation-moved",
            "authenticated bootstrap did not match the published resident generation",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn terminate_group(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
    let (sender, receiver) = mpsc::sync_channel(1);
    wait_for_exit(child.id(), sender);
    if receiver.recv_timeout(TERMINATION_HORIZON).is_err() {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.wait();
}

#[cfg(unix)]
fn child_diagnostic(path: &Path, offset: u64) -> String {
    let bytes = fs::read(path).unwrap_or_default();
    let start = usize::try_from(offset)
        .unwrap_or(bytes.len())
        .min(bytes.len());
    let recent = &bytes[start..];
    let recent = &recent[recent.len().saturating_sub(4096)..];
    let detail = String::from_utf8_lossy(recent).trim().to_owned();
    if detail.is_empty() {
        "detached child exited before publishing discovery and left no diagnostic".to_owned()
    } else {
        format!("detached child exited before publishing discovery: {detail}")
    }
}

fn map_failure(outcome: Outcome<()>) -> Outcome<wire::CanvasOpen> {
    match outcome {
        Outcome::Reply(_) => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "resident::daemon-outcome-invalid".to_owned(),
            detail: "detached startup returned a terminal reply before readiness".to_owned(),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn write_framed(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    stream.write_all(HANDOFF_MAGIC)?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

fn read_framed(stream: &mut impl Read) -> Result<Vec<u8>, Outcome<()>> {
    let mut magic = vec![0; HANDOFF_MAGIC.len()];
    stream.read_exact(&mut magic).map_err(|error| {
        environment_fault("resident::daemon-handoff-unavailable", &error.to_string())
    })?;
    if magic != HANDOFF_MAGIC {
        return Err(environment_fault(
            "resident::daemon-handoff-invalid",
            "detached child handoff did not carry the internal protocol marker",
        ));
    }
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).map_err(|error| {
        environment_fault("resident::daemon-handoff-unavailable", &error.to_string())
    })?;
    let length = usize::try_from(u32::from_be_bytes(length)).map_err(|error| {
        environment_fault("resident::daemon-handoff-invalid", &error.to_string())
    })?;
    if length > MAX_HANDOFF_BYTES {
        return Err(environment_fault(
            "resident::daemon-handoff-invalid",
            "detached child handoff exceeded its fixed byte bound",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).map_err(|error| {
        environment_fault("resident::daemon-handoff-unavailable", &error.to_string())
    })?;
    Ok(bytes)
}

#[cfg(unix)]
fn read_status(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut magic = vec![0; HANDOFF_MAGIC.len()];
    match stream.read_exact(&mut magic) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    if magic != HANDOFF_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "detached child outcome had no internal protocol marker",
        ));
    }
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if length > MAX_HANDOFF_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "detached child outcome exceeded its fixed byte bound",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn handoff_present(descriptor: RawFd) -> Result<bool, Outcome<()>> {
    let mut prefix = vec![0_u8; HANDOFF_MAGIC.len()];
    // SAFETY: recv only inspects the named descriptor and writes within prefix.
    let read = unsafe {
        libc::recv(
            descriptor,
            prefix.as_mut_ptr().cast(),
            prefix.len(),
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if read == -1 {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::EBADF) | Some(libc::ENOTSOCK) | Some(libc::EAGAIN) => Ok(false),
            _ => Err(environment_fault(
                "resident::daemon-handoff-unavailable",
                &error.to_string(),
            )),
        };
    }
    let read = usize::try_from(read).map_err(|error| {
        environment_fault("resident::daemon-handoff-unavailable", &error.to_string())
    })?;
    Ok(read == HANDOFF_MAGIC.len() && prefix == HANDOFF_MAGIC)
}
