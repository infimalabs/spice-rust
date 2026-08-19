use super::*;

pub(super) struct CleanupFaultRunner {
    pub(super) sibling_started: AtomicBool,
}

pub(super) struct WorkerPanicRunner;

impl Runner for WorkerPanicRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        panic!("fixture verifier worker panic")
    }
    fn run(&self, step: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        if step.label == VERIFY_ACTIONS[0].label() {
            Ok(timed_command(true))
        } else {
            panic!("a successor crossed a panicked prerequisite")
        }
    }
    fn run_with_readiness(
        &self,
        _: Step,
        _: &CancellationToken,
        _: usize,
        _: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        panic!("a readiness action crossed a panicked prerequisite")
    }
    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("scoped route is not under test")
    }
    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        panic!("browser crossed a panicked prerequisite")
    }
    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        panic!("workspace prebuild crossed a panicked prerequisite")
    }
    fn workspace_tests(
        &self,
        _: &CancellationToken,
        _: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        panic!("workspace tests crossed a panicked prerequisite")
    }
    fn structural_preflight(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("structural preflight crossed a panicked prerequisite")
    }
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(BatteryRun::Cancelled)
    }
    fn accept_battery_state(&self) -> Result<(), String> {
        panic!("state acceptance crossed a panicked prerequisite")
    }
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        panic!("evidence publication crossed a panicked prerequisite")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed a panicked prerequisite")
    }
}

impl Runner for CleanupFaultRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        readiness();
        self.run(step, cancellation, jobs)
    }
    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("scoped route is not under test")
    }
    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        while !self.sibling_started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        Ok(BrowserRun {
            process: timed_command(true),
            completed: false,
        })
    }
    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        _: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        self.sibling_started.store(true, Ordering::Release);
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        Err(RunError::Cleanup("fixture sibling reap fault".to_owned()))
    }
    fn structural_preflight(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError> {
        Ok(if cancellation.is_cancelled() {
            BatteryRun::Cancelled
        } else {
            BatteryRun::Admitted(Vec::new())
        })
    }
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("battery crossed a failed prerequisite")
    }
    fn accept_battery_state(&self) -> Result<(), String> {
        panic!("state acceptance crossed a failed prerequisite")
    }
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        panic!("evidence publication crossed a failed prerequisite")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed a failed prerequisite")
    }
}

#[cfg(unix)]
pub(super) struct InterruptRunner {
    pub(super) pid_file: PathBuf,
}

#[cfg(unix)]
impl InterruptRunner {
    pub(super) fn new() -> Self {
        Self {
            pid_file: fixture_pid_file("cancel"),
        }
    }
}

#[cfg(unix)]
impl Runner for InterruptRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(
        &self,
        cancellation: &CancellationToken,
        jobs: usize,
    ) -> Result<CommandRun, RunError> {
        supervised_sleep(&self.pid_file, cancellation, jobs)
    }
    fn run(&self, step: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        if step.label == VERIFY_ACTIONS[0].label() {
            Ok(timed_command(true))
        } else {
            panic!("a successor crossed interruption")
        }
    }
    fn run_with_readiness(
        &self,
        _: Step,
        _: &CancellationToken,
        _: usize,
        _: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        panic!("a readiness action crossed interruption")
    }
    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("scoped route is not under test")
    }
    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        panic!("browser crossed interruption")
    }
    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        panic!("workspace prebuild crossed interruption")
    }
    fn workspace_tests(
        &self,
        _: &CancellationToken,
        _: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        panic!("workspace tests crossed interruption")
    }
    fn structural_preflight(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("structural preflight crossed interruption")
    }
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        while !cancellation.is_cancelled() {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(BatteryRun::Cancelled)
    }
    fn accept_battery_state(&self) -> Result<(), String> {
        panic!("state acceptance crossed interruption")
    }
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        panic!("evidence publication crossed interruption")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed interruption")
    }
}

#[cfg(unix)]
pub(super) struct ScopedInterruptRunner {
    pub(super) pid_file: PathBuf,
}

#[cfg(unix)]
impl ScopedInterruptRunner {
    pub(super) fn new() -> Self {
        Self {
            pid_file: fixture_pid_file("scoped-cancel"),
        }
    }
}

#[cfg(unix)]
impl Runner for ScopedInterruptRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn run_with_readiness(
        &self,
        _: Step,
        _: &CancellationToken,
        _: usize,
        _: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        cancellation: &CancellationToken,
        jobs: usize,
    ) -> Result<CommandRun, RunError> {
        supervised_sleep(&self.pid_file, cancellation, jobs)
    }
    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn workspace_tests(
        &self,
        _: &CancellationToken,
        _: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn structural_preflight(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn accept_battery_state(&self) -> Result<(), String> {
        unreachable!("full route is not under test")
    }
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        unreachable!("full route is not under test")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        unreachable!("full route is not under test")
    }
}

