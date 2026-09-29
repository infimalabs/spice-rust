use super::*;

#[path = "watch/cleanup.rs"]
mod cleanup;
#[cfg(test)]
#[path = "watch/tests.rs"]
mod tests;

use cleanup::{ExitOrigin, WatchEvent};

const WATCH_TEST_BOUND: Duration = Duration::from_secs(30);
const RECEIVE_SLICE: Duration = Duration::from_millis(250);
const CLEANUP_RESERVE: Duration = Duration::from_secs(2);

#[test]
fn watch_follows_a_real_filesystem_notification_then_reconciles_tape() {
    let scratch = Scratch::new("watch-notification");
    let (tape_path, tape) = session(scratch.path());
    let root = scratch.path().to_string_lossy().into_owned();
    let started = Instant::now();
    let deadline = started + WATCH_TEST_BOUND;
    let mut watch = WatchProcess::spawn(&root, started, deadline);

    watch.assert_initial_cursor();
    watch.await_readiness(scratch.path());
    let locus = match tape.tee(&record(PROSE)) {
        Ok(locus) => locus,
        Err(error) => watch.fail(format!("post-readiness tape write failed: {error}")),
    };
    watch.await_tape_event(ExpectedTapeEvent::new(tape_path, locus));
    watch.finish();
    scratch.remove();
}

/// Panic-safe ownership of the endless watcher and both pipe readers.
struct WatchProcess {
    process: Option<sill::Running>,
    events: mpsc::Receiver<WatchEvent>,
    status: Option<std::process::ExitStatus>,
    exit_origin: Option<ExitOrigin>,
    stderr: Vec<u8>,
    observations: Vec<StreamOutcome>,
    cleanup_errors: Vec<String>,
    phase: &'static str,
    started: Instant,
    deadline: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedTapeEvent {
    tape: PathBuf,
    span: Span,
}

impl ExpectedTapeEvent {
    fn new(tape: PathBuf, locus: tape::Locus) -> Self {
        Self {
            tape,
            span: locus.into(),
        }
    }

    fn appears_in(&self, outcome: &StreamOutcome) -> bool {
        outcome.observations.iter().any(|observation| {
            matches!(
                observation,
                Observation::Event {
                    lane,
                    dialect: Dialect::Omp,
                    tape,
                    spans,
                    event: SurfaceEvent::Prose { text, phase, media },
                } if lane == "lane-a"
                    && tape == &self.tape
                    && spans.as_slice() == [self.span]
                    && text == PROSE_TEXT
                    && phase.is_none()
                    && media.is_empty()
            )
        })
    }
}

impl WatchProcess {
    fn spawn(root: &str, started: Instant, deadline: Instant) -> Self {
        let (process, events) = cleanup::spawn_watch(root, deadline).expect("watch launches");

        Self {
            process: Some(process),
            events,
            status: None,
            stderr: Vec::new(),
            observations: Vec::new(),
            cleanup_errors: Vec::new(),
            exit_origin: None,
            phase: "spawn",
            started,
            deadline,
        }
    }

    fn assert_initial_cursor(&mut self) {
        let initial = self.receive_until("initial cursor", || Ok(()), |_| Ok(true));
        self.require(
            initial.mode == Mode::Watch
                && matches!(
                    initial.observations.as_slice(),
                    [Observation::Absent { .. }]
                ),
            format!("initial cursor was not the expected watch absence: {initial:?}"),
        );
    }

    fn await_readiness(&mut self, root: &Path) {
        let readiness = root.join("watch-readiness.probe");
        let mut attempt = 0_u64;
        self.receive_until(
            "probe readiness",
            || {
                attempt += 1;
                fs::write(&readiness, attempt.to_string())
                    .map_err(|error| format!("readiness probe write failed: {error}"))
            },
            |outcome| {
                if outcome
                    .observations
                    .iter()
                    .any(|observation| matches!(observation, Observation::Event { .. }))
                {
                    return Err(format!(
                        "readiness probe manufactured a tape event: {outcome:?}"
                    ));
                }
                if outcome.mode != Mode::Watch {
                    return Err(format!("readiness emitted the wrong mode: {outcome:?}"));
                }
                Ok(true)
            },
        );
    }

    fn await_tape_event(&mut self, expected: ExpectedTapeEvent) {
        self.receive_until(
            "tape event reconciliation",
            || Ok(()),
            |outcome| {
                if outcome.mode != Mode::Watch {
                    return Err(format!(
                        "reconciliation emitted the wrong mode: {outcome:?}"
                    ));
                }
                Ok(expected.appears_in(outcome))
            },
        );
    }

    fn receive_until(
        &mut self,
        phase: &'static str,
        mut before_slice: impl FnMut() -> Result<(), String>,
        mut matches: impl FnMut(&StreamOutcome) -> Result<bool, String>,
    ) -> StreamOutcome {
        self.phase = phase;
        self.run_before_slice(&mut before_slice);
        loop {
            let remaining = self.operational_remaining();
            if remaining.is_zero() {
                self.fail("the single test-wide observation budget expired".to_owned());
            }
            let received = self.events.recv_timeout(remaining.min(RECEIVE_SLICE));
            match received {
                Ok(WatchEvent::Record(record)) => match self.decode_and_match(record, &mut matches)
                {
                    Some(outcome) => return outcome,
                    None => continue,
                },
                Ok(WatchEvent::Exited) => {
                    self.exit_origin = Some(ExitOrigin::Natural);
                    self.fail("watch exited before explicit fixture termination".to_owned());
                }
                Ok(WatchEvent::Failed(detail)) => {
                    self.fail(format!("watch output observation failed: {detail}"));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.run_before_slice(&mut before_slice);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => self.fail_disconnected(),
            }
        }
    }

    fn run_before_slice(&mut self, before_slice: &mut impl FnMut() -> Result<(), String>) {
        if let Err(error) = before_slice() {
            self.fail(error);
        }
    }

    fn decode_and_match(
        &mut self,
        record: Vec<u8>,
        matches: &mut impl FnMut(&StreamOutcome) -> Result<bool, String>,
    ) -> Option<StreamOutcome> {
        let outcome = match wire::decode::<StreamOutcome>(&record) {
            Ok(outcome) => outcome,
            Err(error) => self.fail(format!(
                "watch emitted an invalid stream DTO: {error}; record={:?}",
                String::from_utf8_lossy(&record)
            )),
        };
        self.observations.push(outcome.clone());
        match matches(&outcome) {
            Ok(true) => Some(outcome),
            Ok(false) => None,
            Err(error) => self.fail(error),
        }
    }

    fn fail_disconnected(&mut self) -> ! {
        self.fail("watch observation channel disconnected early".to_owned());
    }

    fn require(&mut self, condition: bool, detail: String) {
        if !condition {
            self.fail(detail);
        }
    }

    fn operational_remaining(&self) -> Duration {
        self.remaining().saturating_sub(CLEANUP_RESERVE)
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}
