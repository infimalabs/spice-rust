use super::capture::{CaptureWorker, FailureContext, start_capture};
use super::guardian::PendingGuardian;
use super::{ProcessCompletion, ProcessState, Running, start_exit_watcher};
use crate::observe::{Observation, OutputStream};
use crate::spec::{ProcessGroup, Spec, StdinPolicy, StreamPolicy};
use std::fs::File;
use std::io;
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;

pub(super) fn spawn_inner(
    spec: Spec,
    observation: Option<Observation>,
    guarded: Option<&File>,
    publish: &mut dyn FnMut(u32, u32) -> io::Result<()>,
) -> io::Result<Running> {
    spawn_inner_with_guardian(spec, observation, guarded, publish, PendingGuardian::start)
}

#[cfg(test)]
pub(super) fn spawn_inner_refusing_descriptor_setup(
    spec: Spec,
    guarded: &File,
) -> io::Result<Running> {
    spawn_inner_with_guardian(
        spec,
        None,
        Some(guarded),
        &mut |_, _| Ok(()),
        PendingGuardian::start_refusing_descriptor_setup,
    )
}

fn spawn_inner_with_guardian(
    spec: Spec,
    observation: Option<Observation>,
    guarded: Option<&File>,
    publish: &mut dyn FnMut(u32, u32) -> io::Result<()>,
    start_guardian: fn(&File) -> io::Result<PendingGuardian>,
) -> io::Result<Running> {
    validate_observation(&spec, observation.as_ref())?;
    if guarded.is_some() && spec.group != ProcessGroup::New {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "guarded processes require a new process group",
        ));
    }
    let wakes = SpawnWakes::new(&spec, observation.as_ref())?;
    let guardian = guarded.map(start_guardian).transpose()?;
    let mut child = command(&spec, guardian.as_ref()).spawn()?;
    let started = Instant::now();
    let pid = child.id();
    // The topology is published here rather than on the way out, because
    // everything below this line -- three thread spawns among it -- runs while
    // the child is already alive. A wrapper that dies before publishing hands
    // possession to a guardian nothing on disk names, and no later reader can
    // tell that waiting for it would be bounded, so this runs first and the
    // group is made to exist before the record claims it. The child is already
    // observable by now and cannot be held back: `spawn` does not return until
    // the child has `exec`ed, so no wrapper-side ordering can close that gap.
    if let Some(guardian) = guardian.as_ref()
        && let Err(error) = adopt_group(pid).and_then(|()| publish(pid, guardian.id()))
    {
        return spawn_cleanup(child, spec.group, pid, error);
    }
    let pipes = match capture_pipes(&mut child, &spec) {
        Ok(pipes) => pipes,
        Err(error) => return spawn_cleanup(child, spec.group, pid, error),
    };
    let process = Arc::new(ProcessState {
        completion: Mutex::new(ProcessCompletion::default()),
        exited: Condvar::new(),
    });
    start_exit_watcher(pid, wakes.exit, Arc::clone(&process));
    let failure = Arc::new(FailureContext {
        pid,
        group: spec.group,
        owner: spec.wait.owner().to_owned(),
        grace: spec.grace,
        reap: spec.deadlines.reap(),
        process: Arc::clone(&process),
        wake: wakes.failure,
        failed: AtomicBool::new(false),
    });
    let failure_observer = observation.as_ref().map(Observation::observer);
    let mut captures = CaptureSetup {
        observation,
        failure_observer,
        observer_wake: wakes.observer,
        selected_stream: wakes.selected_stream,
        sink_deadline: spec.deadlines.output_drain(),
        failure,
    };
    let stdout = pipes.stdout.map(|(reader, limit)| {
        start_capture(
            reader,
            limit,
            captures.worker(OutputStream::Stdout, wakes.stdout),
        )
    });
    let stderr = pipes.stderr.map(|(reader, limit)| {
        start_capture(
            reader,
            limit,
            captures.worker(OutputStream::Stderr, wakes.stderr),
        )
    });
    let guardian = guardian.map(PendingGuardian::activate);
    Ok(Running {
        pid,
        spec,
        started,
        wake_receiver: wakes.receiver,
        cancellation_wake: wakes.cancellation,
        child: Some(child),
        process,
        stdout,
        stderr,
        guardian,
    })
}

/// Create the child's process group from this side too, before naming it.
///
/// The published record names a group, and a reader that probes a group its
/// child has not created yet is told it is absent -- which reads as terminal,
/// the one answer that licenses another acquisition. Both sides setting the
/// same group is how that gap is closed rather than narrowed: whichever runs
/// first creates it, and the wrapper returning from here means it exists.
/// The two refusals are that same success seen from behind, and both report a
/// child that is already past the point of needing this: `EACCES` is refused
/// only once the child has `exec`ed, which it cannot reach without its own
/// group, and `ESRCH` only once it has exited, after which no group of its
/// making can still appear.
fn adopt_group(pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: `pid` is this wrapper's child, joining the group it is named by.
    if unsafe { libc::setpgid(pid, pid) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::EACCES | libc::ESRCH)) {
        return Ok(());
    }
    Err(error)
}

fn validate_observation(spec: &Spec, observation: Option<&Observation>) -> io::Result<()> {
    let Some(observation) = observation else {
        return Ok(());
    };
    let selected = match observation.stream() {
        OutputStream::Stdout => spec.output.stdout(),
        OutputStream::Stderr => spec.output.stderr(),
    };
    if matches!(selected, StreamPolicy::Capture { .. }) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "selected {} must use the capture stream policy",
            observation.stream()
        ),
    ))
}

