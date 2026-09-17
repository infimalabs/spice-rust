//! A job: one command the lane keeps beyond the call that started it.
//!
//! Every command the occupant runs is kept by a detached keeper process,
//! `spice __lane-job`, that owns nothing but the command: it runs the argv
//! in the command's directory under its environment with both streams
//! appended to one log, ends the command's process group at its deadline or
//! when asked to stop, and publishes how the command ended. The keeper is its
//! own process group under nobody's supervision, so the command survives the
//! turn, the occupant, and the supervisor that started it; its result waits
//! in the job's state for whoever asks. The lane observes a job through its
//! durable state: a direct kernel watch on the job's directory wakes every
//! read, and the keeper's exit names a keeper that died without an ending.
//! The keeper reads its stop request the same way.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant as Monotonic};

use serde::{Deserialize, Serialize};
use sill::Environment;
use wire::CanonicalUuid;

use super::super::super::RuntimePaths;
use super::super::super::daemon::observer;
use super::super::{
    REQUEST_BYTES, STATE_BYTES, now_millis, process_exists_raw, publish_json, read_json,
    signal_group,
};

/// The hidden entry the keeper runs under.
pub(in crate::resident::supervisor) const ENTRY: &str = "__lane-job";

const VERSION: u32 = 1;
const REQUEST: &str = "request.json";
const STATE: &str = "state.json";
const LOG: &str = "log";
const STOP: &str = "stop";

/// Bytes of the log a reading keeps: the end, where a failure explains
/// itself.
pub(super) const LOG_TAIL: usize = 64 * 1024;

/// How long a command has to leave on its own once the keeper ends it.
const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// What the keeper reads: the command as the model wrote it, the argv the
/// lane runs, and where and under what to run it.
#[derive(Deserialize, Serialize)]
struct Request {
    version: u32,
    id: String,
    command: String,
    argv: Vec<OsString>,
    cwd: PathBuf,
    environment: Vec<(OsString, OsString)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_millis: Option<u64>,
    /// What the results carry ahead of the command's output, as omp noted it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
}

/// What the keeper publishes: who keeps the command, and how it ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::resident) struct State {
    version: u32,
    pub(in crate::resident) id: String,
    pub(in crate::resident) command: String,
    pub(in crate::resident) keeper: u32,
    pub(in crate::resident) started_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) timeout_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) ending: Option<Ending>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::resident) ended_millis: Option<u64>,
}

impl State {
    /// The ending with its wall time, once the keeper has published one.
    fn ended(&self) -> Option<Ended> {
        let ending = self.ending.clone()?;
        let wall = self
            .ended_millis
            .map(|ended| Duration::from_millis(ended.saturating_sub(self.started_millis)));
        Some(Ended { ending, wall })
    }
}

/// How a job ended and how long it ran, as the keeper recorded them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) struct Ended {
    pub(in crate::resident) ending: Ending,
    /// From the keeper's start to the ending; unknown for a lost job.
    pub(in crate::resident) wall: Option<Duration>,
}

/// How a kept command ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::resident) enum Ending {
    Exited {
        code: i32,
    },
    Signalled {
        signal: i32,
    },
    TimedOut,
    Stopped,
    Failed {
        reason: String,
    },
    /// The keeper is gone without publishing an ending; read, never written.
    Lost,
}

/// What the keeper's loop waits on.
enum Keep {
    Exited(std::process::ExitStatus),
    /// The job's directory changed: a stop may have been asked.
    Changed,
    Failed(String),
}

impl Keep {
    fn woke(woke: io::Result<()>) -> Self {
        woke.map_or_else(|error| Self::Failed(error.to_string()), |()| Self::Changed)
    }
}

