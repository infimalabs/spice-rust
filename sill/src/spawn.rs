#[cfg(unix)]
use crate::observe::RecordObserver;
use crate::observe::{Observation, ObservationFailure};
#[cfg(unix)]
use crate::spec::ProcessGroup;
use crate::spec::Spec;
use std::error::Error;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::process::Child;
use std::process::ExitStatus;
#[cfg(unix)]
use std::sync::atomic::Ordering;
#[cfg(unix)]
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
const WAKE_BYTE: &[u8] = &[1];

#[cfg(unix)]
use capture::{CaptureHandle, FailureContext};
#[cfg(unix)]
use launch::spawn_inner;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
    complete: bool,
}

impl CapturedOutput {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn complete(&self) -> bool {
        self.complete
    }
}

#[derive(Debug)]
pub enum Completion {
    Exited(ExitStatus),
    TimedOut(ExitStatus),
    Cancelled(ExitStatus),
}

impl Completion {
    pub fn status(&self) -> ExitStatus {
        match self {
            Self::Exited(status) | Self::TimedOut(status) | Self::Cancelled(status) => *status,
        }
    }
}

#[derive(Debug)]
pub struct WaitOutcome {
    completion: Completion,
    observation_failure: Option<ObservationFailure>,
    stdout: Option<CapturedOutput>,
    stderr: Option<CapturedOutput>,
}

/// One bounded passive observation of a running child.
#[derive(Debug)]
pub enum WaitStep {
    /// The process exited within the observation bound and was reaped.
    Exited(WaitOutcome),
    /// The bound elapsed while the process remained live.
    Running(Box<Running>),
}

impl WaitOutcome {
    pub fn completion(&self) -> &Completion {
        &self.completion
    }

    pub fn stdout(&self) -> Option<&CapturedOutput> {
        self.stdout.as_ref()
    }

    pub fn stderr(&self) -> Option<&CapturedOutput> {
        self.stderr.as_ref()
    }

    /// Selected-stream failure that forced process cleanup.
    #[must_use]
    pub const fn observation_failure(&self) -> Option<&ObservationFailure> {
        self.observation_failure.as_ref()
    }
}

#[derive(Debug)]
pub enum WaitError {
    Io {
        owner: String,
        operation: &'static str,
        source: io::Error,
    },
    ReapDeadlineExceeded {
        owner: String,
        deadline: Duration,
    },
}

impl fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                owner,
                operation,
                source,
            } => write!(
                formatter,
                "wait owned by {owner:?} failed while {operation}: {source}"
            ),
            Self::ReapDeadlineExceeded { owner, deadline } => write!(
                formatter,
                "wait owned by {owner:?} exceeded its {deadline:?} reap deadline"
            ),
        }
    }
}

impl Error for WaitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::ReapDeadlineExceeded { .. } => None,
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
pub struct Running {
    pid: u32,
    spec: Spec,
    started: Instant,
    wake_receiver: std::os::unix::net::UnixDatagram,
    cancellation_wake: std::os::unix::net::UnixDatagram,
    child: Option<Child>,
    process: Arc<ProcessState>,
    stdout: Option<CaptureHandle>,
    stderr: Option<CaptureHandle>,
    guardian: Option<guardian::Guardian>,
}

#[cfg(unix)]
#[derive(Debug)]
struct ProcessState {
    completion: Mutex<ProcessCompletion>,
    exited: Condvar,
}

#[cfg(unix)]
#[derive(Debug, Default)]
struct ProcessCompletion {
    notice: Option<Result<(), IoFailure>>,
    status: Option<ExitStatus>,
    taken: bool,
    observation_failure: Option<ObservationFailure>,
    cleanup_failure: Option<IoFailure>,
}

#[cfg(unix)]
#[derive(Clone, Debug)]
struct IoFailure {
    kind: io::ErrorKind,
    detail: String,
}

