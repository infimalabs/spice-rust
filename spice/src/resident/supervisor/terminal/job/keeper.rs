//! Detached keeper runtime and observation control.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant as Monotonic};

use wire::CanonicalUuid;

use super::super::super::super::daemon::observer;
use super::{
    DELIVERED, DELIVERY_GRACE, Ended, Ending, LOG, OPERATOR_STOP, REQUEST_BYTES, Request, STATE,
    STOP, State, TERMINATE_GRACE, VERSION, WAKE, events, now_millis, publish_json, read_json,
    signal_group,
};
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
        Ok((child, pump)) => {
            let stop = directory.join(STOP);
            let ending = tend(
                child,
                &receiver,
                request.timeout_millis.map(Duration::from_millis),
                || watch(directory, &sender, Keep::woke).map(|()| stop.exists()),
            );
            if let Some(pump) = pump {
                pump.settle(TERMINATE_GRACE);
            }
            ending
        }
        Err(reason) => Ending::Failed { reason },
    });
    state.ended_millis = Some(now_millis()?);
    publish_json(&state_path, &state)?;
    if let Some(lane) = request.lane
        && (state.ending != Some(Ending::Stopped) || directory.join(OPERATOR_STOP).exists())
        && unheard(directory, DELIVERY_GRACE, |directory| {
            directory.join(DELIVERED).exists()
        })
    {
        wake(lane, directory);
    }
    Ok(())
}

/// Whether nobody has heard by the end of the grace: the record an occupant
/// leaves on hearing is awaited on the directory's watch, so an occupant that
/// is there answers at once and a vacant lane costs the grace.
fn unheard(directory: &Path, grace: Duration, heard: impl Fn(&Path) -> bool) -> bool {
    let deadline = Monotonic::now() + grace;
    let (sender, receiver) = mpsc::channel();
    loop {
        if watch(directory, &sender, Keep::woke).is_err() || heard(directory) {
            return !heard(directory);
        }
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        if !matches!(receiver.recv_timeout(remaining), Ok(Keep::Changed)) {
            return !heard(directory);
        }
    }
}

/// The lane an event nobody hears wakes: a deadline-bounded command's. A
/// service never keeps a lane idle, so its events reach only a seated
/// occupant and wake nobody.
pub(super) fn event_lane(request: &Request) -> Option<CanonicalUuid> {
    request.timeout_millis.and(request.lane)
}

/// What the keeper does with each event it publishes: with a lane to wake,
/// the latest event nobody hears within the grace wakes it, as an unheard
/// ending does, so its next occupant hears the events it missed.
fn waker(lane: Option<CanonicalUuid>, directory: &Path) -> io::Result<Box<dyn Fn(u64) + Send>> {
    let Some(lane) = lane else {
        return Ok(Box::new(|_| {}));
    };
    let directory = directory.to_path_buf();
    let (published, receiver) = mpsc::channel::<u64>();
    thread::Builder::new()
        .name("spice-job-waker".to_owned())
        .spawn(move || {
            while let Ok(mut sequence) = receiver.recv() {
                while let Ok(later) = receiver.try_recv() {
                    sequence = later;
                }
                if unheard(&directory, DELIVERY_GRACE, |directory| {
                    events::heard_through(directory, sequence)
                }) {
                    wake(lane, &directory);
                }
            }
        })?;
    Ok(Box::new(move |sequence| {
        let _ = published.send(sequence);
    }))
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

/// Start the command in its own group with its streams appended to the log
/// and a waiter reporting its exit. With events, stdout reaches the log
/// through the pump that publishes its lines.
fn spawn(
    directory: &Path,
    request: &Request,
    sender: mpsc::Sender<Keep>,
) -> Result<(u32, Option<events::Pump>), String> {
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
    let published = if request.events {
        Some(
            waker(event_lane(request), directory)
                .map_err(|error| format!("waker unavailable: {error}"))?,
        )
    } else {
        None
    };
    let mut command = ProcessCommand::new(program);
    command
        .args(arguments)
        .current_dir(&request.cwd)
        .env_clear()
        .envs(request.environment.entries().iter().cloned())
        .stdin(Stdio::null())
        .stderr(stderr);
    if published.is_some() {
        command.stdout(Stdio::piped());
    } else {
        command.stdout(
            log.try_clone()
                .map_err(|error| format!("log unavailable: {error}"))?,
        );
    }
    detach(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let pid = child.id();
    let pump = match (child.stdout.take(), published) {
        (Some(stdout), Some(published)) => {
            match events::pump(stdout, log, directory.to_path_buf(), published) {
                Ok(pump) => Some(pump),
                Err(error) => {
                    let _ = signal_group(pid, libc::SIGKILL);
                    return Err(format!("event pump unavailable: {error}"));
                }
            }
        }
        _ => None,
    };
    thread::Builder::new()
        .name("spice-job-waiter".to_owned())
        .spawn(move || {
            let _ = sender.send(match child.wait() {
                Ok(status) => Keep::Exited(status),
                Err(error) => Keep::Failed(error.to_string()),
            });
        })
        .map_err(|error| format!("waiter unavailable: {error}"))?;
    Ok((pid, pump))
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
pub(super) fn detach(command: &mut ProcessCommand) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
pub(super) fn detach(_command: &mut ProcessCommand) {}

/// Send what `signal` makes of the next change to `path`, a directory or a
/// file; the watch is armed before the reader looks.
pub(super) fn watch<T: Send + 'static>(
    path: &Path,
    sender: &mpsc::Sender<T>,
    signal: impl FnOnce(io::Result<()>) -> T + Send + 'static,
) -> io::Result<()> {
    let observer = observer::DirectObserver::arm(path)?;
    observer::wait_for_change(observer, sender.clone(), signal);
    Ok(())
}

/// What reaches a job's observer: the job's own progress, and the lane's
/// word on the call that is observing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::resident) enum Signal {
    /// The job's directory changed.
    Changed,
    /// The job's log was written.
    Wrote,
    KeeperExited,
    ObserverFailed(String),
    /// The call this observation answers was withdrawn.
    Cancel,
    /// A message arrived for the occupant; the observation should yield.
    Interrupt,
}

impl Signal {
    pub(super) fn woke(woke: io::Result<()>) -> Self {
        woke.map_or_else(
            |error| Self::ObserverFailed(error.to_string()),
            |()| Self::Changed,
        )
    }

    pub(super) fn wrote(wrote: io::Result<()>) -> Self {
        wrote.map_or_else(
            |error| Self::ObserverFailed(error.to_string()),
            |()| Self::Wrote,
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
    pub(super) sender: mpsc::Sender<Signal>,
    pub(super) receiver: mpsc::Receiver<Signal>,
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

    #[cfg(test)]
    pub(in crate::resident) fn was_interrupted(&self) -> bool {
        matches!(self.receiver.try_recv(), Ok(Signal::Interrupt))
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
