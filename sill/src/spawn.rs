use crate::spec::Spec;
#[cfg(unix)]
use crate::spec::{ProcessGroup, StdinPolicy, StreamPolicy};
use std::error::Error;
use std::fmt;
use std::io;
#[cfg(unix)]
use std::io::Read;
use std::process::ExitStatus;
#[cfg(unix)]
use std::process::{Child, ChildStderr, ChildStdout, Stdio};
#[cfg(unix)]
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
const READ_BUDGET_PER_EVENT: usize = 64 * 1024;
#[cfg(unix)]
const WAKE_BYTE: &[u8] = &[1];

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
    child_exit: Arc<Mutex<Option<io::Result<()>>>>,
    child_reaped: bool,
    stdout: Option<CaptureReader<ChildStdout>>,
    stderr: Option<CaptureReader<ChildStderr>>,
}

#[cfg(not(unix))]
#[derive(Debug)]
pub struct Running {
    spec: Spec,
}

#[cfg(unix)]
pub fn spawn(spec: Spec) -> io::Result<Running> {
    use std::os::unix::net::UnixDatagram;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let (waiter_wake, wake_receiver) = UnixDatagram::pair()?;
    let cancellation_wake = waiter_wake.try_clone()?;
    wake_receiver.set_nonblocking(true)?;

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

    let mut child = command.spawn()?;
    let started = Instant::now();
    let pid = child.id();
    let (stdout, stdout_error) = capture_result(capture_stdout(&mut child, spec.output.stdout()));
    let (stderr, stderr_error) = capture_result(capture_stderr(&mut child, spec.output.stderr()));
    let child_exit = start_exit_watcher(pid, waiter_wake);
    let mut running = Running {
        pid,
        spec,
        started,
        wake_receiver,
        cancellation_wake,
        child: Some(child),
        child_exit,
        child_reaped: false,
        stdout,
        stderr,
    };

    if let Some(error) = stdout_error.or(stderr_error) {
        let _cleanup_result = running.terminate_and_reap();
        return Err(error);
    }

    Ok(running)
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
) -> Arc<Mutex<Option<io::Result<()>>>> {
    let child_exit = Arc::new(Mutex::new(None));
    let watcher_exit = Arc::clone(&child_exit);

    std::thread::spawn(move || {
        let result = wait_for_exit(pid);
        if let Ok(mut slot) = watcher_exit.lock() {
            *slot = Some(result);
        }
        let _wake_result = waiter_wake.send(WAKE_BYTE);
    });

    child_exit
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
fn capture_result<R>(
    result: io::Result<Option<CaptureReader<R>>>,
) -> (Option<CaptureReader<R>>, Option<io::Error>) {
    match result {
        Ok(reader) => (reader, None),
        Err(error) => (None, Some(error)),
    }
}

#[cfg(unix)]
fn configure_stream(command: &mut std::process::Command, stdout: bool, policy: StreamPolicy) {
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

#[cfg(unix)]
fn capture_stdout(
    child: &mut Child,
    policy: StreamPolicy,
) -> io::Result<Option<CaptureReader<ChildStdout>>> {
    match policy {
        StreamPolicy::Capture { limit } => {
            let reader = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("captured stdout pipe is absent"))?;
            set_nonblocking(&reader)?;
            Ok(Some(CaptureReader::new(reader, limit)))
        }
        StreamPolicy::Inherit | StreamPolicy::Null => Ok(None),
    }
}

#[cfg(unix)]
fn capture_stderr(
    child: &mut Child,
    policy: StreamPolicy,
) -> io::Result<Option<CaptureReader<ChildStderr>>> {
    match policy {
        StreamPolicy::Capture { limit } => {
            let reader = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("captured stderr pipe is absent"))?;
            set_nonblocking(&reader)?;
            Ok(Some(CaptureReader::new(reader, limit)))
        }
        StreamPolicy::Inherit | StreamPolicy::Null => Ok(None),
    }
}