/// Keep one command: run it, end it at its deadline or on request, publish
/// how it ended.
///
/// # Errors
///
/// Returns the failure of reading the request or publishing the state; a
/// command that cannot run is an ending, not an error.
pub(in crate::resident::supervisor) fn keep(request_path: &Path) -> io::Result<()> {
    let directory = request_path
        .parent()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "job request has no directory"))?;
    let request: Request = read_json(request_path, REQUEST_BYTES)?;
    if request.version != VERSION {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("unsupported job request version {}", request.version),
        ));
    }
    let state_path = directory.join(STATE);
    let mut state = State {
        version: VERSION,
        id: request.id.clone(),
        command: request.command.clone(),
        keeper: std::process::id(),
        started_millis: now_millis()?,
        timeout_millis: request.timeout_millis,
        ending: None,
        ended_millis: None,
    };
    publish_json(&state_path, &state)?;
    let (sender, receiver) = mpsc::channel();
    state.ending = Some(match spawn(directory, &request, sender.clone()) {
        Ok(child) => {
            let stop = directory.join(STOP);
            tend(
                child,
                &receiver,
                request.timeout_millis.map(Duration::from_millis),
                || watch(directory, &sender, Keep::woke).map(|()| stop.exists()),
            )
        }
        Err(reason) => Ending::Failed { reason },
    });
    state.ended_millis = Some(now_millis()?);
    publish_json(&state_path, &state)
}

/// Start the command in its own group with both streams appended to the log
/// and a waiter reporting its exit.
fn spawn(directory: &Path, request: &Request, sender: mpsc::Sender<Keep>) -> Result<u32, String> {
    let (program, arguments) = request
        .argv
        .split_first()
        .ok_or_else(|| "empty argv".to_owned())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(LOG))
        .map_err(|error| format!("log unavailable: {error}"))?;
    let stderr = log
        .try_clone()
        .map_err(|error| format!("log unavailable: {error}"))?;
    let mut command = ProcessCommand::new(program);
    command
        .args(arguments)
        .current_dir(&request.cwd)
        .env_clear()
        .envs(request.environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr);
    detach(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pid = child.id();
    thread::Builder::new()
        .name("spice-job-waiter".to_owned())
        .spawn(move || {
            let _ = sender.send(match child.wait() {
                Ok(status) => Keep::Exited(status),
                Err(error) => Keep::Failed(error.to_string()),
            });
        })
        .map_err(|error| format!("waiter unavailable: {error}"))?;
    Ok(pid)
}

/// Wait for the command to end, ending it at its deadline or on request:
/// the group is asked to leave, then killed once the grace elapses. Without
/// a deadline the command runs until it ends or a stop is asked.
///
/// `stop_asked` arms one watch on the job's directory and says whether a
/// stop has been asked by now; it is consulted on every wake, so an
/// unwatchable directory is read as a stop rather than as a command nobody
/// can reach.
fn tend(
    group: u32,
    receiver: &mpsc::Receiver<Keep>,
    timeout: Option<Duration>,
    stop_asked: impl Fn() -> io::Result<bool>,
) -> Ending {
    let mut deadline = timeout.map(|timeout| Monotonic::now() + timeout);
    let mut pending: Option<Ending> = None;
    let mut changed = true;
    loop {
        if changed && pending.is_none() && stop_asked().unwrap_or(true) {
            pending = Some(Ending::Stopped);
            let _ = signal_group(group, libc::SIGTERM);
            deadline = Some(Monotonic::now() + TERMINATE_GRACE);
        }
        changed = false;
        let remaining = deadline.map_or(Duration::MAX, |deadline| {
            deadline.saturating_duration_since(Monotonic::now())
        });
        match receiver.recv_timeout(remaining) {
            Ok(Keep::Exited(status)) => {
                return match pending {
                    Some(ending) => swept(group, ending),
                    None => exited(status),
                };
            }
            Ok(Keep::Failed(reason)) => return Ending::Failed { reason },
            Ok(Keep::Changed) => changed = true,
            Err(mpsc::RecvTimeoutError::Timeout) if pending.is_none() => {
                pending = Some(Ending::TimedOut);
                let _ = signal_group(group, libc::SIGTERM);
                deadline = Some(Monotonic::now() + TERMINATE_GRACE);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = signal_group(group, libc::SIGKILL);
                return match receiver.recv() {
                    Ok(Keep::Failed(reason)) => Ending::Failed { reason },
                    Ok(_) | Err(_) => swept(group, pending.unwrap_or(Ending::Stopped)),
                };
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ending::Failed {
                    reason: "the command's waiter ended without a status".to_owned(),
                };
            }
        }
    }
}

