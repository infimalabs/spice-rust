use super::*;

use std::any::Any;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread::JoinHandle;

pub(super) trait ChildControl: Send {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
}

impl ChildControl for Child {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        Child::wait(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExitOrigin {
    Natural,
    FixtureTermination,
}

pub(super) struct CleanupJob {
    child: Option<Box<dyn ChildControl>>,
    stdout: Option<Receiver<String>>,
    stdout_reader: Option<JoinHandle<Result<(), String>>>,
    stderr_reader: Option<JoinHandle<Result<Vec<u8>, String>>>,
    status: Option<ExitStatus>,
    exit_origin: Option<ExitOrigin>,
}

pub(super) struct CleanupReport {
    status: Option<ExitStatus>,
    exit_origin: Option<ExitOrigin>,
    stderr: Vec<u8>,
    observations: Vec<StreamOutcome>,
    errors: Vec<String>,
}

impl CleanupJob {
    pub(super) fn new(
        child: Option<Box<dyn ChildControl>>,
        stdout: Option<Receiver<String>>,
        stdout_reader: Option<JoinHandle<Result<(), String>>>,
        stderr_reader: Option<JoinHandle<Result<Vec<u8>, String>>>,
        status: Option<ExitStatus>,
        exit_origin: Option<ExitOrigin>,
    ) -> Self {
        Self {
            child,
            stdout,
            stdout_reader,
            stderr_reader,
            status,
            exit_origin,
        }
    }

    fn from_watch(watch: &mut WatchProcess) -> Self {
        Self::new(
            watch.child.take(),
            watch.stdout.take(),
            watch.stdout_reader.take(),
            watch.stderr_reader.take(),
            watch.status,
            watch.exit_origin,
        )
    }

    fn run(mut self) -> CleanupReport {
        let mut errors = Vec::new();
        self.stop_child(&mut errors);
        let observations = self.finish_stdout(&mut errors);
        let stderr = self.finish_stderr(&mut errors);
        CleanupReport {
            status: self.status,
            exit_origin: self.exit_origin,
            stderr,
            observations,
            errors,
        }
    }

    fn stop_child(&mut self, errors: &mut Vec<String>) {
        if self.status.is_some() {
            self.child.take();
            return;
        }
        let Some(mut child) = self.child.take() else {
            errors.push("watch child handle was already absent".to_owned());
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.status = Some(status);
                self.exit_origin = Some(ExitOrigin::Natural);
            }
            Ok(None) => self.terminate_and_reap(child.as_mut(), "watch", errors),
            Err(error) => {
                errors.push(format!("watch liveness probe failed: {error}"));
                self.terminate_and_reap(child.as_mut(), "watch after probe failure", errors);
            }
        }
    }

    fn terminate_and_reap(
        &mut self,
        child: &mut dyn ChildControl,
        context: &str,
        errors: &mut Vec<String>,
    ) {
        self.exit_origin = Some(ExitOrigin::FixtureTermination);
        if let Err(error) = child.kill() {
            errors.push(format!("{context} kill failed: {error}"));
        }
        match child.wait() {
            Ok(status) => self.status = Some(status),
            Err(error) => errors.push(format!("{context} reap failed: {error}")),
        }
    }

    fn finish_stdout(&mut self, errors: &mut Vec<String>) -> Vec<StreamOutcome> {
        if let Some(reader) = self.stdout_reader.take() {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(format!("watch stdout read failed: {error}")),
                Err(_) => errors.push("watch stdout reader panicked".to_owned()),
            }
        }
        let Some(receiver) = self.stdout.take() else {
            return Vec::new();
        };
        receiver
            .try_iter()
            .filter_map(
                |line| match wire::decode::<StreamOutcome>(line.as_bytes()) {
                    Ok(outcome) => Some(outcome),
                    Err(error) => {
                        errors.push(format!(
                            "shutdown stdout DTO was invalid: {error}; line={line:?}"
                        ));
                        None
                    }
                },
            )
            .collect()
    }

    fn finish_stderr(&mut self, errors: &mut Vec<String>) -> Vec<u8> {
        let Some(reader) = self.stderr_reader.take() else {
            return Vec::new();
        };
        match reader.join() {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => {
                errors.push(format!("watch stderr read failed: {error}"));
                Vec::new()
            }
            Err(_) => {
                errors.push("watch stderr reader panicked".to_owned());
                Vec::new()
            }
        }
    }
}

pub(super) struct CleanupWorker {
    report: Receiver<Result<CleanupReport, String>>,
    done: Receiver<()>,
    pending: Option<Result<CleanupReport, String>>,
}