#[cfg(unix)]
fn set_nonblocking(descriptor: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    let descriptor = descriptor.as_raw_fd();
    let flags = unsafe { platform::fcntl(descriptor, platform::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    let result =
        unsafe { platform::fcntl(descriptor, platform::F_SETFL, flags | platform::O_NONBLOCK) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
impl Running {
    pub fn id(&self) -> u32 {
        self.pid
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
                return self
                    .finish(Completion::Exited(status))
                    .map(WaitStep::Exited);
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
                return self.finish(Completion::Exited(status));
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

        Ok(WaitOutcome {
            completion,
            stdout: self.stdout.take().map(CaptureReader::finish),
            stderr: self.stderr.take().map(CaptureReader::finish),
        })
    }

    fn take_child_exit(&mut self) -> Result<Option<ExitStatus>, WaitError> {
        let exit = self
            .child_exit
            .lock()
            .map_err(|_| {
                self.io_error(
                    "reading child completion",
                    io::Error::other("child completion lock is poisoned"),
                )
            })?
            .take();

        match exit {
            Some(Ok(())) => self.reap_notified_child().map(Some),
            Some(Err(source)) => Err(self.io_error("watching for child completion", source)),
            None => Ok(None),
        }
    }

    fn reap_notified_child(&mut self) -> Result<ExitStatus, WaitError> {
        let status = self
            .child
            .as_mut()
            .expect("an unreaped process owns its child handle")
            .wait()
            .map_err(|source| self.io_error("reaping notified child", source))?;
        self.child.take();
        self.child_reaped = true;
        Ok(status)
    }

    fn block_for(&mut self, duration: Duration) -> Result<(), WaitError> {
        use std::os::fd::AsRawFd;

        let mut descriptors = Vec::with_capacity(3);
        descriptors.push(platform::PollFd::read(self.wake_receiver.as_raw_fd()));
        if let Some(stdout) = &self.stdout
            && !stdout.is_complete()
        {
            descriptors.push(platform::PollFd::read(stdout.raw_fd()));
        }
        if let Some(stderr) = &self.stderr
            && !stderr.is_complete()
        {
            descriptors.push(platform::PollFd::read(stderr.raw_fd()));
        }

        let result = unsafe {
            platform::poll(
                descriptors.as_mut_ptr(),
                platform::Nfds::try_from(descriptors.len()).expect("three poll descriptors fit"),
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
        if let Some(stdout) = &mut self.stdout {
            stdout.drain_available().map_err(|source| WaitError::Io {
                owner: self.spec.wait.owner().to_owned(),
                operation: "draining stdout",
                source,
            })?;
        }
        if let Some(stderr) = &mut self.stderr {
            stderr.drain_available().map_err(|source| WaitError::Io {
                owner: self.spec.wait.owner().to_owned(),
                operation: "draining stderr",
                source,
            })?;
        }
        Ok(())
    }

    fn outputs_complete(&self) -> bool {
        self.stdout.as_ref().is_none_or(CaptureReader::is_complete)
            && self.stderr.as_ref().is_none_or(CaptureReader::is_complete)
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
        if !self.child_reaped {
            let _signal_result = self.signal(platform::SIGKILL);
            let _reap_result = self.await_child_for(self.spec.deadlines.reap());
        }
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct CaptureReader<R> {
    reader: R,
    output: CapturedOutput,
    limit: usize,
}

#[cfg(unix)]
impl<R: Read + std::os::fd::AsRawFd> CaptureReader<R> {
    fn new(reader: R, limit: usize) -> Self {
        Self {
            reader,
            output: CapturedOutput {
                bytes: Vec::with_capacity(limit.min(8 * 1024)),
                truncated: false,
                complete: false,
            },
            limit,
        }
    }

    fn raw_fd(&self) -> std::os::fd::RawFd {
        self.reader.as_raw_fd()
    }

    fn drain_available(&mut self) -> io::Result<()> {
        let mut drained = 0;
        let mut buffer = [0_u8; 8 * 1024];

        while drained < READ_BUDGET_PER_EVENT {
            match self.reader.read(&mut buffer) {
                Ok(0) => {
                    self.output.complete = true;
                    return Ok(());
                }
                Ok(read) => {
                    drained += read;
                    let available = self.limit.saturating_sub(self.output.bytes.len());
                    let retained = available.min(read);
                    self.output.bytes.extend_from_slice(&buffer[..retained]);
                    self.output.truncated |= retained < read;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }

        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.output.complete
    }

    fn finish(self) -> CapturedOutput {
        self.output
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
mod platform;

#[cfg(all(test, unix))]
mod tests {
    use super::{Completion, spawn};
    use crate::lock::{Acquisition, LockFile, LockName, tests::unique_lock_path};
    use crate::spec::{
        Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup,
        Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
    };
    use std::fs;
    use std::time::Duration;

    #[test]
    fn output_capture_is_bounded_and_environment_is_explicit() {
        let spec = test_spec(
            "/bin/sh",
            ["-c", "printf '%s' \"$ONLY\"; printf '56789'"],
            Environment::new([("ONLY", "1234")]).expect("valid environment"),
            Cancellation::Never,
            Duration::from_secs(1),
            StreamPolicy::Capture { limit: 4 },
        );
        let outcome = spawn(spec)
            .expect("spawn bounded output child")
            .wait()
            .expect("wait for bounded output child");

        assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
        let stdout = outcome.stdout().expect("stdout was captured");
        assert_eq!(stdout.bytes(), b"1234");
        assert!(stdout.truncated());
        assert!(stdout.complete());
    }

    #[test]
    fn ambient_environment_is_cleared_before_explicit_entries_are_set() {
        let spec = test_spec(
            "/usr/bin/env",
            std::iter::empty::<&str>(),
            Environment::new([("SILL_ONLY", "exact")]).expect("valid environment"),
            Cancellation::Never,
            Duration::from_secs(1),
            StreamPolicy::Capture { limit: 1024 },
        );
        let outcome = spawn(spec)
            .expect("spawn environment probe")
            .wait()
            .expect("wait for environment probe");

        assert!(matches!(outcome.completion(), Completion::Exited(status) if status.success()));
        assert_eq!(
            outcome.stdout().expect("stdout was captured").bytes(),
            b"SILL_ONLY=exact\n"
        );
    }

    #[test]
    fn runtime_deadline_terminates_the_whole_process_group() {
        let spec = test_spec(
            "/bin/sh",
            ["-c", "/usr/bin/tail -f /dev/null & wait"],
            Environment::empty(),
            Cancellation::Never,
            Duration::from_millis(20),
            StreamPolicy::Null,
        );
        let outcome = spawn(spec)
            .expect("spawn process group")
            .wait()
            .expect("reap timed-out process group");

        assert!(matches!(outcome.completion(), Completion::TimedOut(_)));
    }

    #[test]
    fn cancellation_terminates_and_reaps_the_child() {
        let token = CancellationToken::new();
        let spec = test_spec(
            "/usr/bin/tail",
            ["-f", "/dev/null"],
            Environment::empty(),
            Cancellation::Token(token.clone()),
            Duration::from_secs(1),
            StreamPolicy::Null,
        );
        let running = spawn(spec).expect("spawn cancellable child");
        let waiter = std::thread::spawn(move || running.wait());
        assert!(
            token.wait_for_registration(Duration::from_secs(1)),
            "wait did not register its cancellation callback"
        );
        token.cancel();
        let outcome = waiter
            .join()
            .expect("cancellation waiter thread must not panic")
            .expect("reap cancelled child");

        assert!(matches!(outcome.completion(), Completion::Cancelled(_)));
    }

    #[test]
    fn exec_child_does_not_inherit_lock_possession() {
        let path = unique_lock_path("cloexec");
        let lock = LockFile::new(LockName::new("cloexec").expect("valid name"), &path);
        let possession = match lock.try_acquire().expect("acquire parent lock") {
            Acquisition::Acquired(possession) => possession,
            Acquisition::Contended => panic!("fresh parent lock must be available"),
        };
        let token = CancellationToken::new();
        let running = spawn(test_spec(
            "/usr/bin/tail",
            ["-f", "/dev/null"],
            Environment::empty(),
            Cancellation::Token(token.clone()),
            Duration::from_secs(1),
            StreamPolicy::Null,
        ))
        .expect("spawn child while lock is held");

        drop(possession);
        let reacquired = match lock.try_acquire().expect("reacquire after parent release") {
            Acquisition::Acquired(possession) => possession,
            Acquisition::Contended => panic!("exec child inherited the lock descriptor"),
        };
        token.cancel();
        assert!(matches!(
            running
                .wait()
                .expect("reap lock-inheritance probe")
                .completion(),
            Completion::Cancelled(_)
        ));
        drop(reacquired);
        fs::remove_file(path).expect("remove lock-inheritance test file");
    }

    fn test_spec<I, S>(
        program: &str,
        arguments: I,
        environment: Environment,
        cancellation: Cancellation,
        runtime: Duration,
        stdout: StreamPolicy,
    ) -> Spec
    where
        I: IntoIterator<Item = S>,
        S: Into<std::ffi::OsString>,
    {
        Spec {
            argv: Argv::new(program, arguments).expect("valid argv"),
            cwd: std::env::temp_dir(),
            environment,
            group: ProcessGroup::New,
            deadlines: Deadlines::new(
                runtime,
                Duration::from_millis(50),
                Duration::from_millis(100),
            )
            .expect("valid deadlines"),
            wait: WaitPolicy::new("sill spawn test", cancellation, RestartDisposition::Never)
                .expect("valid wait policy"),
            output: Output::new(
                StdinPolicy::Null,
                stdout,
                StreamPolicy::Capture { limit: 1024 },
            ),
            redaction: Redaction::All,
            grace: Duration::from_millis(10),
        }
    }
}