/// The ending of a group asked to end, once its leader has died: a leader
/// signalled mid-fork leaves a child the signal never reached, so whoever
/// is left in the group is asked to leave and, after the grace, killed. An
/// empty group refuses the first signal, so the common case waits nothing.
fn swept(group: u32, ending: Ending) -> Ending {
    if signal_group(group, libc::SIGTERM).is_ok() {
        thread::sleep(TERMINATE_GRACE);
        let _ = signal_group(group, libc::SIGKILL);
    }
    ending
}

#[cfg(unix)]
fn exited(status: std::process::ExitStatus) -> Ending {
    use std::os::unix::process::ExitStatusExt as _;
    match (status.code(), status.signal()) {
        (Some(code), _) => Ending::Exited { code },
        (None, Some(signal)) => Ending::Signalled { signal },
        (None, None) => Ending::Failed {
            reason: "the command ended without a status".to_owned(),
        },
    }
}

#[cfg(not(unix))]
fn exited(status: std::process::ExitStatus) -> Ending {
    match status.code() {
        Some(code) => Ending::Exited { code },
        None => Ending::Failed {
            reason: "the command ended without a status".to_owned(),
        },
    }
}

#[cfg(unix)]
fn detach(command: &mut ProcessCommand) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn detach(_command: &mut ProcessCommand) {}

/// Send what `signal` makes of the directory's next change; the watch is
/// armed before the reader looks.
fn watch<T: Send + 'static>(
    directory: &Path,
    sender: &mpsc::Sender<T>,
    signal: impl FnOnce(io::Result<()>) -> T + Send + 'static,
) -> io::Result<()> {
    let observer = observer::DirectObserver::arm(directory)?;
    observer::wait_for_change(observer, sender.clone(), signal);
    Ok(())
}

/// What reaches a job's observer: the job's own progress, and the lane's
/// word on the call that is observing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) enum Signal {
    Changed,
    KeeperExited,
    ObserverFailed(String),
    /// The call this observation answers was withdrawn.
    Cancel,
    /// A message arrived for the occupant; the observation should yield.
    Interrupt,
}

impl Signal {
    fn woke(woke: io::Result<()>) -> Self {
        woke.map_or_else(
            |error| Self::ObserverFailed(error.to_string()),
            |()| Self::Changed,
        )
    }
}

impl observer::ExitSender for mpsc::Sender<Signal> {
    fn exited(self) {
        let _ = self.send(Signal::KeeperExited);
    }

    fn failed(self, error: String) {
        let _ = self.send(Signal::ObserverFailed(error));
    }
}

/// One call's observation channel: the job's progress and the lane's
/// interruptions arrive on it together, so a wait needs no polling.
pub(in crate::resident) struct Control {
    sender: mpsc::Sender<Signal>,
    receiver: mpsc::Receiver<Signal>,
}

/// The lane's end of one call's control: cancel it, or interrupt it.
#[derive(Clone)]
pub(in crate::resident) struct Handle(mpsc::Sender<Signal>);

impl Control {
    pub(in crate::resident) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sender, receiver }
    }

    pub(in crate::resident) fn handle(&self) -> Handle {
        Handle(self.sender.clone())
    }
}

impl Handle {
    pub(in crate::resident) fn cancel(&self) {
        let _ = self.0.send(Signal::Cancel);
    }

    pub(in crate::resident) fn interrupt(&self) {
        let _ = self.0.send(Signal::Interrupt);
    }
}

/// How one bounded observation of a job ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) enum Observed {
    Ended(Ended),
    Cancelled,
    Interrupted,
    Elapsed,
}

/// The last of a job's log, and whether earlier output was left behind.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::resident) struct Tail {
    pub(in crate::resident) text: String,
    pub(in crate::resident) truncated: bool,
}

/// Where one lane's jobs live.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Jobs {
    root: PathBuf,
}

/// One kept command, by the directory that records it.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Job {
    directory: PathBuf,
    id: String,
    command: String,
    timeout: Option<Duration>,
    notices: Vec<String>,
    keeper: Option<u32>,
}

impl Jobs {
    /// This lane's jobs, beneath the lane's own supervisor records.
    pub(in crate::resident) fn for_lane(paths: &RuntimePaths, lane: CanonicalUuid) -> Self {
        Self::new(paths.supervisors.join(lane.to_string()).join("jobs"))
    }