#[cfg(unix)]
impl IoFailure {
    fn capture(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            detail: error.to_string(),
        }
    }

    fn error(&self) -> io::Error {
        io::Error::new(self.kind, self.detail.clone())
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
pub struct Running {
    spec: Spec,
}

#[cfg(unix)]
pub fn spawn(spec: Spec) -> io::Result<Running> {
    spawn_inner(spec, None, None)
}

#[cfg(unix)]
pub(crate) fn spawn_guarded(spec: Spec, file: &File) -> io::Result<Running> {
    spawn_inner(spec, None, Some(file))
}

/// Spawn one child with a bounded selected-stream observer.
///
/// The selected stream and every other captured stream are drained from child
/// creation onward, independently of calls to [`Running::wait`] or shutdown.
///
/// # Errors
///
/// Returns an I/O error when the selected stream is not configured for capture
/// or when the child and its capture workers cannot be created.
#[cfg(unix)]
pub fn spawn_observed(spec: Spec, observation: Observation) -> io::Result<Running> {
    spawn_inner(spec, Some(observation), None)
}

#[cfg(not(unix))]
pub fn spawn_observed(_spec: Spec, _observation: Observation) -> io::Result<Running> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process spawning requires a native release target",
    ))
}

#[cfg(not(unix))]
pub fn spawn(_spec: Spec) -> io::Result<Running> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process spawning requires a native release target",
    ))
}

#[cfg(unix)]
fn start_exit_watcher(
    pid: u32,
    waiter_wake: std::os::unix::net::UnixDatagram,
    process: Arc<ProcessState>,
) {
    std::thread::spawn(move || {
        let result = wait_for_exit(pid).map_err(IoFailure::capture);
        if let Ok(mut completion) = process.completion.lock() {
            completion.notice = Some(result);
            process.exited.notify_all();
        }
        let _wake_result = waiter_wake.send(WAKE_BYTE);
    });
}