struct SpawnWakes {
    receiver: UnixDatagram,
    cancellation: UnixDatagram,
    exit: UnixDatagram,
    failure: UnixDatagram,
    stdout: Option<UnixDatagram>,
    stderr: Option<UnixDatagram>,
    observer: Option<UnixDatagram>,
    selected_stream: Option<OutputStream>,
}

impl SpawnWakes {
    fn new(spec: &Spec, observation: Option<&Observation>) -> io::Result<Self> {
        let (waiter, receiver) = UnixDatagram::pair()?;
        let cancellation = waiter.try_clone()?;
        let exit = waiter.try_clone()?;
        let failure = waiter.try_clone()?;
        let stdout = matches!(spec.output.stdout(), StreamPolicy::Capture { .. })
            .then(|| waiter.try_clone())
            .transpose()?;
        let stderr = matches!(spec.output.stderr(), StreamPolicy::Capture { .. })
            .then(|| waiter.try_clone())
            .transpose()?;
        let selected_stream = observation.map(Observation::stream);
        let observer = selected_stream.map(|_| waiter.try_clone()).transpose()?;
        receiver.set_nonblocking(true)?;
        Ok(Self {
            receiver,
            cancellation,
            exit,
            failure,
            stdout,
            stderr,
            observer,
            selected_stream,
        })
    }
}

struct CapturePipes {
    stdout: Option<(ChildStdout, usize)>,
    stderr: Option<(ChildStderr, usize)>,
}

fn capture_pipes(child: &mut Child, spec: &Spec) -> io::Result<CapturePipes> {
    Ok(CapturePipes {
        stdout: take_stdout(child, spec.output.stdout())?,
        stderr: take_stderr(child, spec.output.stderr())?,
    })
}

struct CaptureSetup {
    observation: Option<Observation>,
    failure_observer: Option<Arc<dyn crate::observe::RecordObserver>>,
    observer_wake: Option<UnixDatagram>,
    selected_stream: Option<OutputStream>,
    sink_deadline: std::time::Duration,
    failure: Arc<FailureContext>,
}

impl CaptureSetup {
    fn worker(&mut self, stream: OutputStream, wake: Option<UnixDatagram>) -> CaptureWorker {
        let selected =
            (self.observation.as_ref().map(Observation::stream) == Some(stream)).then(|| {
                self.observation
                    .take()
                    .expect("selected observation remains present")
            });
        CaptureWorker {
            stream,
            observation: selected,
            failure_observer: self.failure_observer.as_ref().map(Arc::clone),
            sink_deadline: self.sink_deadline,
            failure: Arc::clone(&self.failure),
            wake: wake.expect("captured stream has a preallocated wake"),
            observer_wake: (self.selected_stream == Some(stream)).then(|| {
                self.observer_wake
                    .take()
                    .expect("selected stream has an observer wake")
            }),
        }
    }
}

fn command(spec: &Spec, guardian: Option<&PendingGuardian>) -> Command {
    let mut command = Command::new(spec.argv.program());
    command
        .args(spec.argv.arguments())
        .current_dir(spec.cwd())
        .env_clear()
        .envs(spec.environment.entries().iter().cloned());
    match spec.output.stdin() {
        StdinPolicy::Inherit => command.stdin(Stdio::inherit()),
        StdinPolicy::Null => command.stdin(Stdio::null()),
    };
    configure_stream(&mut command, true, spec.output.stdout());
    configure_stream(&mut command, false, spec.output.stderr());
    if spec.group == ProcessGroup::New {
        command.process_group(0);
    }
    if let Some(guardian) = guardian {
        let notifier = guardian.notifier_fd();
        // SAFETY: this child-only hook performs only async-signal-safe system calls.
        unsafe {
            command.pre_exec(move || super::guardian::notify_group(notifier));
        }
    }
    command
}

fn configure_stream(command: &mut Command, stdout: bool, policy: StreamPolicy) {
    let stdio = match policy {
        StreamPolicy::Inherit => Stdio::inherit(),
        StreamPolicy::Null => Stdio::null(),
        StreamPolicy::Capture { .. } => Stdio::piped(),
    };
    if stdout {
        command.stdout(stdio);
    } else {
        command.stderr(stdio);
    }
}

fn take_stdout(
    child: &mut Child,
    policy: StreamPolicy,
) -> io::Result<Option<(ChildStdout, usize)>> {
    match policy {
        StreamPolicy::Capture { limit } => {
            let reader = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("captured stdout pipe is absent"))?;
            Ok(Some((reader, limit)))
        }
        StreamPolicy::Inherit | StreamPolicy::Null => Ok(None),
    }
}

fn take_stderr(
    child: &mut Child,
    policy: StreamPolicy,
) -> io::Result<Option<(ChildStderr, usize)>> {
    match policy {
        StreamPolicy::Capture { limit } => {
            let reader = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("captured stderr pipe is absent"))?;
            Ok(Some((reader, limit)))
        }
        StreamPolicy::Inherit | StreamPolicy::Null => Ok(None),
    }
}

fn spawn_cleanup(
    mut child: Child,
    group: ProcessGroup,
    pid: u32,
    error: io::Error,
) -> io::Result<Running> {
    let _signal_result = super::signal_pid(pid, group, super::platform::SIGKILL, "failed spawn");
    let _reap_result = child.wait();
    Err(error)
}
