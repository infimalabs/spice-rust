use super::*;

use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Observation, ObserveError,
    ObservedRecord, Output, OutputStream, ProcessGroup, RecordObserver, Redaction,
    RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy, WaitStep,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const OUTPUT_QUEUE: usize = 64;
const OUTPUT_DRAIN: Duration = Duration::from_millis(500);
const REAP_DEADLINE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExitOrigin {
    Natural,
    FixtureTermination,
}

pub(super) enum WatchEvent {
    Record(Vec<u8>),
    Exited,
    Failed(String),
}

struct WatchObserver {
    events: mpsc::Sender<WatchEvent>,
}

impl RecordObserver for WatchObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        self.events
            .send(WatchEvent::Record(record.bytes().to_vec()))
            .map_err(|error| ObserveError::new(format!("watch record receiver closed: {error}")))
    }

    fn failed(&self, failure: &sill::ObservationFailure) {
        let _ = self.events.send(WatchEvent::Failed(failure.to_string()));
    }

    fn exited(&self) {
        let _ = self.events.send(WatchEvent::Exited);
    }
}

pub(super) fn spawn_watch(
    root: &str,
    deadline: Instant,
) -> Result<(sill::Running, mpsc::Receiver<WatchEvent>), String> {
    let runtime = deadline
        .saturating_duration_since(Instant::now())
        .saturating_sub(CLEANUP_RESERVE);
    let argv = Argv::new(
        env!("CARGO_BIN_EXE_spice"),
        [
            OsString::from("--json"),
            OsString::from("watch"),
            root.into(),
        ],
    )
    .map_err(|error| error.to_string())?;
    spawn_observed(argv, watch_environment()?, runtime)
}

#[cfg(all(test, unix))]
pub(super) fn spawn_fixture(
    script: &str,
    runtime: Duration,
) -> Result<(sill::Running, mpsc::Receiver<WatchEvent>), String> {
    let argv = Argv::new(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("watch-cleanup-fixture"),
        ],
    )
    .map_err(|error| error.to_string())?;
    spawn_observed(argv, Environment::empty(), runtime)
}

fn spawn_observed(
    argv: Argv,
    environment: Environment,
    runtime: Duration,
) -> Result<(sill::Running, mpsc::Receiver<WatchEvent>), String> {
    let (sender, events) = mpsc::channel();
    let observation = Observation::new(
        OutputStream::Stdout,
        OUTPUT_LIMIT,
        OUTPUT_QUEUE,
        OUTPUT_DRAIN,
        Arc::new(WatchObserver { events: sender }),
    )
    .map_err(|error| error.to_string())?;
    let capture = StreamPolicy::Capture {
        limit: OUTPUT_LIMIT,
    };
    let spec = Spec {
        argv,
        cwd: std::env::current_dir().map_err(|error| error.to_string())?,
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, OUTPUT_DRAIN, REAP_DEADLINE)
            .map_err(|error| error.to_string())?,
        wait: WaitPolicy::new(
            "real watch fixture",
            Cancellation::Never,
            RestartDisposition::Never,
        )
        .map_err(|error| error.to_string())?,
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: Duration::ZERO,
    };
    sill::spawn_observed(spec, observation)
        .map(|running| (running, events))
        .map_err(|error| error.to_string())
}

#[expect(
    clippy::disallowed_methods,
    reason = "the integration fixture preserves the launched CLI environment except for lane identity"
)]
fn watch_environment() -> Result<Environment, String> {
    Environment::new(std::env::vars_os().filter(|(name, _)| name != OsStr::new(steer::hook::LANE)))
        .map_err(|error| error.to_string())
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
        let Some(process) = self.process.take() else {
            return;
        };
        let waited = if self.exit_origin == Some(ExitOrigin::Natural) {
            process.wait()
        } else {
            match process.wait_for(Duration::ZERO) {
                Ok(WaitStep::Exited(waited)) => {
                    self.exit_origin = Some(ExitOrigin::Natural);
                    Ok(waited)
                }
                Ok(WaitStep::Running(process)) => {
                    self.exit_origin = Some(ExitOrigin::FixtureTermination);
                    if let Err(error) = process.signal_kill() {
                        self.cleanup_errors
                            .push(format!("watch process-group kill failed: {error}"));
                    }
                    process.wait()
                }
                Err(error) => Err(error),
            }
        };
        match waited {
            Ok(waited) => self.accept_waited(&waited),
            Err(error) => self
                .cleanup_errors
                .push(format!("watch process cleanup failed: {error}")),
        }
        self.drain_events();
        if self.remaining().is_zero() {
            self.cleanup_errors.push(format!(
                "kill/reap and stdout/stderr drain exceeded the original {WATCH_TEST_BOUND:?} test-wide deadline"
            ));
        }
    }

    fn accept_waited(&mut self, waited: &WaitOutcome) {
        self.status = Some(waited.completion().status());
        if matches!(waited.completion(), Completion::TimedOut(_)) {
            self.cleanup_errors
                .push("watch process reached its supervised runtime deadline".to_owned());
        }
        if !waited.outputs_drained() {
            self.cleanup_errors
                .push("watch stdout/stderr did not drain within the original deadline".to_owned());
        }
        if let Some(stderr) = waited.stderr() {
            self.stderr = stderr.bytes().to_vec();
            if stderr.truncated() {
                self.cleanup_errors
                    .push("watch stderr exceeded the fixture capture limit".to_owned());
            }
            if !stderr.complete() {
                self.cleanup_errors
                    .push("watch stderr capture was incomplete".to_owned());
            }
        }
        if let Some(failure) = waited.observation_failure() {
            self.cleanup_errors
                .push(format!("watch output observation failed: {failure}"));
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                WatchEvent::Record(record) => match wire::decode::<StreamOutcome>(&record) {
                    Ok(outcome) => self.observations.push(outcome),
                    Err(error) => self.cleanup_errors.push(format!(
                        "shutdown stdout DTO was invalid: {error}; record={:?}",
                        String::from_utf8_lossy(&record)
                    )),
                },
                WatchEvent::Exited => {}
                WatchEvent::Failed(detail) => self
                    .cleanup_errors
                    .push(format!("watch output observation failed: {detail}")),
            }
        }
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

impl Drop for WatchProcess {
    fn drop(&mut self) {
        if self.process.is_some() {
            self.shutdown();
        }
    }
}