#[cfg(unix)]
fn wait_for_exit(pid: u32) -> io::Result<()> {
    loop {
        let mut information = std::mem::MaybeUninit::<platform::SignalInfo>::uninit();
        let result = unsafe {
            platform::waitid(
                platform::P_PID,
                pid,
                information.as_mut_ptr(),
                platform::WEXITED | platform::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(());
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
impl Running {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub(crate) fn guardian_id(&self) -> Option<u32> {
        self.guardian.as_ref().map(guardian::Guardian::id)
    }

    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    pub fn runtime_remaining(&self) -> Duration {
        self.spec
            .deadlines
            .runtime()
            .saturating_sub(self.started.elapsed())
    }

    pub fn wait_for(mut self, duration: Duration) -> Result<WaitStep, WaitError> {
        let started = Instant::now();
        loop {
            self.drain_outputs()?;
            if let Some(status) = self.take_child_exit()? {
                let completion = self.completion(status)?;
                return self.finish(completion).map(WaitStep::Exited);
            }
            let elapsed = started.elapsed();
            if elapsed >= duration {
                return Ok(WaitStep::Running(Box::new(self)));
            }
            self.block_for(duration.saturating_sub(elapsed))?;
        }
    }

    pub fn signal_terminate(&self) -> Result<(), WaitError> {
        self.signal(platform::SIGTERM)
    }

    pub fn signal_kill(&self) -> Result<(), WaitError> {
        self.signal(platform::SIGKILL)
    }

    pub fn wait(mut self) -> Result<WaitOutcome, WaitError> {
        let cancellation_wake = self
            .cancellation_wake
            .try_clone()
            .map_err(|source| self.io_error("registering cancellation wake", source))?;
        let _cancellation_registration =
            self.spec
                .wait
                .cancellation()
                .register_waker(Arc::new(move || {
                    let _wake_result = cancellation_wake.send(WAKE_BYTE);
                }));

        loop {
            self.drain_outputs()?;

            if let Some(status) = self.take_child_exit()? {
                let completion = self.completion(status)?;
                return self.finish(completion);
            }

            if self.spec.wait.cancellation().is_cancelled() {
                let status = self.terminate_and_reap()?;
                return self.finish(Completion::Cancelled(status));
            }

            let elapsed = self.started.elapsed();
            if elapsed >= self.spec.deadlines.runtime() {
                let status = self.terminate_and_reap()?;
                return self.finish(Completion::TimedOut(status));
            }

            self.block_for(self.spec.deadlines.runtime().saturating_sub(elapsed))?;
        }
    }

    fn terminate_and_reap(&mut self) -> Result<ExitStatus, WaitError> {
        if let Some(status) = self.take_child_exit()? {
            return Ok(status);
        }

        self.signal(platform::SIGTERM)?;
        if let Some(status) = self.await_child_for(self.spec.grace)? {
            return Ok(status);
        }

        self.signal(platform::SIGKILL)?;
        if let Some(status) = self.await_child_for(self.spec.deadlines.reap())? {
            return Ok(status);
        }

        Err(WaitError::ReapDeadlineExceeded {
            owner: self.spec.wait.owner().to_owned(),
            deadline: self.spec.deadlines.reap(),
        })
    }

    fn await_child_for(&mut self, duration: Duration) -> Result<Option<ExitStatus>, WaitError> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.take_child_exit()? {
                return Ok(Some(status));
            }
            let elapsed = started.elapsed();
            if elapsed >= duration {
                return Ok(None);
            }
            self.block_for(duration.saturating_sub(elapsed))?;
        }
    }

    fn finish(mut self, completion: Completion) -> Result<WaitOutcome, WaitError> {
        let started = Instant::now();
        loop {
            self.drain_outputs()?;
            if self.outputs_complete() || started.elapsed() >= self.spec.deadlines.output_drain() {
                break;
            }
            self.block_for(
                self.spec
                    .deadlines
                    .output_drain()
                    .saturating_sub(started.elapsed()),
            )?;
        }

        let outcome = WaitOutcome {
            completion,
            observation_failure: self.observation_failure()?,
            stdout: self
                .stdout
                .take()
                .map(CaptureHandle::finish)
                .transpose()
                .map_err(|source| self.io_error("finishing stdout capture", source))?,
            stderr: self
                .stderr
                .take()
                .map(CaptureHandle::finish)
                .transpose()
                .map_err(|source| self.io_error("finishing stderr capture", source))?,
        };
        if guardian::group_absent(self.pid)
            && let Some(guardian) = self.guardian.take()
        {
            guardian
                .wait()
                .map_err(|source| self.io_error("reaping lock guardian", source))?;
        }
        Ok(outcome)
    }

    fn take_child_exit(&mut self) -> Result<Option<ExitStatus>, WaitError> {
        let notice = {
            let mut completion = self.process.completion.lock().map_err(|_| {
                self.io_error(
                    "reading child completion",
                    io::Error::other("child completion lock is poisoned"),
                )
            })?;
            if let Some(failure) = &completion.cleanup_failure {
                return Err(self.io_error("cleaning up failed observation", failure.error()));
            }
            if let Some(status) = completion.status
                && !completion.taken
            {
                completion.taken = true;
                self.child.take();
                return Ok(Some(status));
            }
            completion.notice.clone()
        };

        match notice {
            Some(Ok(())) => {
                reap_process(self.pid, &self.process)
                    .map_err(|source| self.io_error("reaping notified child", source))?;
                self.take_child_exit()
            }
            Some(Err(source)) => {
                Err(self.io_error("watching for child completion", source.error()))
            }
            None => Ok(None),
        }
    }

    fn completion(&self, status: ExitStatus) -> Result<Completion, WaitError> {
        Ok(Completion::Exited(status))
    }

    fn observation_failure(&self) -> Result<Option<ObservationFailure>, WaitError> {
        self.process
            .completion
            .lock()
            .map(|completion| completion.observation_failure.clone())
            .map_err(|_| {
                self.io_error(
                    "reading observation completion",
                    io::Error::other("child completion lock is poisoned"),
                )
            })
    }

    fn block_for(&mut self, duration: Duration) -> Result<(), WaitError> {
        use std::os::fd::AsRawFd;

        let mut descriptors = Vec::with_capacity(1);
        descriptors.push(platform::PollFd::read(self.wake_receiver.as_raw_fd()));

        let result = unsafe {
            platform::poll(
                descriptors.as_mut_ptr(),
                platform::Nfds::try_from(descriptors.len()).expect("one poll descriptor fits"),
                poll_timeout(duration),
            )
        };
        if result == -1 {
            let source = io::Error::last_os_error();
            if source.kind() != io::ErrorKind::Interrupted {
                return Err(self.io_error("blocking on process events", source));
            }
        }

        self.drain_wake()?;
        self.drain_outputs()
    }

    fn drain_wake(&self) -> Result<(), WaitError> {
        let mut buffer = [0_u8; 32];
        loop {
            match self.wake_receiver.recv(&mut buffer) {
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(source) if source.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => return Err(self.io_error("draining process wake events", source)),
            }
        }
    }

    fn drain_outputs(&mut self) -> Result<(), WaitError> {
        if let Some(stdout) = &self.stdout {
            stdout.failure().map_err(|source| WaitError::Io {
                owner: self.spec.wait.owner().to_owned(),
                operation: "draining stdout",
                source,
            })?;
        }
        if let Some(stderr) = &self.stderr {
            stderr.failure().map_err(|source| WaitError::Io {
                owner: self.spec.wait.owner().to_owned(),
                operation: "draining stderr",
                source,
            })?;
        }
        Ok(())
    }

    fn outputs_complete(&self) -> bool {
        self.stdout.as_ref().is_none_or(CaptureHandle::is_complete)
            && self.stderr.as_ref().is_none_or(CaptureHandle::is_complete)
    }

    fn signal(&self, signal: i32) -> Result<(), WaitError> {
        signal_pid(self.pid, self.spec.group, signal, self.spec.wait.owner())
    }

    fn io_error(&self, operation: &'static str, source: io::Error) -> WaitError {
        WaitError::Io {
            owner: self.spec.wait.owner().to_owned(),
            operation,
            source,
        }
    }
}

#[cfg(not(unix))]
impl Running {
    pub fn id(&self) -> u32 {
        unreachable!("unsupported targets cannot construct Running")
    }

    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    pub fn runtime_remaining(&self) -> Duration {
        self.spec.deadlines.runtime()
    }

    pub fn wait_for(self, _duration: Duration) -> Result<WaitStep, WaitError> {
        Err(WaitError::Io {
            owner: self.spec.wait.owner().to_owned(),
            operation: "waiting for a process on an unsupported target",
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "process waiting requires a native release target",
            ),
        })
    }

    pub fn signal_terminate(&self) -> Result<(), WaitError> {
        Err(WaitError::Io {
            owner: self.spec.wait.owner().to_owned(),
            operation: "signalling a process on an unsupported target",
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "process signalling requires a native release target",
            ),
        })
    }

    pub fn signal_kill(&self) -> Result<(), WaitError> {
        self.signal_terminate()
    }

    pub fn wait(self) -> Result<WaitOutcome, WaitError> {
        Err(WaitError::Io {
            owner: self.spec.wait.owner().to_owned(),
            operation: "waiting for a process on an unsupported target",
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "process waiting requires a native release target",
            ),
        })
    }
}

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        let reaped = self
            .process
            .completion
            .lock()
            .is_ok_and(|completion| completion.status.is_some());
        if !reaped {
            let _signal_result = self.signal(platform::SIGKILL);
            let _reap_result = self.await_child_for(self.spec.deadlines.reap());
        }
    }
}