impl CleanupWorker {
    pub(super) fn spawn(job: CleanupJob) -> Self {
        let (report_sender, report) = mpsc::channel();
        let (done_sender, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("watch-fixture-cleanup".to_owned())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| job.run()))
                    .map_err(|panic| panic_detail(panic.as_ref()));
                let _ = report_sender.send(result);
                let _ = done_sender.send(());
            })
            .expect("watch cleanup worker launches");
        Self {
            report,
            done,
            pending: None,
        }
    }

    pub(super) fn wait_until(
        mut self,
        deadline: Instant,
    ) -> Result<Result<CleanupReport, String>, Self> {
        if self.pending.is_none() {
            match receive_by(&self.report, deadline) {
                Ok(report) => self.pending = Some(report),
                Err(ReceiveByError::Timeout) => return Err(self),
                Err(ReceiveByError::Disconnected) => {
                    self.pending = Some(Err("watch cleanup worker disconnected".to_owned()));
                }
            }
        }
        match receive_by(&self.done, deadline) {
            Ok(()) | Err(ReceiveByError::Disconnected) => {
                Ok(self.pending.take().expect("cleanup report is retained"))
            }
            Err(ReceiveByError::Timeout) => Err(self),
        }
    }
}

enum ReceiveByError {
    Timeout,
    Disconnected,
}

fn receive_by<T>(receiver: &Receiver<T>, deadline: Instant) -> Result<T, ReceiveByError> {
    match receiver.try_recv() {
        Ok(value) => return Ok(value),
        Err(TryRecvError::Disconnected) => return Err(ReceiveByError::Disconnected),
        Err(TryRecvError::Empty) => {}
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ReceiveByError::Timeout);
    }
    receiver
        .recv_timeout(remaining)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => ReceiveByError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => ReceiveByError::Disconnected,
        })
}

fn panic_detail(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            panic
                .downcast_ref::<&str>()
                .map(|detail| (*detail).to_owned())
        })
        .unwrap_or_else(|| "watch cleanup worker panicked".to_owned())
}

pub(super) fn terminate_launch_child(child: Child, deadline: Instant) {
    let job = CleanupJob::new(Some(Box::new(child)), None, None, None, None, None);
    let _ = CleanupWorker::spawn(job).wait_until(deadline);
}

impl WatchProcess {
    pub(super) fn finish(mut self) {
        self.phase = "shutdown";
        self.shutdown();
        let failure = match self.exit_origin {
            Some(ExitOrigin::Natural) => {
                Some("watch exited naturally before explicit fixture termination")
            }
            Some(ExitOrigin::FixtureTermination)
                if self.cleanup_errors.is_empty() && !self.remaining().is_zero() =>
            {
                None
            }
            _ => Some("watch fixture cleanup failed"),
        };
        if let Some(detail) = failure {
            panic!("{}", self.diagnostic(detail));
        }
    }

    pub(super) fn fail(&mut self, detail: String) -> ! {
        let phase = self.phase;
        self.shutdown();
        panic!("{}", self.diagnostic_at(phase, &detail));
    }

    fn shutdown(&mut self) {
        if self.child.is_none()
            && self.stdout.is_none()
            && self.stdout_reader.is_none()
            && self.stderr_reader.is_none()
        {
            return;
        }
        let worker = CleanupWorker::spawn(CleanupJob::from_watch(self));
        match worker.wait_until(self.deadline) {
            Ok(Ok(report)) => self.accept_cleanup(report),
            Ok(Err(error)) => self
                .cleanup_errors
                .push(format!("watch cleanup worker failed: {error}")),
            Err(_) => self.cleanup_errors.push(cleanup_deadline_detail()),
        }
        if self.remaining().is_zero() {
            self.cleanup_errors
                .push("cleanup exhausted the test-wide deadline".to_owned());
        }
    }

    fn accept_cleanup(&mut self, mut report: CleanupReport) {
        self.status = report.status;
        self.exit_origin = report.exit_origin;
        self.stderr = report.stderr;
        self.observations.append(&mut report.observations);
        self.cleanup_errors.append(&mut report.errors);
    }

    fn diagnostic(&self, detail: &str) -> String {
        self.diagnostic_at(self.phase, detail)
    }

    fn diagnostic_at(&self, phase: &str, detail: &str) -> String {
        let status = self.status.map_or_else(
            || "running or unavailable".to_owned(),
            |status| status.to_string(),
        );
        format!(
            "{detail}; phase={phase}; child={status}; exit_origin={:?}; bound={WATCH_TEST_BOUND:?}; elapsed={:?}; remaining={:?}; stderr={:?}; cleanup_errors={:?}; outcomes={:#?}",
            self.exit_origin,
            self.started.elapsed(),
            self.remaining(),
            String::from_utf8_lossy(&self.stderr),
            self.cleanup_errors,
            self.observations,
        )
    }
}

pub(super) fn cleanup_deadline_detail() -> String {
    format!(
        "cleanup exceeded the original {WATCH_TEST_BOUND:?} test-wide deadline before kill/reap and stdout/stderr drain completion"
    )
}

impl Drop for WatchProcess {
    fn drop(&mut self) {
        if self.child.is_some()
            || self.stdout.is_some()
            || self.stdout_reader.is_some()
            || self.stderr_reader.is_some()
        {
            self.shutdown();
        }
    }
}
