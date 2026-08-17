use super::*;

const WATCH_TEST_BOUND: Duration = Duration::from_secs(30);
const RECEIVE_SLICE: Duration = Duration::from_millis(250);
const CLEANUP_RESERVE: Duration = Duration::from_secs(2);

#[test]
fn watch_follows_a_real_filesystem_notification_then_reconciles_tape() {
    let scratch = Scratch::new("watch-notification");
    let (_, tape) = session(scratch.path());
    let root = scratch.path().to_string_lossy().into_owned();
    let started = Instant::now();
    let deadline = started + WATCH_TEST_BOUND;
    let mut watch = WatchProcess::spawn(&root, started, deadline);

    watch.assert_initial_cursor();
    watch.await_readiness(scratch.path());
    if let Err(error) = tape.tee(&record(CODEX_PROSE)) {
        watch.fail(format!("post-readiness tape write failed: {error}"));
    }
    watch.await_tape_event();
    watch.finish();
    scratch.remove();
}

/// Panic-safe ownership of the endless watcher and both pipe readers.
struct WatchProcess {
    child: Option<std::process::Child>,
    stdout: Option<mpsc::Receiver<String>>,
    stdout_reader: Option<std::thread::JoinHandle<Result<(), String>>>,
    stderr_reader: Option<std::thread::JoinHandle<Result<Vec<u8>, String>>>,
    status: Option<std::process::ExitStatus>,
    stderr: Vec<u8>,
    observations: Vec<StreamOutcome>,
    cleanup_errors: Vec<String>,
    phase: &'static str,
    started: Instant,
    deadline: Instant,
}

/// Reap a spawned child even if pipe or reader setup panics before admission.
struct LaunchChild(Option<std::process::Child>);

impl LaunchChild {
    fn child(&mut self) -> &mut std::process::Child {
        self.0.as_mut().expect("launch child remains owned")
    }

    fn admit(mut self) -> std::process::Child {
        self.0.take().expect("launch child remains owned")
    }
}

impl Drop for LaunchChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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
        let mut child = LaunchChild(Some(child.spawn().expect("watch launches")));
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

    fn await_tape_event(&mut self) {
        self.receive_until(
            "tape event reconciliation",
            || Ok(()),
            |outcome| {
                if outcome.mode != Mode::Watch {
                    return Err(format!(
                        "reconciliation emitted the wrong mode: {outcome:?}"
                    ));
                }
                Ok(outcome
                    .observations
                    .iter()
                    .any(|observation| matches!(observation, Observation::Event { .. })))
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
                self.child.take();
                Some(status.to_string())
            }
            Ok(None) => None,
            Err(error) => Some(format!("liveness probe failed: {error}")),
        }
    }

    fn finish(mut self) {
        self.phase = "shutdown";
        self.shutdown();
        let failed = self.status.is_none()
            || self.status.is_some_and(|status| status.success())
            || !self.cleanup_errors.is_empty()
            || self.remaining().is_zero();
        if failed {
            panic!("{}", self.diagnostic("watch fixture cleanup failed"));
        }
    }

    fn fail(&mut self, detail: String) -> ! {
        let phase = self.phase;
        self.shutdown();
        panic!("{}", self.diagnostic_at(phase, &detail));
    }

    fn shutdown(&mut self) {
        self.stop_child();
        self.join_stdout();
        self.join_stderr();
        if self.remaining().is_zero() {
            self.cleanup_errors
                .push("cleanup exhausted the test-wide deadline".to_owned());
        }
    }

    fn stop_child(&mut self) {
        if self.status.is_some() {
            self.child.take();
            return;
        }
        let Some(mut child) = self.child.take() else {
            self.cleanup_errors
                .push("watch child handle was already absent".to_owned());
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => self.status = Some(status),
            Ok(None) => self.terminate_and_reap(&mut child, "watch"),
            Err(error) => {
                self.cleanup_errors
                    .push(format!("watch liveness probe failed: {error}"));
                self.terminate_and_reap(&mut child, "watch after probe failure");
            }
        }
    }

    fn terminate_and_reap(&mut self, child: &mut std::process::Child, context: &str) {
        if let Err(error) = child.kill() {
            self.cleanup_errors
                .push(format!("{context} kill failed: {error}"));
        }
        match child.wait() {
            Ok(status) => self.status = Some(status),
            Err(error) => self
                .cleanup_errors
                .push(format!("{context} reap failed: {error}")),
        }
    }

    fn join_stdout(&mut self) {
        if let Some(reader) = self.stdout_reader.take() {
            match reader.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => self
                    .cleanup_errors
                    .push(format!("watch stdout read failed: {error}")),
                Err(_) => self
                    .cleanup_errors
                    .push("watch stdout reader panicked".to_owned()),
            }
        }
        let Some(receiver) = self.stdout.take() else {
            return;
        };
        for line in receiver.try_iter() {
            match wire::decode::<StreamOutcome>(line.as_bytes()) {
                Ok(outcome) => self.observations.push(outcome),
                Err(error) => self.cleanup_errors.push(format!(
                    "shutdown stdout DTO was invalid: {error}; line={line:?}"
                )),
            }
        }
    }

    fn join_stderr(&mut self) {
        let Some(reader) = self.stderr_reader.take() else {
            return;
        };
        match reader.join() {
            Ok(Ok(bytes)) => self.stderr = bytes,
            Ok(Err(error)) => self
                .cleanup_errors
                .push(format!("watch stderr read failed: {error}")),
            Err(_) => self
                .cleanup_errors
                .push("watch stderr reader panicked".to_owned()),
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
            "{detail}; phase={phase}; child={status}; elapsed={:?}; remaining={:?}; stderr={:?}; cleanup_errors={:?}; outcomes={:#?}",
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
        if self.child.is_some()
            || self.stdout.is_some()
            || self.stdout_reader.is_some()
            || self.stderr_reader.is_some()
        {
            self.shutdown();
        }
    }
}