#[cfg(unix)]
fn fail_observation(
    context: &FailureContext,
    observer: &Arc<dyn RecordObserver>,
    failure: ObservationFailure,
) {
    if context.failed.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Ok(mut completion) = context.process.completion.lock() {
        completion.observation_failure = Some(failure.clone());
    }

    if let Err(error) = abort_process(context)
        && let Ok(mut completion) = context.process.completion.lock()
    {
        completion.cleanup_failure = Some(error);
    }
    let _wake_result = context.wake.send(WAKE_BYTE);
    let _reported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        observer.failed(&failure);
    }));
}

#[cfg(unix)]
fn abort_process(context: &FailureContext) -> Result<(), IoFailure> {
    signal_pid(
        context.pid,
        context.group,
        platform::SIGTERM,
        &context.owner,
    )
    .map_err(|error| IoFailure {
        kind: io::ErrorKind::Other,
        detail: error.to_string(),
    })?;
    if wait_for_exit_notice(&context.process, context.grace)? {
        reap_process(context.pid, &context.process).map_err(IoFailure::capture)?;
        return Ok(());
    }

    signal_pid(
        context.pid,
        context.group,
        platform::SIGKILL,
        &context.owner,
    )
    .map_err(|error| IoFailure {
        kind: io::ErrorKind::Other,
        detail: error.to_string(),
    })?;
    if wait_for_exit_notice(&context.process, context.reap)? {
        reap_process(context.pid, &context.process).map_err(IoFailure::capture)?;
        return Ok(());
    }

    Err(IoFailure {
        kind: io::ErrorKind::TimedOut,
        detail: format!(
            "observation cleanup exceeded its {:?} reap deadline",
            context.reap
        ),
    })
}