    pub(in crate::resident) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Start keeping one command.
    ///
    /// # Errors
    ///
    /// Returns the failure of recording the request or launching the keeper.
    pub(in crate::resident) fn start(
        &self,
        command: &str,
        argv: Vec<OsString>,
        cwd: &Path,
        environment: &Environment,
        timeout: Option<Duration>,
        notices: &[String],
    ) -> io::Result<Job> {
        let id = crate::resident::new_uuid()
            .to_string()
            .split('-')
            .next()
            .unwrap_or_default()
            .to_owned();
        let directory = self.root.join(&id);
        fs::create_dir_all(&directory)?;
        let request_path = directory.join(REQUEST);
        publish_json(
            &request_path,
            &Request {
                version: VERSION,
                id: id.clone(),
                command: command.to_owned(),
                argv,
                cwd: cwd.to_path_buf(),
                environment: environment.entries().to_vec(),
                timeout_millis: timeout
                    .map(|timeout| u64::try_from(timeout.as_millis()))
                    .transpose()
                    .map_err(io::Error::other)?,
                notices: notices.to_vec(),
            },
        )?;
        let keeper = launch(&directory, &request_path, cwd, environment)?;
        Ok(Job {
            directory,
            id,
            command: command.to_owned(),
            timeout,
            notices: notices.to_vec(),
            keeper,
        })
    }

    /// One job of this lane, by the identity the model quotes: a job exists
    /// from the moment its request is recorded, before its keeper speaks.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an identity that names no job.
    pub(in crate::resident) fn open(&self, id: &str) -> io::Result<Job> {
        let named = !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit());
        let directory = self.root.join(id);
        let request: Request = named
            .then(|| read_json(&directory.join(REQUEST), REQUEST_BYTES))
            .transpose()
            .map_err(|error| match error.kind() {
                ErrorKind::NotFound => io::Error::new(ErrorKind::NotFound, format!("no job {id}")),
                _ => error,
            })?
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, format!("no job {id}")))?;
        let keeper = read_state(&directory)?.map(|state| state.keeper);
        Ok(Job {
            directory,
            id: request.id,
            command: request.command,
            timeout: request.timeout_millis.map(Duration::from_millis),
            notices: request.notices,
            keeper,
        })
    }

    /// Every job of this lane, oldest request first.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the lane's job directory.
    pub(in crate::resident) fn list(&self) -> io::Result<Vec<Job>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut jobs = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            match self.open(&id) {
                Ok(job) => {
                    let requested = fs::metadata(entry.path().join(REQUEST))?.modified()?;
                    jobs.push((requested, job));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        jobs.sort_by(|(left, a), (right, b)| left.cmp(right).then_with(|| a.id.cmp(&b.id)));
        Ok(jobs.into_iter().map(|(_, job)| job).collect())
    }
}

#[cfg(not(test))]
use launch_detached as launch;
#[cfg(test)]
use launch_direct as launch;

/// Launch the keeper of one recorded request; the keeper's own streams go to
/// the job's log, and it answers to nobody once started.
#[cfg(not(test))]
fn launch_detached(
    directory: &Path,
    request_path: &Path,
    cwd: &Path,
    environment: &Environment,
) -> io::Result<Option<u32>> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(LOG))?;
    let stderr = log.try_clone()?;
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg(ENTRY)
        .arg(request_path)
        .current_dir(cwd)
        .env_clear()
        .envs(environment.entries().iter().cloned())
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr);
    detach(&mut command);
    let mut keeper = command.spawn()?;
    let pid = keeper.id();
    thread::Builder::new()
        .name("spice-job-reaper".to_owned())
        .spawn(move || {
            let _ = keeper.wait();
        })?;
    Ok(Some(pid))
}

/// Keep the request on a thread of this process: the test's own keeper. Like
/// the process, it ends quietly when the job's directory is gone.
#[cfg(test)]
fn launch_direct(
    _directory: &Path,
    request_path: &Path,
    _cwd: &Path,
    _environment: &Environment,
) -> io::Result<Option<u32>> {
    let request_path = request_path.to_path_buf();
    thread::Builder::new()
        .name("spice-job-keeper".to_owned())
        .spawn(move || drop(keep(&request_path)))?;
    Ok(None)
}

