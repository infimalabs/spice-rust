mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        BATTERY_LABEL, BROWSER_LABEL, BatteryRun, BrowserRun, Budget, CancellationToken,
        CommandRun, ENTRY_POINT_LABEL, FORMAT_LABEL, PREREQUISITES, ProcessOutput, RunError,
        Runner, Status, VERIFY_ACTIONS, VerifyError, count_peer_cargo, execute_action, ready_nodes,
        run_process, success, verify_package_with_control, verify_with_control,
    };
    use crate::verify_timing::Disposition;
    use crate::{ScopedCommand, Step, landing};

    const FIXTURE_CPU_COUNT: usize = 16;
    const PERCENT_SCALE: u64 = 100;
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct TimedRunner {
        delays: BTreeMap<&'static str, Duration>,
        calls: Mutex<Vec<&'static str>>,
        active: AtomicUsize,
        peak: AtomicUsize,
        work_micros: AtomicU64,
    }

    impl TimedRunner {
        fn benchmark() -> Self {
            let mut delays = BTreeMap::new();
            delays.insert(ENTRY_POINT_LABEL, Duration::from_millis(5));
            delays.insert(FORMAT_LABEL, Duration::from_millis(25));
            for action in &VERIFY_ACTIONS[2..=4] {
                delays.insert(action.label(), Duration::from_millis(35));
            }
            delays.insert(BROWSER_LABEL, Duration::from_millis(100));
            for action in &VERIFY_ACTIONS[6..=8] {
                delays.insert(action.label(), Duration::from_millis(35));
            }
            delays.insert(BATTERY_LABEL, Duration::from_millis(160));
            delays.insert(landing::LABEL, Duration::from_millis(5));
            Self::new(delays)
        }

        fn uniform(delay: Duration) -> Self {
            Self::new(
                VERIFY_ACTIONS
                    .iter()
                    .map(|action| (action.label(), delay))
                    .collect(),
            )
        }

        fn new(delays: BTreeMap<&'static str, Duration>) -> Self {
            Self {
                delays,
                calls: Mutex::new(Vec::new()),
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                work_micros: AtomicU64::new(0),
            }
        }

        fn perform(&self, label: &'static str, cancellation: Option<&CancellationToken>) -> bool {
            self.calls.lock().expect("timed calls lock").push(label);
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.peak.fetch_max(active, Ordering::AcqRel);
            let delay = self
                .delays
                .get(label)
                .copied()
                .unwrap_or_else(|| panic!("timed fixture omitted action {label:?}"));
            self.work_micros
                .fetch_add(delay.as_micros() as u64, Ordering::AcqRel);
            let started = Instant::now();
            while started.elapsed() < delay {
                if cancellation.is_some_and(CancellationToken::is_cancelled) {
                    self.active.fetch_sub(1, Ordering::AcqRel);
                    return false;
                }
                thread::sleep(Duration::from_millis(1));
            }
            self.active.fetch_sub(1, Ordering::AcqRel);
            true
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().expect("timed calls lock").clone()
        }
    }

    impl Runner for TimedRunner {
        fn entry_point(&self) -> Result<String, String> {
            self.perform(ENTRY_POINT_LABEL, None);
            Ok("fixture entry point\n".to_owned())
        }

        fn format(
            &self,
            cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            Ok(timed_command(
                self.perform(FORMAT_LABEL, Some(cancellation)),
            ))
        }

        fn run(
            &self,
            step: Step,
            cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            Ok(timed_command(self.perform(step.label, Some(cancellation))))
        }

        fn run_scoped(
            &self,
            step: ScopedCommand,
            package: &str,
            cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            let label = step.label(package);
            let _ = label;
            Ok(timed_command(!cancellation.is_cancelled()))
        }

        fn browser(
            &self,
            cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
            Ok(BrowserRun {
                process: timed_command(self.perform(BROWSER_LABEL, Some(cancellation))),
                completed: true,
            })
        }

        fn battery(&self, cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
            if self.perform(BATTERY_LABEL, Some(cancellation)) {
                Ok(BatteryRun::Admitted(br#"{"admitted":true}"#.to_vec()))
            } else {
                Ok(BatteryRun::Cancelled)
            }
        }

        fn landing(&self) -> Result<landing::Report, landing::Error> {
            self.perform(landing::LABEL, None);
            Ok(landing::fixture_report(true))
        }
    }

    fn timed_command(completed: bool) -> CommandRun {
        let output = ProcessOutput::status_only(Status {
            success: completed,
            code: completed.then_some(0),
        });
        if completed {
            CommandRun::Exited(output)
        } else {
            CommandRun::Cancelled(output)
        }
    }

    #[test]
    fn graph_has_only_backward_edges_and_terminal_landing_depends_on_every_action() {
        for (index, prerequisites) in PREREQUISITES.iter().enumerate() {
            assert!(prerequisites.iter().all(|predecessor| *predecessor < index));
        }
        assert_eq!(
            PREREQUISITES.last(),
            Some(&&[1, 2, 3, 4, 5, 6, 7, 8, 9][..])
        );
    }

    #[test]
    fn readiness_never_crosses_an_unfinished_declared_prerequisite() {
        let mut nodes = (0..VERIFY_ACTIONS.len())
            .map(|_| super::NodeState::Pending)
            .collect::<Vec<_>>();
        assert_eq!(ready_nodes(&nodes), [0]);

        nodes[0] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
        assert_eq!(ready_nodes(&nodes), [1, 9]);
        nodes[9] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
        assert_eq!(ready_nodes(&nodes), [1]);

        for index in 1..9 {
            let ready = ready_nodes(&nodes);
            assert!(
                ready.contains(&index),
                "action {index} was not ready after its prerequisites"
            );
            assert!(
                !ready.contains(&10),
                "landing crossed unfinished action {index}"
            );
            nodes[index] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
        }
        assert_eq!(ready_nodes(&nodes), [10]);
    }

    #[test]
    fn peer_cargo_pressure_collapses_the_small_worker_budget() {
        let no_pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(0));
        let one_peer = Budget::from_observation(FIXTURE_CPU_COUNT, Some(1));
        let pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(8));
        let two_cpus = Budget::from_observation(2, Some(0));
        let one_free_cpu = Budget::from_observation(2, Some(1));
        let saturated_cpu = Budget::from_observation(1, Some(4));
        let unknown = Budget::from_observation(FIXTURE_CPU_COUNT, None);
        assert_eq!(no_pressure.workers, 3);
        assert_eq!(one_peer.workers, 2);
        assert_eq!(pressure.workers, 1);
        assert_eq!(two_cpus.workers, 2);
        assert_eq!(one_free_cpu.workers, 1);
        assert_eq!(saturated_cpu.workers, 1);
        assert_eq!(unknown.workers, 1);
        assert_eq!(unknown.cargo_jobs, 1);
        assert!(no_pressure.cargo_jobs <= FIXTURE_CPU_COUNT / no_pressure.workers);
        assert_eq!(pressure.cargo_jobs, 1);
    }

    #[test]
    fn cargo_probe_excludes_the_current_cargo_ancestor() {
        let listing = concat!(
            "10 1 cargo\n",
            "20 10 xtask\n",
            "30 1 /toolchains/bin/cargo\n",
            "31 30 rustc\n",
        );
        assert_eq!(count_peer_cargo(listing, 20), 1);
    }

    #[test]
    fn single_worker_mode_executes_every_action_once_without_overlap() {
        let runner = TimedRunner::uniform(Duration::from_millis(2));
        let mut output = Vec::new();
        verify_with_control(
            &runner,
            &mut output,
            Budget::fixture(1, 1, 8),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("single-worker graph succeeds");

        assert_eq!(runner.peak.load(Ordering::Acquire), 1);
        assert_exactly_once(&runner.calls());
        assert!(
            String::from_utf8(output)
                .expect("scheduler output is UTF-8")
                .contains("scheduler workers=1 cargo_jobs=1 active_peer_cargo_jobs=8")
        );
    }

    #[test]
    fn deterministic_terminals_survive_out_of_order_completion() {
        let runner = TimedRunner::benchmark();
        let mut output = Vec::new();
        verify_with_control(
            &runner,
            &mut output,
            Budget::fixture(3, 2, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("concurrent graph succeeds");

        let output = String::from_utf8(output).expect("scheduler output is UTF-8");
        let terminal_ordinals = output
            .lines()
            .filter(|line| line.contains("stage=terminal"))
            .map(|line| {
                line.split("ordinal=")
                    .nth(1)
                    .and_then(|tail| tail.split('/').next())
                    .and_then(|ordinal| ordinal.parse::<usize>().ok())
                    .expect("terminal has an ordinal")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal_ordinals,
            (1..=VERIFY_ACTIONS.len()).collect::<Vec<_>>()
        );
        assert_exactly_once(&runner.calls());
        assert!(runner.peak.load(Ordering::Acquire) >= 2);
    }

    #[test]
    fn controlled_warm_benchmark_cuts_wall_time_without_more_cpu_work() {
        let serial = TimedRunner::benchmark();
        let serial_started = Instant::now();
        let serial_cancellation = CancellationToken::new();
        for action in VERIFY_ACTIONS {
            execute_action(&serial, action, &serial_cancellation, 2)
                .result
                .expect("serial fixture action succeeds");
        }
        let serial_wall = serial_started.elapsed();

        let concurrent = TimedRunner::benchmark();
        let concurrent_started = Instant::now();
        verify_with_control(
            &concurrent,
            &mut Vec::new(),
            Budget::fixture(3, 2, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("concurrent warm fixture succeeds");
        let concurrent_wall = concurrent_started.elapsed();

        assert!(
            concurrent_wall.as_nanos() * u128::from(PERCENT_SCALE) <= serial_wall.as_nanos() * 75,
            concat!(
                "warm scheduler reduction was below 25%: ",
                "serial={serial_wall:?} concurrent={concurrent_wall:?}"
            ),
            serial_wall = serial_wall,
            concurrent_wall = concurrent_wall,
        );
        let serial_cpu = serial.work_micros.load(Ordering::Acquire);
        let concurrent_cpu = concurrent.work_micros.load(Ordering::Acquire);
        assert!(
            concurrent_cpu * PERCENT_SCALE <= serial_cpu * 115,
            "fixture CPU work grew over 15%: serial={serial_cpu} concurrent={concurrent_cpu}"
        );
        assert_exactly_once(&serial.calls());
        assert_exactly_once(&concurrent.calls());
    }

    #[cfg(unix)]
    #[test]
    fn operator_interruption_cancels_and_reaps_the_whole_sibling_process_group() {
        let fixture = InterruptRunner::new();
        let cancellation = CancellationToken::new();
        let interrupted = Arc::new(AtomicBool::new(false));
        let controller_token = cancellation.clone();
        let controller_interrupted = Arc::clone(&interrupted);
        let pid_file = fixture.pid_file.clone();
        let controller = thread::spawn(move || {
            wait_for_path(&pid_file);
            controller_interrupted.store(true, Ordering::Release);
            controller_token.cancel();
        });

        let mut output = Vec::new();
        let error = verify_with_control(
            &fixture,
            &mut output,
            Budget::fixture(3, 1, 0),
            cancellation,
            interrupted,
        )
        .expect_err("operator interruption refuses verification");
        controller.join().expect("interrupt controller joins");
        assert!(matches!(error, VerifyError::Interrupted));

        let pids = read_pids(&fixture.pid_file);
        for pid in pids {
            assert!(
                !process_exists(pid),
                "cancelled verifier child {pid} leaked"
            );
        }
        let output = String::from_utf8(output).expect("scheduler output is UTF-8");
        assert!(output.contains("disposition=cancelled"));
        assert!(output.contains("primary_ordinal=operator interrupted=true"));
        fs::remove_file(&fixture.pid_file).expect("remove interrupt fixture pid file");
    }

    #[cfg(unix)]
    #[test]
    fn first_failure_cancels_and_reaps_the_whole_sibling_process_group() {
        let fixture = FailureRunner::new();
        let mut output = Vec::new();
        let error = verify_with_control(
            &fixture,
            &mut output,
            Budget::fixture(3, 1, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("battery refusal cancels its running sibling");
        assert!(matches!(error, VerifyError::BatteryRefused));

        let pids = read_pids(&fixture.pid_file);
        for pid in pids {
            assert!(!process_exists(pid), "failed verifier sibling {pid} leaked");
        }
        let output = String::from_utf8(output).expect("scheduler output is UTF-8");
        assert!(output.contains("disposition=cancelled"));
        assert!(output.contains("primary_ordinal=10 interrupted=false"));
        fs::remove_file(&fixture.pid_file).expect("remove failure fixture pid file");
    }

    #[test]
    fn primary_failure_reports_a_deterministic_sibling_cleanup_fault() {
        let runner = CleanupFaultRunner {
            format_started: AtomicBool::new(false),
        };
        let mut output = Vec::new();
        let error = verify_with_control(
            &runner,
            &mut output,
            Budget::fixture(3, 1, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("battery refusal remains primary");

        assert!(matches!(error, VerifyError::BatteryRefused));
        let output = String::from_utf8(output).expect("scheduler output is UTF-8");
        assert!(output.contains("scheduler=cleanup-fault ordinal=2"));
        assert!(output.contains("primary_ordinal=10 interrupted=false cleanup_faults=1"));
    }

    #[test]
    fn worker_panic_is_a_bounded_primary_failure_instead_of_a_scheduler_hang() {
        let runner = WorkerPanicRunner;
        let mut output = Vec::new();
        let error = verify_with_control(
            &runner,
            &mut output,
            Budget::fixture(3, 1, 0),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("worker panic refuses verification");

        assert!(matches!(
            error,
            VerifyError::WorkerPanic {
                label: FORMAT_LABEL,
                ..
            }
        ));
        let output = String::from_utf8(output).expect("scheduler output is UTF-8");
        assert!(output.contains("disposition=panic"));
        assert!(output.contains("primary_ordinal=2 interrupted=false"));
    }

    #[cfg(unix)]
    #[test]
    fn scoped_interruption_reaps_its_isolated_cargo_process_group() {
        let fixture = ScopedInterruptRunner::new();
        let cancellation = CancellationToken::new();
        let controller_token = cancellation.clone();
        let pid_file = fixture.pid_file.clone();
        let controller = thread::spawn(move || {
            wait_for_path(&pid_file);
            controller_token.cancel();
        });

        let error = verify_package_with_control("gate", &fixture, &mut Vec::new(), &cancellation)
            .expect_err("scoped interruption refuses feedback run");
        controller
            .join()
            .expect("scoped interrupt controller joins");
        assert!(matches!(error, VerifyError::Cancelled { .. }));
        for pid in read_pids(&fixture.pid_file) {
            assert!(!process_exists(pid), "scoped verifier child {pid} leaked");
        }
        fs::remove_file(&fixture.pid_file).expect("remove scoped fixture pid file");
    }

    fn assert_exactly_once(calls: &[&'static str]) {
        let mut actual = calls.to_vec();
        actual.sort_unstable();
        let mut expected = VERIFY_ACTIONS
            .iter()
            .map(|action| action.label())
            .collect::<Vec<_>>();
        expected.sort_unstable();
        assert_eq!(actual, expected, "actions were duplicated or omitted");
    }

    struct CleanupFaultRunner {
        format_started: AtomicBool,
    }

    struct WorkerPanicRunner;

    impl Runner for WorkerPanicRunner {
        fn entry_point(&self) -> Result<String, String> {
            Ok(String::new())
        }

        fn format(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            panic!("fixture verifier worker panic")
        }

        fn run(
            &self,
            _step: Step,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            panic!("a successor crossed a panicked prerequisite")
        }

        fn run_scoped(
            &self,
            _step: ScopedCommand,
            _package: &str,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("scoped route is not under test")
        }

        fn browser(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
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

        fn format(
            &self,
            cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            self.format_started.store(true, Ordering::Release);
            while !cancellation.is_cancelled() {
                thread::yield_now();
            }
            Err(RunError::Cleanup("fixture sibling reap fault".to_owned()))
        }

        fn run(
            &self,
            _step: Step,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            panic!("a successor crossed a failed prerequisite")
        }

        fn run_scoped(
            &self,
            _step: ScopedCommand,
            _package: &str,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("scoped route is not under test")
        }

        fn browser(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
            panic!("browser crossed a failed prerequisite")
        }

        fn battery(&self, _cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
            while !self.format_started.load(Ordering::Acquire) {
                thread::yield_now();
            }
            Ok(BatteryRun::Refused(b"fixture refusal\n".to_vec()))
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
                pid_file: std::env::temp_dir().join(format!(
                    "spice-verify-cancel-{}-{}",
                    std::process::id(),
                    NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                )),
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
            cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            supervised_sleep(&self.pid_file, cancellation, cargo_jobs)
        }

        fn run(
            &self,
            _step: Step,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            panic!("a successor crossed interruption")
        }

        fn run_scoped(
            &self,
            _step: ScopedCommand,
            _package: &str,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("scoped route is not under test")
        }

        fn browser(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
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
                pid_file: std::env::temp_dir().join(format!(
                    "spice-verify-scoped-cancel-{}-{}",
                    std::process::id(),
                    NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                )),
            }
        }
    }

    #[cfg(unix)]
    impl Runner for ScopedInterruptRunner {
        fn entry_point(&self) -> Result<String, String> {
            Ok(String::new())
        }

        fn format(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("full route is not under test")
        }

        fn run(
            &self,
            _step: Step,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("full route is not under test")
        }

        fn run_scoped(
            &self,
            _step: ScopedCommand,
            _package: &str,
            cancellation: &CancellationToken,
            cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            supervised_sleep(&self.pid_file, cancellation, cargo_jobs)
        }

        fn browser(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
            unreachable!("full route is not under test")
        }

        fn battery(&self, _cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
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
                pid_file: std::env::temp_dir().join(format!(
                    "spice-verify-failure-{}-{}",
                    std::process::id(),
                    NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                )),
            }
        }
    }

    #[cfg(unix)]
    impl Runner for FailureRunner {
        fn entry_point(&self) -> Result<String, String> {
            Ok(String::new())
        }

        fn format(
            &self,
            cancellation: &CancellationToken,
            cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            supervised_sleep(&self.pid_file, cancellation, cargo_jobs)
        }

        fn run(
            &self,
            _step: Step,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            panic!("a successor crossed the primary failure")
        }

        fn run_scoped(
            &self,
            _step: ScopedCommand,
            _package: &str,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<CommandRun, RunError> {
            unreachable!("scoped route is not under test")
        }

        fn browser(
            &self,
            _cancellation: &CancellationToken,
            _cargo_jobs: usize,
        ) -> Result<BrowserRun, RunError> {
            panic!("browser crossed the primary failure")
        }

        fn battery(&self, _cancellation: &CancellationToken) -> Result<BatteryRun, RunError> {
            wait_for_path(&self.pid_file);
            Ok(BatteryRun::Refused(b"fixture refusal\n".to_vec()))
        }

        fn landing(&self) -> Result<landing::Report, landing::Error> {
            panic!("landing crossed the primary failure")
        }
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
}