#[cfg(unix)]
fn wait_for_exit_notice(process: &ProcessState, duration: Duration) -> Result<bool, IoFailure> {
    let completion = process.completion.lock().map_err(|_| IoFailure {
        kind: io::ErrorKind::Other,
        detail: "child completion lock is poisoned".to_owned(),
    })?;
    let (completion, _timeout) = process
        .exited
        .wait_timeout_while(completion, duration, |completion| {
            completion.notice.is_none() && completion.status.is_none()
        })
        .map_err(|_| IoFailure {
            kind: io::ErrorKind::Other,
            detail: "child completion lock is poisoned".to_owned(),
        })?;
    match &completion.notice {
        Some(Err(failure)) => Err(failure.clone()),
        Some(Ok(())) => Ok(true),
        None => Ok(completion.status.is_some()),
    }
}

#[cfg(unix)]
fn reap_process(pid: u32, process: &ProcessState) -> io::Result<ExitStatus> {
    use std::os::unix::process::ExitStatusExt;

    let mut completion = process
        .completion
        .lock()
        .map_err(|_| io::Error::other("child completion lock is poisoned"))?;
    if let Some(status) = completion.status {
        return Ok(status);
    }
    if let Some(Err(failure)) = &completion.notice {
        return Err(failure.error());
    }
    let pid =
        i32::try_from(pid).map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
    let mut raw = 0;
    loop {
        let result = unsafe { libc::waitpid(pid, &mut raw, 0) };
        if result == pid {
            let status = ExitStatus::from_raw(raw);
            completion.status = Some(status);
            process.exited.notify_all();
            return Ok(status);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn signal_pid(pid: u32, group: ProcessGroup, signal: i32, owner: &str) -> Result<(), WaitError> {
    let pid = i32::try_from(pid).map_err(|source| WaitError::Io {
        owner: owner.to_owned(),
        operation: "converting child pid",
        source: io::Error::new(io::ErrorKind::InvalidData, source),
    })?;
    let target = match group {
        ProcessGroup::Inherit => pid,
        ProcessGroup::New => -pid,
    };
    let result = unsafe { platform::kill(target, signal) };
    if result == 0 {
        return Ok(());
    }

    let source = io::Error::last_os_error();
    if source.raw_os_error() == Some(platform::ESRCH) {
        Ok(())
    } else {
        Err(WaitError::Io {
            owner: owner.to_owned(),
            operation: "signalling child process group",
            source,
        })
    }
}

#[cfg(unix)]
fn poll_timeout(duration: Duration) -> i32 {
    if duration.is_zero() {
        return 0;
    }

    let whole_milliseconds = duration.as_millis();
    let rounded_milliseconds =
        whole_milliseconds + u128::from(!duration.subsec_nanos().is_multiple_of(1_000_000));
    i32::try_from(rounded_milliseconds).unwrap_or(i32::MAX)
}

#[cfg(unix)]
mod capture;
#[cfg(unix)]
mod guardian;
#[cfg(unix)]
mod launch;
#[cfg(unix)]
mod platform;

#[cfg(all(test, unix))]
mod tests;
