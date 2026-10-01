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
//! The keeper reads its stop request the same way, and an ending nobody
//! hears within the grace wakes the lane, so its next occupant hears it.

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
use steer::Key;
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
const DELIVERED: &str = "delivered";
const WAKE: &str = "wake";

/// Bytes of the log a reading keeps: the end, where a failure explains
/// itself.
pub(super) const LOG_TAIL: usize = 16 * 1024;

/// How long a command has to leave on its own once the keeper ends it.
const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// How long an occupant has to hear an ending before the keeper wakes the
/// lane to deliver it.
const DELIVERY_GRACE: Duration = Duration::from_secs(2);

/// What the keeper reads: the command as the model wrote it, the argv the
/// lane runs, and where and under what to run it.
#[derive(Deserialize, Serialize)]
struct Request {
    version: u32,
    id: String,
    command: String,
    argv: Vec<OsString>,
    cwd: PathBuf,
    environment: Environment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_millis: Option<u64>,
    /// What the results carry ahead of the command's output, as omp noted it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
    /// The lane to wake when nobody hears the ending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lane: Option<CanonicalUuid>,
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
    publish_json(&state_path, &state)?;
    if let Some(lane) = request.lane
        && state.ending != Some(Ending::Stopped)
        && unheard(directory, DELIVERY_GRACE)
    {
        wake(lane, directory);
    }
    Ok(())
}

/// Whether nobody has heard the ending by the end of the grace: the record
/// an occupant leaves on delivery is awaited on the directory's watch, so an
/// occupant that is there answers at once and a vacant lane costs the grace.
fn unheard(directory: &Path, grace: Duration) -> bool {
    let delivered = directory.join(DELIVERED);
    let deadline = Monotonic::now() + grace;
    let (sender, receiver) = mpsc::channel();
    loop {
        if watch(directory, &sender, Keep::woke).is_err() || delivered.exists() {
            return !delivered.exists();
        }
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        if !matches!(receiver.recv_timeout(remaining), Ok(Keep::Changed)) {
            return !delivered.exists();
        }
    }
}

/// Wake the lane so its next occupant hears the ending. The keeper runs in
/// the occupant's environment, so the lane it names is unset and the wake
/// speaks for the operator; a lane still occupied refuses, and what the
/// wake said is kept with the job rather than in the command's log.
fn wake(lane: CanonicalUuid, directory: &Path) {
    let _woken = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(WAKE))
        .and_then(|record| {
            let stderr = record.try_clone()?;
            ProcessCommand::new(std::env::current_exe()?)
                .args(["lane", "wake"])
                .arg(lane.to_string())
                .env_remove(steer::hook::LANE)
                .stdin(Stdio::null())
                .stdout(record)
                .stderr(stderr)
                .status()
        });
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
        .envs(request.environment.entries().iter().cloned())
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

/// Where one lane's jobs live, and the lane to wake for an ending nobody
/// hears.
#[derive(Clone, Debug)]
pub(in crate::resident) struct Jobs {
    root: PathBuf,
    lane: Option<CanonicalUuid>,
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
        Self {
            root: paths.supervisors.join(lane.to_string()).join("jobs"),
            lane: Some(lane),
        }
    }

    /// Jobs kept for no lane, so an ending nobody hears wakes nobody: the
    /// keeper a test runs on its own thread spawns nothing.
    #[cfg(test)]
    pub(in crate::resident) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lane: None,
        }
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
        fs::create_dir_all(&self.root)?;
        let (id, directory) = self.claim()?;
        let request_path = directory.join(REQUEST);
        publish_json(
            &request_path,
            &Request {
                version: VERSION,
                id: id.clone(),
                command: command.to_owned(),
                argv,
                cwd: cwd.to_path_buf(),
                environment: environment.clone(),
                timeout_millis: timeout
                    .map(|timeout| u64::try_from(timeout.as_millis()))
                    .transpose()
                    .map_err(io::Error::other)?,
                notices: notices.to_vec(),
                lane: self.lane,
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

    /// A directory of this lane's for a new job, named by the microsecond
    /// key the system names everything with, so a listing is a timeline; a
    /// microsecond already taken yields the next.
    fn claim(&self) -> io::Result<(String, PathBuf)> {
        let mut micros = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_micros(),
        )
        .map_err(io::Error::other)?;
        loop {
            let id = Key::for_micros(micros)
                .map_err(io::Error::other)?
                .to_string();
            let directory = self.root.join(&id);
            match fs::create_dir(&directory) {
                Ok(()) => return Ok((id, directory)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => micros += 1,
                Err(error) => return Err(error),
            }
        }
    }

    /// One job of this lane, by the identity the model quotes: a job exists
    /// from the moment its request is recorded, before its keeper speaks.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for an identity that names no job.
    pub(in crate::resident) fn open(&self, id: &str) -> io::Result<Job> {
        let named = Key::shaped(id);
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

    /// Every job of this lane, oldest first: the keys order themselves.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the lane's job directory.
    pub(in crate::resident) fn list(&self) -> io::Result<Vec<Job>> {
        self.select(|_| true)
    }

    /// The jobs whose ending nobody has heard, oldest first, without opening
    /// the record of any job already delivered: a lane's row is read far more
    /// often than a job ends.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the lane's job directory.
    pub(in crate::resident) fn kept(&self) -> io::Result<Vec<Job>> {
        self.select(|directory| !directory.join(DELIVERED).exists())
    }

    fn select(&self, wanted: impl Fn(&Path) -> bool) -> io::Result<Vec<Job>> {
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
            if !wanted(&entry.path()) {
                continue;
            }
            match self.open(&id) {
                Ok(job) => jobs.push(job),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(jobs)
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

    /// Claim the record that the lane has heard how the job ended: in the
    /// answer to the call that ran it, in a job answer that reported its
    /// ending, or as steering once it was kept. The record is created at most
    /// once, so of two tellers racing to it exactly one wins; the loser has
    /// nothing left to say.
    ///
    /// # Errors
    ///
    /// Returns the failure of creating the record other than its existence.
    pub(in crate::resident) fn claim_heard(&self) -> io::Result<bool> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(DELIVERED))
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Give back a claim whose telling never reached the lane, so the next
    /// seat's catch-up owes it again.
    ///
    /// # Errors
    ///
    /// Returns the failure of removing the record other than its absence.
    pub(in crate::resident) fn release_heard(&self) -> io::Result<()> {
        match fs::remove_file(self.directory.join(DELIVERED)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Complete keeper-owned output, including bytes omitted from previews.
    pub(super) fn log_path(&self) -> PathBuf {
        self.directory.join(LOG)
    }

    /// The last of the log, bounded.
    ///
    /// # Errors
    ///
    /// Returns the failure of reading the log; a log not yet written is empty.
    pub(in crate::resident) fn tail(&self) -> io::Result<Tail> {
        let mut file = match File::open(self.log_path()) {
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
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        // Lossy decoding can expand invalid bytes. Keep the displayed UTF-8
        // body within the same byte budget without cutting a code point.
        let mut start = text.len().saturating_sub(LOG_TAIL);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            text.drain(..start);
        }
        Ok(Tail {
            text,
            truncated: truncated || start > 0,
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
#[path = "job/tests.rs"]
mod tests;