#[cfg(unix)]
pub(super) struct FailureRunner {
    pub(super) pid_file: PathBuf,
}

#[cfg(unix)]
pub(super) struct DetachedShardRunner {
    pub(super) root_pid_file: PathBuf,
    pub(super) detached_identity_file: PathBuf,
    detached_publication: FixturePublicationObserver,
    owner_identity_file: Option<PathBuf>,
    owner_publication: Option<FixturePublicationObserver>,
    test_executable: PathBuf,
}

#[cfg(unix)]
pub(super) struct FixturePublicationObserver {
    receiver: std::os::unix::net::UnixDatagram,
    path: PathBuf,
}

#[cfg(unix)]
impl FixturePublicationObserver {
    pub(super) fn new(label: &str) -> Self {
        let path = fixture_pid_file(&format!("{label}-ready"));
        let receiver = std::os::unix::net::UnixDatagram::bind(&path)
            .expect("bind fixture publication observer");
        receiver
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("bound fixture publication observation");
        Self { receiver, path }
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(super) fn wait(&self, publication: &std::path::Path) {
        let mut event = [0_u8; 1];
        let received = self
            .receiver
            .recv(&mut event)
            .expect("fixture publication arrives within its five-second bound");
        assert_eq!(received, 1, "fixture publication event is exact");
        assert!(
            publication.is_file(),
            "fixture announced publication before committing {}",
            publication.display(),
        );
    }
}

#[cfg(unix)]
impl Drop for FixturePublicationObserver {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
impl DetachedShardRunner {
    pub(super) fn new() -> Self {
        Self {
            root_pid_file: fixture_pid_file("detached-shard-root"),
            detached_identity_file: fixture_pid_file("detached-shard-child"),
            detached_publication: FixturePublicationObserver::new("detached-shard"),
            owner_identity_file: None,
            owner_publication: None,
            test_executable: std::env::current_exe().expect("fixture test executable resolves"),
        }
    }

    pub(super) fn term_forking() -> Self {
        Self {
            root_pid_file: fixture_pid_file("term-fork-shard-root"),
            detached_identity_file: fixture_pid_file("term-fork-shard-child"),
            detached_publication: FixturePublicationObserver::new("term-fork-shard-child"),
            owner_identity_file: Some(fixture_pid_file("term-fork-shard-owner")),
            owner_publication: Some(FixturePublicationObserver::new("term-fork-shard-owner")),
            test_executable: std::env::current_exe().expect("fixture test executable resolves"),
        }
    }

    pub(super) fn owner_identity_file(&self) -> Option<&std::path::Path> {
        self.owner_identity_file.as_deref()
    }

    pub(super) fn wait_for_detached_publication(&self) {
        self.detached_publication.wait(&self.detached_identity_file);
    }
}

#[cfg(unix)]
impl Drop for DetachedShardRunner {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.root_pid_file);
        let _ = fs::remove_file(&self.detached_identity_file);
        if let Some(owner_identity) = &self.owner_identity_file {
            let _ = fs::remove_file(owner_identity);
        }
    }
}

#[cfg(unix)]
impl FailureRunner {
    pub(super) fn new() -> Self {
        Self {
            pid_file: fixture_pid_file("failure"),
        }
    }
}

#[cfg(unix)]
impl Runner for FailureRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        readiness();
        self.run(step, cancellation, jobs)
    }
    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("scoped route is not under test")
    }
    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        wait_for_path(&self.pid_file);
        Ok(BrowserRun {
            process: timed_command(true),
            completed: false,
        })
    }
    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }
    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        jobs: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        supervised_sleep(&self.pid_file, cancellation, jobs)
    }
    fn structural_preflight(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError> {
        Ok(if cancellation.is_cancelled() {
            BatteryRun::Cancelled
        } else {
            BatteryRun::Admitted(Vec::new())
        })
    }
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("battery crossed the primary failure")
    }
    fn accept_battery_state(&self) -> Result<(), String> {
        panic!("state acceptance crossed the primary failure")
    }
    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        panic!("evidence publication crossed the primary failure")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed the primary failure")
    }
}