/// The state a job directory holds, with a keeper that vanished before
/// publishing an ending read as lost; `None` before the keeper has spoken.
fn read_state(directory: &Path) -> io::Result<Option<State>> {
    let mut state: State = match read_json(&directory.join(STATE), STATE_BYTES) {
        Ok(state) => state,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if state.ending.is_none()
        && state.keeper != std::process::id()
        && !process_exists_raw(state.keeper).unwrap_or(true)
    {
        state.ending = Some(Ending::Lost);
    }
    Ok(Some(state))
}

impl Job {
    pub(in crate::resident) fn id(&self) -> &str {
        &self.id
    }

    pub(in crate::resident) fn command(&self) -> &str {
        &self.command
    }

    /// How long the command may run before the keeper ends it, if bounded.
    pub(in crate::resident) fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// What the results carry ahead of the command's output.
    pub(in crate::resident) fn notices(&self) -> &[String] {
        &self.notices
    }

    /// How the job ended, if it has.
    pub(in crate::resident) fn ended(&self) -> Option<Ended> {
        self.state().ok().flatten().and_then(|state| state.ended())
    }

    /// The job's published state, if the keeper has spoken.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the state document.
    pub(in crate::resident) fn state(&self) -> io::Result<Option<State>> {
        read_state(&self.directory)
    }

    /// Ask the keeper to end the command.
    ///
    /// # Errors
    ///
    /// Returns the failure of writing the stop request.
    pub(in crate::resident) fn stop(&self) -> io::Result<()> {
        File::create(self.directory.join(STOP)).map(drop)
    }

    /// The last of the log, bounded.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the log; a log not yet written is empty.
    pub(in crate::resident) fn tail(&self) -> io::Result<Tail> {
        let mut file = match File::open(self.directory.join(LOG)) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Tail::default()),
            Err(error) => return Err(error),
        };
        let length = file.metadata()?.len();
        let limit = u64::try_from(LOG_TAIL).map_err(io::Error::other)?;
        let truncated = length > limit;
        if truncated {
            file.seek(SeekFrom::End(
                -i64::try_from(limit).map_err(io::Error::other)?,
            ))?;
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit)
            .read_to_end(&mut bytes)?;
        Ok(Tail {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            truncated,
        })
    }

    /// Observe the job until it ends, the call is cancelled or interrupted,
    /// or `bound` elapses.
    ///
    /// # Errors
    ///
    /// Returns the failure of watching the job's state or of reading it.
    pub(in crate::resident) fn observe(
        &self,
        control: &Control,
        bound: Duration,
    ) -> io::Result<Observed> {
        let mut keeper = self.keeper;
        let deadline = Monotonic::now() + bound;
        let mut observed_exit = false;
        loop {
            watch(&self.directory, &control.sender, Signal::woke)?;
            if let Some(state) = self.state()? {
                if let Some(ended) = state.ended() {
                    return Ok(Observed::Ended(ended));
                }
                keeper = keeper.or(Some(state.keeper));
            }
            if let (Some(pid), false) = (keeper, observed_exit) {
                observed_exit = true;
                if pid != std::process::id() {
                    observer::wait_for_exit(pid, control.sender.clone());
                }
            }
            let remaining = deadline.saturating_duration_since(Monotonic::now());
            match control.receiver.recv_timeout(remaining) {
                Ok(Signal::Changed) => {}
                Ok(Signal::KeeperExited) => {
                    if let Some(ended) = self.state()?.and_then(|state| state.ended()) {
                        return Ok(Observed::Ended(ended));
                    }
                    return Ok(Observed::Ended(Ended {
                        ending: Ending::Lost,
                        wall: None,
                    }));
                }
                Ok(Signal::ObserverFailed(reason)) => return Err(io::Error::other(reason)),
                Ok(Signal::Cancel) => return Ok(Observed::Cancelled),
                Ok(Signal::Interrupt) => return Ok(Observed::Interrupted),
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(Observed::Elapsed),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        ErrorKind::BrokenPipe,
                        "job observer disconnected",
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a test waits for a job to answer before it fails.
    const TEST_BOUND: Duration = Duration::from_secs(2);

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-job-{name}-{}-{}",
            std::process::id(),
            now_millis().expect("fixture time")
        ));
        fs::create_dir_all(&root).expect("scratch directory");
        root
    }

    fn argv(command: &str) -> Vec<OsString> {
        ["/bin/sh", "-c", command].map(OsString::from).to_vec()
    }

    #[test]
    fn a_job_runs_to_its_end_and_publishes_how_it_ended() {
        let root = scratch("ends");
        let jobs = Jobs::new(root.join("jobs"));
        let job = jobs
            .start(
                "say",
                argv("printf 'out\\n'; printf 'err\\n' >&2; exit 3"),
                &root,
                &Environment::empty(),
                Some(Duration::from_secs(5)),
                &["noted".to_owned()],
            )
            .expect("the job starts");
        let control = Control::new();
        let Observed::Ended(ended) = job
            .observe(&control, TEST_BOUND)
            .expect("the job is observed")
        else {
            panic!("the job ends");
        };
        assert_eq!(ended.ending, Ending::Exited { code: 3 });
        assert!(ended.wall.is_some());
        assert_eq!(
            job.tail().expect("the log reads"),
            Tail {
                text: "out\nerr\n".to_owned(),
                truncated: false
            }
        );
        let listed = jobs.list().expect("the lane's jobs list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id(), job.id());
        assert_eq!(listed[0].command(), "say");
        let reopened = jobs.open(job.id()).expect("the job reopens");
        assert_eq!(reopened.command(), "say");
        assert_eq!(reopened.timeout(), Some(Duration::from_secs(5)));
        assert_eq!(reopened.notices(), ["noted".to_owned()]);
        assert_eq!(reopened.ended(), Some(ended));
        assert!(jobs.open("../jobs").is_err());
        assert!(jobs.open("0badcafe").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_stopped_job_is_ended_and_an_interrupted_observation_yields() {
        let root = scratch("stops");
        let jobs = Jobs::new(root.join("jobs"));
        // A fifo nobody writes holds its reader open, so the command ends
        // only because the keeper ends it.
        let status = ProcessCommand::new("mkfifo")
            .arg(root.join("hold"))
            .status()
            .expect("mkfifo");
        assert!(status.success());
        let job = jobs
            .start(
                "hold",
                argv("cat hold"),
                &root,
                &Environment::empty(),
                None,
                &[],
            )
            .expect("the job starts");
        let control = Control::new();
        control.handle().interrupt();
        assert_eq!(
            job.observe(&control, TEST_BOUND)
                .expect("the job is observed"),
            Observed::Interrupted
        );
        assert_eq!(
            job.observe(&control, Duration::ZERO)
                .expect("the job is observed"),
            Observed::Elapsed
        );
        job.stop().expect("the stop is asked");
        let Observed::Ended(ended) = job
            .observe(&control, TEST_BOUND)
            .expect("the job is observed")
        else {
            panic!("the job ends");
        };
        assert_eq!(ended.ending, Ending::Stopped);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_job_past_its_deadline_is_ended_as_timed_out() {
        let root = scratch("deadline");
        let jobs = Jobs::new(root.join("jobs"));
        let status = ProcessCommand::new("mkfifo")
            .arg(root.join("hold"))
            .status()
            .expect("mkfifo");
        assert!(status.success());
        let job = jobs
            .start(
                "hold",
                argv("cat hold"),
                &root,
                &Environment::empty(),
                Some(Duration::from_millis(20)),
                &[],
            )
            .expect("the job starts");
        let Observed::Ended(ended) = job
            .observe(&Control::new(), TEST_BOUND)
            .expect("the job is observed")
        else {
            panic!("the job ends");
        };
        assert_eq!(ended.ending, Ending::TimedOut);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_command_that_cannot_run_ends_as_failed() {
        let root = scratch("fails");
        let jobs = Jobs::new(root.join("jobs"));
        let job = jobs
            .start(
                "nothing",
                vec![OsString::from("/nonexistent/spice-job-shell")],
                &root,
                &Environment::empty(),
                Some(Duration::from_secs(5)),
                &[],
            )
            .expect("the job starts");
        let observed = job
            .observe(&Control::new(), TEST_BOUND)
            .expect("the job is observed");
        assert!(
            matches!(
                observed,
                Observed::Ended(Ended {
                    ending: Ending::Failed { .. },
                    ..
                })
            ),
            "{observed:?}"
        );
        let _ = fs::remove_dir_all(root);
    }
}
