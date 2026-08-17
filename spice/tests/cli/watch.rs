use super::*;

#[path = "watch/cleanup.rs"]
mod cleanup;
#[cfg(test)]
#[path = "watch/tests.rs"]
mod tests;

use cleanup::{ChildControl, ExitOrigin};

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
    let locus = match tape.tee(&record(CODEX_PROSE)) {
        Ok(locus) => locus,
        Err(error) => watch.fail(format!("post-readiness tape write failed: {error}")),
    };
    watch.await_tape_event(ExpectedTapeEvent::new(tape_path, locus));
    watch.finish();
    scratch.remove();
}

/// Panic-safe ownership of the endless watcher and both pipe readers.
struct WatchProcess {
    child: Option<Box<dyn ChildControl>>,
    stdout: Option<mpsc::Receiver<String>>,
    stdout_reader: Option<std::thread::JoinHandle<Result<(), String>>>,
    stderr_reader: Option<std::thread::JoinHandle<Result<Vec<u8>, String>>>,
    status: Option<std::process::ExitStatus>,
    exit_origin: Option<ExitOrigin>,
    stderr: Vec<u8>,
    observations: Vec<StreamOutcome>,
    cleanup_errors: Vec<String>,
    phase: &'static str,
    started: Instant,
    deadline: Instant,
}

/// Reap a spawned child even if pipe or reader setup panics before admission.
struct LaunchChild {
    child: Option<std::process::Child>,
    deadline: Instant,
}

impl LaunchChild {
    fn new(child: std::process::Child, deadline: Instant) -> Self {
        Self {
            child: Some(child),
            deadline,
        }
    }

    fn child(&mut self) -> &mut std::process::Child {
        self.child.as_mut().expect("launch child remains owned")
    }

    fn admit(mut self) -> Box<dyn ChildControl> {
        Box::new(self.child.take().expect("launch child remains owned"))
    }
}

impl Drop for LaunchChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            cleanup::terminate_launch_child(child, self.deadline);
        }
    }
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
                    dialect: Dialect::Codex,
                    tape,
                    spans,
                    event: SurfaceEvent::Prose { text, phase, media },
                } if lane == "lane-a"
                    && tape == &self.tape
                    && spans.as_slice() == [self.span]
                    && text == CODEX_PROSE_TEXT
                    && phase.is_none()
                    && media.is_empty()
            )
        })
    }
}

impl WatchProcess {
    fn spawn(root: &str, started: Instant, deadline: Instant) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spice"));
        child
            .args(["--json", "watch", root])
            .env_remove(steer::hook::INBOX)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = LaunchChild::new(child.spawn().expect("watch launches"), deadline);
        let stdout = child.child().stdout.take().expect("stdout is piped");
        let stderr = child.child().stderr.take().expect("stderr is piped");

        let (sender, receiver) = mpsc::channel();
        let stdout_reader = std::thread::Builder::new()
            .name("watch-fixture-stdout".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let line = line.map_err(|error| error.to_string())?;
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Ok(())
            })
            .expect("stdout reader launches");
        let stderr_reader = std::thread::Builder::new()
            .name("watch-fixture-stderr".to_owned())
            .spawn(move || {
                let mut stderr = stderr;
                let mut bytes = Vec::new();
                stderr
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                Ok(bytes)
            })
            .expect("stderr reader launches");

        Self {
            child: Some(child.admit()),
            stdout: Some(receiver),
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
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
            let received = self
                .stdout
                .as_ref()
                .expect("stdout receiver remains owned")
                .recv_timeout(remaining.min(RECEIVE_SLICE));
            match received {
                Ok(line) => match self.decode_and_match(line, &mut matches) {
                    Some(outcome) => return outcome,
                    None => continue,
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.resume_after_timeout(&mut before_slice);
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
        line: String,
        matches: &mut impl FnMut(&StreamOutcome) -> Result<bool, String>,
    ) -> Option<StreamOutcome> {
        let outcome = match wire::decode::<StreamOutcome>(line.as_bytes()) {
            Ok(outcome) => outcome,
            Err(error) => self.fail(format!(
                "watch emitted an invalid stream DTO: {error}; line={line:?}"
            )),
        };
        self.observations.push(outcome.clone());
        match matches(&outcome) {
            Ok(true) => Some(outcome),
            Ok(false) => None,
            Err(error) => self.fail(error),
        }
    }

    fn resume_after_timeout(&mut self, before_slice: &mut impl FnMut() -> Result<(), String>) {
        if let Some(status) = self.poll_status_if_exited() {
            self.fail(format!("watch exited early; child={status}"));
        }
        self.run_before_slice(before_slice);
    }

    fn fail_disconnected(&mut self) -> ! {
        let status = self
            .poll_status_if_exited()
            .unwrap_or_else(|| "running with a closed stdout pipe".to_owned());
        self.fail(format!(
            "watch stdout reader disconnected early; child={status}"
        ));
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

    fn poll_status_if_exited(&mut self) -> Option<String> {
        if let Some(status) = self.status {
            return Some(status.to_string());
        }
        let Some(child) = self.child.as_mut() else {
            return Some("missing child handle".to_owned());
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.status = Some(status);
                self.exit_origin = Some(ExitOrigin::Natural);
                self.child.take();
                Some(status.to_string())
            }
            Ok(None) => None,
            Err(error) => Some(format!("liveness probe failed: {error}")),
        }
    }
}