#[cfg(unix)]
impl Runner for DetachedShardRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }

    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }

    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }

    fn run_with_readiness(
        &self,
        step: Step,
        cancellation: &CancellationToken,
        jobs: usize,
        readiness: ActionReadiness,
    ) -> Result<CommandRun, RunError> {
        readiness();
        self.run(step, cancellation, jobs)
    }

    fn run_scoped(
        &self,
        _: ScopedCommand,
        _: &str,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        unreachable!("scoped route is not under test")
    }

    fn browser(&self, _: &CancellationToken, _: usize) -> Result<BrowserRun, RunError> {
        match (&self.owner_identity_file, &self.owner_publication) {
            (Some(owner_identity), Some(owner_publication)) => {
                owner_publication.wait(owner_identity);
                assert!(
                    !self.detached_identity_file.exists(),
                    "late detached shard existed before cancellation"
                );
            }
            (None, None) => self.detached_publication.wait(&self.detached_identity_file),
            _ => panic!("TERM-fork fixture owner contract is partial"),
        }
        Ok(BrowserRun {
            process: timed_command(true),
            completed: false,
        })
    }

    fn workspace_test_prebuild(
        &self,
        _: &CancellationToken,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        Ok(timed_command(true))
    }

    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        jobs: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        let (fixture_test, fixture_environment, fixture_contract) =
            match (&self.owner_identity_file, &self.owner_publication) {
                (Some(owner_identity), Some(owner_publication)) => (
                    TERM_FORK_WORKSPACE_SHARD_TEST,
                    TERM_FORK_FIXTURE_CONTRACT_ENVIRONMENT,
                    term_fork_fixture_contract(
                        owner_identity,
                        owner_publication.path(),
                        &self.detached_identity_file,
                        self.detached_publication.path(),
                        5000,
                    ),
                ),
                (None, None) => (
                    DETACHED_WORKSPACE_SHARD_TEST,
                    DETACHED_FIXTURE_CONTRACT_ENVIRONMENT,
                    detached_fixture_contract(
                        &self.detached_identity_file,
                        self.detached_publication.path(),
                        5000,
                    ),
                ),
                _ => panic!("TERM-fork fixture owner contract is partial"),
            };
        run_process(
            WORKSPACE_TEST_LABEL,
            "/bin/sh",
            [
                "-c",
                concat!(
                    "printf '%s\\n' \"$$\" > \"$ROOT_PID_FILE\"; ",
                    "\"$TEST_EXECUTABLE\" --ignored --exact ",
                    "\"$DETACHED_WORKSPACE_SHARD_TEST\" --nocapture --test-threads=1 ",
                    "</dev/null >/dev/null 2>&1 & wait",
                ),
            ],
            cancellation,
            jobs,
            &[
                ("ROOT_PID_FILE", self.root_pid_file.as_os_str()),
                (fixture_environment, std::ffi::OsStr::new(&fixture_contract)),
                ("TEST_EXECUTABLE", self.test_executable.as_os_str()),
                (
                    "DETACHED_WORKSPACE_SHARD_TEST",
                    std::ffi::OsStr::new(fixture_test),
                ),
            ],
        )
    }

    fn structural_preflight(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<BatteryRun, RunError> {
        Ok(if cancellation.is_cancelled() {
            BatteryRun::Cancelled
        } else {
            BatteryRun::Admitted(Vec::new())
        })
    }

    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("battery crossed the primary failure")
    }

    fn accept_battery_state(&self) -> Result<(), String> {
        panic!("state acceptance crossed the primary failure")
    }

    fn publish_full_verifier_evidence(&self) -> Result<(), String> {
        panic!("evidence publication crossed the primary failure")
    }

    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed the primary failure")
    }
}

#[cfg(unix)]
pub(super) fn fixture_pid_file(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-verify-{label}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
    ))
}

#[cfg(unix)]
#[cfg(unix)]
fn supervised_sleep(
    pid_file: &std::path::Path,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
) -> Result<CommandRun, RunError> {
    run_process(
        FORMAT_LABEL,
        "/bin/sh",
        [
            "-c",
            "sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PID_FILE\"; wait",
        ],
        cancellation,
        cargo_jobs,
        &[("PID_FILE", pid_file.as_os_str())],
    )
}

#[cfg(unix)]
pub(super) fn wait_for_path(path: &std::path::Path) {
    let started = Instant::now();
    while !path.is_file() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "fixture did not start"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
pub(super) fn read_pids(path: &std::path::Path) -> Vec<u32> {
    fs::read_to_string(path)
        .expect("read verifier fixture pids")
        .split_whitespace()
        .map(|pid| pid.parse().expect("fixture pid is numeric"))
        .collect()
}

#[cfg(unix)]
pub(super) fn assert_processes_gone(path: &std::path::Path, label: &str) {
    let pids = read_pids(path);
    let started = Instant::now();
    loop {
        let live = pids
            .iter()
            .copied()
            .filter(|pid| process_exists(*pid))
            .collect::<Vec<_>>();
        if live.is_empty() {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{label} processes {live:?} remained after the bounded post-reap observation"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
pub(super) fn assert_process_stops_within(pid: u32, label: &str, deadline: Duration) {
    let started = Instant::now();
    while process_is_running(pid) {
        assert!(
            started.elapsed() < deadline,
            "{label} process {pid} remained alive after the {deadline:?} bounded observation"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
pub(super) fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && !output.stdout.trim_ascii_start().starts_with(b"Z")
                && !output.stdout.trim_ascii().is_empty()
        })
}
