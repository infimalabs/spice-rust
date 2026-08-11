struct CleanupFaultRunner {
    sibling_started: AtomicBool,
}

struct WorkerPanicRunner;

impl Runner for WorkerPanicRunner {
    fn entry_point(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn format(&self, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        panic!("fixture verifier worker panic")
    }
    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        panic!("a successor crossed a panicked prerequisite")
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
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(BatteryRun::Cancelled)
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
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("battery crossed a failed prerequisite")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed a failed prerequisite")
    }
}

#[cfg(unix)]
struct InterruptRunner {
    pid_file: PathBuf,
}

#[cfg(unix)]
impl InterruptRunner {
    fn new() -> Self {
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
    fn run(&self, _: Step, _: &CancellationToken, _: usize) -> Result<CommandRun, RunError> {
        panic!("a successor crossed interruption")
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
    fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
        while !cancellation.is_cancelled() {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(BatteryRun::Cancelled)
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed interruption")
    }
}

#[cfg(unix)]
struct ScopedInterruptRunner {
    pid_file: PathBuf,
}

#[cfg(unix)]
impl ScopedInterruptRunner {
    fn new() -> Self {
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
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        unreachable!("full route is not under test")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        unreachable!("full route is not under test")
    }
}

#[cfg(unix)]
struct FailureRunner {
    pid_file: PathBuf,
}

#[cfg(unix)]
impl FailureRunner {
    fn new() -> Self {
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
    fn workspace_tests(
        &self,
        cancellation: &CancellationToken,
        jobs: usize,
        _: usize,
    ) -> Result<CommandRun, RunError> {
        supervised_sleep(&self.pid_file, cancellation, jobs)
    }
    fn battery(&self, _: &CancellationToken) -> Result<BatteryRun, RunError> {
        panic!("battery crossed the primary failure")
    }
    fn landing(&self) -> Result<landing::Report, landing::Error> {
        panic!("landing crossed the primary failure")
    }
}

#[cfg(unix)]
fn fixture_pid_file(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-verify-{label}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
    ))
}

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
fn wait_for_path(path: &std::path::Path) {
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
fn read_pids(path: &std::path::Path) -> Vec<u32> {
    fs::read_to_string(path)
        .expect("read verifier fixture pids")
        .split_whitespace()
        .map(|pid| pid.parse().expect("fixture pid is numeric"))
        .collect()
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
}
