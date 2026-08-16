use std::collections::BTreeMap;
use std::fs;
use std::hint::black_box;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    BATTERY_LABEL, BROWSER_LABEL, BatteryRun, BrowserRun, Budget, CancellationToken, CommandRun,
    ENTRY_POINT_LABEL, FORMAT_LABEL, FULL_VERIFIER_HARD_RUNTIME, FULL_VERIFIER_PROCESS_BOUNDS,
    FULL_VERIFIER_TEST_RUNTIME, PREREQUISITES, ProcessBounds, ProcessOutput, RunError, Runner,
    SignalGuard, Status, StreamingRunError, VERIFY_ACTIONS, VerifyError, count_peer_cargo,
    is_live_progress_record, ready_nodes, run_bounded_process_streaming, run_process, success,
    verify_package_with_control, verify_with_control, verify_with_deadline,
    verify_with_deadline_and_rebalance, write_deferred_supervisor_output,
};
use crate::verify_timing::Disposition;
use crate::{ScopedCommand, Step, WORKSPACE_TEST_LABEL, landing};

const FIXTURE_CPU_COUNT: usize = 16;
const BENCHMARK_CPU_ITERATIONS: u64 = 10_000_000;
const NON_COOPERATIVE_CHILD_TEST: &str =
    "verify_execution::tests::non_cooperative_in_process_verifier_fixture";
const DETACHED_WORKSPACE_SHARD_TEST: &str =
    "verify_execution::tests::detached_workspace_shard_fixture";
const DETACHED_FIXTURE_LIFE_EXTENSION: &str = "life-ms";
const DETACHED_FIXTURE_DEFAULT_LIFE: Duration = Duration::from_secs(600);
const SUPERVISOR_PID_MARKER: &str = "verifier-supervisor-fixture pid=";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn detached_fixture_life(value: Option<&str>) -> Duration {
    let Some(value) = value else {
        return DETACHED_FIXTURE_DEFAULT_LIFE;
    };
    let milliseconds = value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|milliseconds| *milliseconds > 0)
        .unwrap_or_else(|| {
            panic!("detached fixture life must be a positive integer number of milliseconds")
        });
    Duration::from_millis(milliseconds)
}

fn read_detached_fixture_life(identity_path: &std::path::Path) -> Duration {
    let path = identity_path.with_extension(DETACHED_FIXTURE_LIFE_EXTENSION);
    match fs::read_to_string(&path) {
        Ok(value) => detached_fixture_life(Some(&value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => detached_fixture_life(None),
        Err(error) => panic!("detached fixture life {path:?} cannot be read: {error}"),
    }
}

#[cfg(unix)]
fn publish_detached_fixture_identity(identity_path: &std::path::Path, identity: &str) {
    let destination = atomic::Destination::new(identity_path)
        .expect("resolve detached fixture identity destination");
    let mut staged = destination
        .stage()
        .expect("stage detached fixture identity");
    staged
        .write_all(identity.as_bytes())
        .expect("write detached fixture identity");
    staged
        .commit()
        .expect("publish detached fixture identity atomically");
}

struct TimedRunner {
    delays: BTreeMap<&'static str, Duration>,
    prebuild_delay: Duration,
    calls: Mutex<Vec<&'static str>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    cpu_iterations: u64,
}

struct RefusingOutput;

impl std::io::Write for RefusingOutput {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "fixture output sink closed",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimedRunner {
    fn benchmark() -> Self {
        let mut delays = BTreeMap::new();
        delays.insert(VERIFY_ACTIONS[0].label(), Duration::from_millis(10));
        delays.insert(ENTRY_POINT_LABEL, Duration::from_millis(5));
        delays.insert(FORMAT_LABEL, Duration::from_millis(5));
        for action in &VERIFY_ACTIONS[2..=6] {
            delays.insert(action.label(), Duration::from_millis(10));
        }
        // Keep the two parallel critical-path actions far enough above
        // process-launch noise that the 25% reduction remains observable
        // while the complete workspace test graph is saturated.
        delays.insert(BROWSER_LABEL, Duration::from_millis(600));
        delays.insert(VERIFY_ACTIONS[8].label(), Duration::from_millis(600));
        for action in &VERIFY_ACTIONS[9..=10] {
            delays.insert(action.label(), Duration::from_millis(10));
        }
        delays.insert(BATTERY_LABEL, Duration::from_millis(10));
        delays.insert(landing::LABEL, Duration::from_millis(5));
        Self::new(delays, BENCHMARK_CPU_ITERATIONS)
    }

    fn uniform(delay: Duration) -> Self {
        Self::new(
            VERIFY_ACTIONS
                .iter()
                .map(|action| (action.label(), delay))
                .collect(),
            0,
        )
    }

    fn new(delays: BTreeMap<&'static str, Duration>, cpu_iterations: u64) -> Self {
        Self {
            delays,
            prebuild_delay: Duration::ZERO,
            calls: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            cpu_iterations,
        }
    }

    fn with_prebuild_delay(mut self, delay: Duration) -> Self {
        self.prebuild_delay = delay;
        self
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
        let mut value = u64::try_from(label.len()).expect("small fixture label");
        for index in 0..self.cpu_iterations {
            value = value.rotate_left(7).wrapping_add(index ^ 0x9e37_79b9);
        }
        black_box(value);
        thread::sleep(delay);
        self.active.fetch_sub(1, Ordering::AcqRel);
        !cancellation.is_some_and(CancellationToken::is_cancelled)
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

    fn workspace_test_prebuild(
        &self,
        cancellation: &CancellationToken,
        _cargo_jobs: usize,
    ) -> Result<CommandRun, RunError> {
        thread::sleep(self.prebuild_delay);
        Ok(timed_command(!cancellation.is_cancelled()))
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
        Some(&&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11][..])
    );
}

#[test]
fn readiness_never_crosses_an_unfinished_declared_prerequisite() {
    let mut nodes = (0..VERIFY_ACTIONS.len())
        .map(|_| super::NodeState::Pending)
        .collect::<Vec<_>>();
    assert_eq!(ready_nodes(&nodes), [0]);

    nodes[0] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [1]);

    for index in 1..=2 {
        assert_eq!(ready_nodes(&nodes), [index]);
        nodes[index] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    }

    assert_eq!(ready_nodes(&nodes), [3]);
    nodes[3] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [4]);
    nodes[4] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [5]);
    nodes[5] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [6]);
    nodes[6] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(
        ready_nodes(&nodes),
        [8, 7],
        "the longer workspace-test and documentation path launches first"
    );
    nodes[7] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [8]);
    nodes[8] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [9, 11]);
    nodes[11] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [9]);
    nodes[9] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [10]);
    nodes[10] = super::NodeState::Complete(success(Vec::new(), Disposition::Success));
    assert_eq!(ready_nodes(&nodes), [12]);
}

#[test]
fn peer_cargo_pressure_collapses_outer_and_workspace_worker_budgets() {
    let no_pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(0));
    let one_peer = Budget::from_observation(FIXTURE_CPU_COUNT, Some(1));
    let two_peers = Budget::from_observation(FIXTURE_CPU_COUNT, Some(2));
    let pressure = Budget::from_observation(FIXTURE_CPU_COUNT, Some(8));
    let one_peer_twelve_cpus = Budget::from_observation(12, Some(1));
    let three_peers = Budget::from_observation(12, Some(3));
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
    assert_eq!(no_pressure.workspace_workers, 4);
    assert_eq!(one_peer.workspace_workers, 2);
    assert_eq!(two_peers.workspace_workers, 1);
    assert_eq!(pressure.workspace_workers, 1);
    assert_eq!(one_peer_twelve_cpus.cargo_jobs, 4);
    assert_eq!(one_peer_twelve_cpus.workspace_workers, 2);
    assert_eq!(three_peers.cargo_jobs, 3);
    assert_eq!(three_peers.workspace_workers, 1);
    assert_eq!(two_cpus.workspace_workers, 1);
    assert_eq!(one_free_cpu.workspace_workers, 1);
    assert_eq!(saturated_cpu.workspace_workers, 1);
    assert_eq!(unknown.workspace_workers, 1);
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
            .contains(concat!(
                "scheduler workers=1 cargo_jobs=1 active_peer_cargo_jobs=8 ",
                "workspace_workers=1 action_count=13 ",
                "deadline_scope=test-phase deadline_ms=180000"
            ))
    );
}

#[test]
fn zero_peer_budget_excludes_cold_setup_and_completes_test_phase_branches() {
    let mut delays = VERIFY_ACTIONS
        .iter()
        .map(|action| (action.label(), Duration::from_millis(10)))
        .collect::<BTreeMap<_, _>>();
    delays.insert(VERIFY_ACTIONS[2].label(), Duration::from_millis(470));
    delays.insert(VERIFY_ACTIONS[3].label(), Duration::from_millis(470));
    delays.insert(VERIFY_ACTIONS[4].label(), Duration::from_millis(470));
    delays.insert(VERIFY_ACTIONS[5].label(), Duration::from_millis(500));
    delays.insert(VERIFY_ACTIONS[7].label(), Duration::from_millis(400));
    delays.insert(VERIFY_ACTIONS[8].label(), Duration::from_millis(500));
    delays.insert(VERIFY_ACTIONS[9].label(), Duration::from_millis(40));
    delays.insert(VERIFY_ACTIONS[10].label(), Duration::from_millis(40));
    delays.insert(VERIFY_ACTIONS[11].label(), Duration::from_millis(40));
    let prebuild_delay = Duration::from_millis(1_500);
    let runner = TimedRunner::new(delays, 0).with_prebuild_delay(prebuild_delay);
    let mut output = Vec::new();

    verify_with_deadline(
        &runner,
        &mut output,
        Budget::from_observation(FIXTURE_CPU_COUNT, Some(0)),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(1_800),
    )
    .expect("zero-peer fixture completes its test phase inside the bounded test window");

    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(
        output.contains(
            "scheduler workers=3 cargo_jobs=5 active_peer_cargo_jobs=0 workspace_workers=4"
        )
    );
    assert!(output.contains("deadline_scope=test-phase deadline_ms=1800"));
    assert_line_order(
        &output,
        "stage=start ordinal=6/13",
        "stage=setup identity=\"cargo test --locked --workspace --all-targets --no-run (workspace-test setup)\"",
    );
    let setup_terminal = "stage=setup-terminal";
    assert_line_order(&output, setup_terminal, "stage=start ordinal=8/13");
    assert_line_order(&output, setup_terminal, "stage=start ordinal=9/13");
    assert_terminal_exit_zero(&output, 6, VERIFY_ACTIONS[5].label());
    assert_terminal_exit_zero(&output, 8, BROWSER_LABEL);
    assert_terminal_exit_zero(&output, 9, VERIFY_ACTIONS[8].label());
    assert_terminal_success(&output, 13, VERIFY_ACTIONS[12].label());
    assert_exactly_once(&runner.calls());
}

#[test]
fn transient_peer_pressure_is_rebalanced_after_workspace_test_setup() {
    let runner = TimedRunner::uniform(Duration::from_millis(2))
        .with_prebuild_delay(Duration::from_millis(2));
    let mut output = Vec::new();
    let initial = Budget::from_observation(12, Some(3));
    let recovered = Budget::from_observation(12, Some(0));

    verify_with_deadline_and_rebalance(
        &runner,
        &mut output,
        initial,
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(1),
        |_| recovered,
    )
    .expect("the post-setup observation restores capacity after transient peers exit");

    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(
        output.contains(
            "scheduler workers=1 cargo_jobs=3 active_peer_cargo_jobs=3 workspace_workers=1"
        )
    );
    assert!(output.contains(concat!(
        "scheduler-rebalanced trigger=workspace-test-setup workers=3 cargo_jobs=4 ",
        "active_peer_cargo_jobs=0 workspace_workers=4"
    )));
    assert_line_order(
        &output,
        "stage=setup-terminal",
        "scheduler-rebalanced trigger=workspace-test-setup",
    );
    assert_line_order(
        &output,
        "scheduler-rebalanced trigger=workspace-test-setup",
        "stage=start ordinal=8/13",
    );
    assert!(runner.peak.load(Ordering::Acquire) >= 2);
    assert_exactly_once(&runner.calls());
}

#[test]
fn unreadable_post_setup_pressure_uses_the_single_worker_degraded_mode() {
    let runner = TimedRunner::uniform(Duration::from_millis(2))
        .with_prebuild_delay(Duration::from_millis(2));
    let mut output = Vec::new();
    let initial = Budget::from_observation(12, Some(0));
    let degraded = Budget::from_observation(12, None);

    verify_with_deadline_and_rebalance(
        &runner,
        &mut output,
        initial,
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_secs(1),
        |_| degraded,
    )
    .expect("the fail-closed post-setup budget still completes every action");

    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(output.contains(concat!(
        "scheduler-rebalanced trigger=workspace-test-setup workers=1 cargo_jobs=1 ",
        "active_peer_cargo_jobs=unknown workspace_workers=1"
    )));
    assert_eq!(runner.peak.load(Ordering::Acquire), 1);
    assert_exactly_once(&runner.calls());
}

#[test]
fn end_to_end_deadline_cancels_the_graph_before_another_action_starts() {
    let runner = TimedRunner::uniform(Duration::from_millis(10));
    let mut output = Vec::new();
    let error = verify_with_deadline(
        &runner,
        &mut output,
        Budget::fixture(1, 1, 0),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(1),
    )
    .expect_err("the complete verifier deadline refuses further work");

    assert!(matches!(error, VerifyError::TimedOut { .. }));
    let expected = VERIFY_ACTIONS[..=6]
        .iter()
        .chain(&VERIFY_ACTIONS[8..=8])
        .map(|action| action.label())
        .collect::<Vec<_>>();
    assert_eq!(runner.calls(), expected);
    let output = String::from_utf8(output).expect("deadline output is UTF-8");
    assert!(output.contains("deadline_scope=test-phase deadline_ms=1"));
    assert!(output.contains("primary_ordinal=deadline"));
}

#[test]
fn completed_test_branches_release_the_terminal_tail_to_the_supervisor_envelope() {
    let mut delays = VERIFY_ACTIONS
        .iter()
        .map(|action| (action.label(), Duration::from_millis(2)))
        .collect::<BTreeMap<_, _>>();
    delays.insert(BROWSER_LABEL, Duration::from_millis(80));
    delays.insert(VERIFY_ACTIONS[8].label(), Duration::from_millis(120));
    delays.insert(VERIFY_ACTIONS[9].label(), Duration::from_millis(50));
    delays.insert(VERIFY_ACTIONS[10].label(), Duration::from_millis(50));
    delays.insert(BATTERY_LABEL, Duration::from_millis(50));
    let runner = TimedRunner::new(delays, 0).with_prebuild_delay(Duration::from_millis(2));
    let mut output = Vec::new();
    let started = Instant::now();

    verify_with_deadline(
        &runner,
        &mut output,
        Budget::fixture(3, 1, 0),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(180),
    )
    .expect("completed test branches leave their bounded terminal tail runnable");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the compressed terminal tail must remain inside its supervisor analogue"
    );
    let calls = runner.calls();
    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert_line_order(
        &output,
        "stage=start ordinal=9/13",
        "stage=start ordinal=8/13",
    );
    assert_terminal_exit_zero(&output, 9, VERIFY_ACTIONS[8].label());
    assert_terminal_success(&output, 13, VERIFY_ACTIONS[12].label());
    assert_exactly_once(&calls);
}

#[test]
fn outer_supervisor_reserves_every_terminal_phase_inside_five_minutes() {
    let bounds = FULL_VERIFIER_PROCESS_BOUNDS;
    let hard_envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
    assert_eq!(hard_envelope, Duration::from_secs(299));
    assert_eq!(bounds.runtime, Duration::from_secs(296));
    assert!(hard_envelope < FULL_VERIFIER_HARD_RUNTIME);
    assert_eq!(
        bounds.output_limit,
        super::OUTPUT_LIMIT * VERIFY_ACTIONS.len() * 2,
        "the outer capture retains both per-action stream bounds"
    );
    assert!(
        bounds.runtime > FULL_VERIFIER_TEST_RUNTIME,
        "the hard supervisor must outlive the child-owned deadline"
    );
}

#[test]
fn live_progress_selection_is_narrow_and_deferred_output_is_byte_complete() {
    let scheduler = b"[verify] scheduler workers=3\n";
    let terminal = b"[verify] stage=terminal ordinal=13/13\n";
    let action = b"compiler output\n";
    let final_record = b"final action bytes";
    assert!(is_live_progress_record(scheduler));
    assert!(is_live_progress_record(terminal));
    assert!(!is_live_progress_record(action));
    assert!(!is_live_progress_record(
        &[b"[verify] ".as_slice(), &vec![b'x'; 8 * 1024]].concat()
    ));

    let captured = [
        scheduler.as_slice(),
        action.as_slice(),
        terminal.as_slice(),
        final_record.as_slice(),
    ]
    .concat();
    let mut deferred = Vec::new();
    write_deferred_supervisor_output(&mut deferred, &captured)
        .expect("deferred supervisor output writes");
    assert_eq!(
        deferred,
        [action.as_slice(), final_record.as_slice()].concat()
    );
}

#[cfg(unix)]
#[test]
fn hard_supervisor_reaps_a_non_cooperative_in_process_verifier() {
    let bounds = ProcessBounds {
        runtime: Duration::from_millis(200),
        grace: Duration::from_millis(100),
        output_drain: Duration::from_millis(100),
        reap: Duration::from_millis(100),
        output_limit: super::OUTPUT_LIMIT,
    };
    let executable = std::env::current_exe().expect("resolve the xtask unit-test process");
    let started = Instant::now();
    let mut live = Vec::new();
    let run = run_bounded_process_streaming(
        "non-cooperative verifier fixture",
        executable,
        benchmark_arguments(NON_COOPERATIVE_CHILD_TEST),
        &std::env::current_dir().expect("resolve verifier fixture cwd"),
        &CancellationToken::new(),
        None,
        &[],
        bounds,
        &mut live,
    )
    .expect("supervise the non-cooperative verifier fixture");
    let elapsed = started.elapsed();
    let CommandRun::TimedOut(output) = run else {
        panic!("non-cooperative verifier did not reach the hard timeout: {run:?}");
    };
    let stdout = String::from_utf8(output.stdout).expect("fixture stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("fixture stderr is UTF-8");
    let transcript = format!("{stdout}{stderr}");
    assert_eq!(
        String::from_utf8(live).expect("live fixture output is UTF-8"),
        "[verify] stage=progress ordinal=1/13 fixture=non-cooperative\n"
    );
    let pid = transcript
        .lines()
        .find_map(|line| {
            line.find(SUPERVISOR_PID_MARKER)
                .map(|offset| &line[offset + SUPERVISOR_PID_MARKER.len()..])
        })
        .and_then(|pid| pid.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("fixture emitted no pid marker: {transcript}"));
    let scaled_envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
    assert!(
        elapsed <= scaled_envelope + Duration::from_secs(1),
        "supervisor exceeded the scaled hard envelope: elapsed={elapsed:?} envelope={scaled_envelope:?}"
    );
    assert!(
        !process_exists(pid),
        "supervisor returned while owned verifier process {pid} was still alive"
    );
}

#[cfg(unix)]
#[test]
fn streaming_outer_supervisor_preserves_interruption_and_group_cleanup() {
    let bounds = ProcessBounds {
        runtime: Duration::from_secs(10),
        grace: Duration::from_millis(100),
        output_drain: Duration::from_millis(100),
        reap: Duration::from_millis(100),
        output_limit: super::OUTPUT_LIMIT,
    };
    let pid_file = fixture_pid_file("streaming-supervisor-cancel");
    let cancellation = CancellationToken::new();
    let controller_token = cancellation.clone();
    let controller_pid_file = pid_file.clone();
    let controller = thread::spawn(move || {
        wait_for_path(&controller_pid_file);
        controller_token.cancel();
    });
    let mut live = Vec::new();
    let started = Instant::now();
    let run = run_bounded_process_streaming(
        "streaming cancellation fixture",
        "/bin/sh",
        [
            "-c",
            concat!(
                "printf '[verify] stage=progress ordinal=1/13 fixture=cancelled\\n'; ",
                "sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PID_FILE\"; wait"
            ),
        ],
        &std::env::current_dir().expect("resolve streaming fixture cwd"),
        &cancellation,
        None,
        &[("PID_FILE", pid_file.as_os_str())],
        bounds,
        &mut live,
    )
    .expect("stream and cancel the verifier fixture");
    controller
        .join()
        .expect("stream cancellation controller joins");
    let CommandRun::Cancelled(output) = run else {
        panic!("streaming fixture did not report cancellation: {run:?}");
    };
    let progress = "[verify] stage=progress ordinal=1/13 fixture=cancelled\n";
    assert_eq!(
        String::from_utf8(live).expect("live output is UTF-8"),
        progress
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("captured output is UTF-8"),
        progress
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "streaming cancellation ignored its token"
    );
    assert_processes_gone(&pid_file, "streaming supervisor cancellation");
    fs::remove_file(pid_file).expect("remove streaming cancellation pid file");
}

#[cfg(unix)]
#[test]
fn live_output_failure_cancels_and_reaps_the_supervised_group() {
    let bounds = ProcessBounds {
        runtime: Duration::from_secs(10),
        grace: Duration::from_millis(100),
        output_drain: Duration::from_millis(100),
        reap: Duration::from_millis(100),
        output_limit: super::OUTPUT_LIMIT,
    };
    let pid_file = fixture_pid_file("streaming-output-failure");
    let error = run_bounded_process_streaming(
        "streaming output failure fixture",
        "/bin/sh",
        [
            "-c",
            concat!(
                "sleep 30 & printf '%s %s\\n' \"$$\" \"$!\" > \"$PID_FILE\"; ",
                "printf '[verify] stage=progress ordinal=1/13 fixture=closed-output\\n'; wait"
            ),
        ],
        &std::env::current_dir().expect("resolve output failure fixture cwd"),
        &CancellationToken::new(),
        None,
        &[("PID_FILE", pid_file.as_os_str())],
        bounds,
        &mut RefusingOutput,
    )
    .expect_err("a closed live output sink refuses the supervisor");
    assert!(matches!(
        error,
        StreamingRunError::Output(error)
            if error.kind() == std::io::ErrorKind::BrokenPipe
    ));
    assert_processes_gone(&pid_file, "streaming output failure");
    fs::remove_file(pid_file).expect("remove streaming output failure pid file");
}

#[cfg(unix)]
#[test]
#[ignore = "launched exactly by the hard-supervisor parent test"]
fn non_cooperative_in_process_verifier_fixture() {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))
        .expect("install fixture signal bridge");
    println!("{SUPERVISOR_PID_MARKER}{}", std::process::id());
    println!("[verify] stage=progress ordinal=1/13 fixture=non-cooperative");
    std::io::stdout().flush().expect("flush fixture pid marker");

    let runner = TimedRunner::uniform(Duration::from_secs(60));
    let _ = verify_with_deadline(
        &runner,
        &mut Vec::new(),
        Budget::fixture(1, 1, 0),
        cancellation,
        interrupted,
        Duration::from_millis(10),
    );
    drop(signal_guard);
}

#[cfg(unix)]
#[test]
#[ignore = "launched exactly by the detached workspace-shard cancellation test"]
#[expect(
    clippy::disallowed_methods,
    reason = "the isolated fixture reads its one injected identity path at the process rim"
)]
fn detached_workspace_shard_fixture() {
    let started = Instant::now();
    let identity_path = std::env::var_os("DETACHED_IDENTITY_FILE")
        .map(PathBuf::from)
        .expect("fixture identity path is present");
    let life = read_detached_fixture_life(&identity_path);
    // SAFETY: this single-threaded fixture process exists only to prove that
    // a workspace-test descendant can leave its action's process group.
    let session = unsafe { libc::setsid() };
    assert_ne!(session, -1, "fixture creates a new session");
    let pid = std::process::id();
    // SAFETY: these queries read the calling fixture's process identities.
    let group = unsafe { libc::getpgrp() };
    // SAFETY: a zero PID asks for the calling fixture's session.
    let observed_session = unsafe { libc::getsid(0) };
    publish_detached_fixture_identity(
        &identity_path,
        &format!("{pid} {group} {observed_session}\n"),
    );
    thread::sleep(life.saturating_sub(started.elapsed()));
}

#[test]
fn detached_fixture_life_has_one_default_and_accepts_milliseconds() {
    assert_eq!(detached_fixture_life(None), DETACHED_FIXTURE_DEFAULT_LIFE);
    assert_eq!(detached_fixture_life(Some("25")), Duration::from_millis(25));
}

#[test]
#[should_panic(
    expected = "detached fixture life must be a positive integer number of milliseconds"
)]
fn detached_fixture_life_rejects_a_malformed_injection() {
    let _ = detached_fixture_life(Some("soon"));
}

#[cfg(unix)]
#[test]
fn detached_workspace_fixture_self_terminates_after_its_parent_exits() {
    let identity_file = fixture_pid_file("detached-self-deadline");
    let life_file = identity_file.with_extension(DETACHED_FIXTURE_LIFE_EXTENSION);
    fs::write(&life_file, "250\n").expect("write injected fixture life");
    let executable = std::env::current_exe().expect("resolve fixture test executable");
    let parent = Command::new("/bin/sh")
        .args([
            "-c",
            concat!(
                "\"$TEST_EXECUTABLE\" --ignored --exact ",
                "\"$DETACHED_WORKSPACE_SHARD_TEST\" --nocapture --test-threads=1 ",
                "</dev/null >/dev/null 2>&1 &"
            ),
        ])
        .env("TEST_EXECUTABLE", executable)
        .env(
            "DETACHED_WORKSPACE_SHARD_TEST",
            DETACHED_WORKSPACE_SHARD_TEST,
        )
        .env("DETACHED_IDENTITY_FILE", &identity_file)
        .status()
        .expect("launch detached fixture through its short-lived parent");
    assert!(parent.success(), "short-lived fixture parent exits cleanly");

    wait_for_path(&identity_file);
    let identity = read_pids(&identity_file);
    assert_eq!(identity.len(), 3, "fixture publishes PID, PGID, and SID");
    assert_process_stops_within(
        identity[0],
        "self-deadlined detached workspace fixture",
        Duration::from_secs(2),
    );
    fs::remove_file(identity_file).expect("remove self-deadline fixture identity");
    fs::remove_file(life_file).expect("remove injected fixture life");
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

    #[rustfmt::skip]
    mod benchmark;

fn benchmark_arguments(test: &str) -> [&str; 5] {
    [
        "--ignored",
        "--exact",
        test,
        "--nocapture",
        "--test-threads=1",
    ]
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

    assert_processes_gone(&fixture.pid_file, "cancelled verifier child");
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
    .expect_err("browser refusal cancels its running sibling");
    assert!(matches!(error, VerifyError::BrowserIncomplete));

    assert_processes_gone(&fixture.pid_file, "failed verifier sibling");
    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(output.contains("disposition=cancelled"));
    assert!(output.contains("primary_ordinal=8 interrupted=false"));
    fs::remove_file(&fixture.pid_file).expect("remove failure fixture pid file");
}

#[cfg(unix)]
#[test]
fn verifier_cancellation_reaps_a_detached_workspace_shard_session() {
    let fixture = DetachedShardRunner::new();
    let mut output = Vec::new();
    let error = verify_with_control(
        &fixture,
        &mut output,
        Budget::fixture(3, 1, 0),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
    )
    .expect_err("browser refusal cancels its detached workspace shard sibling");
    assert!(matches!(error, VerifyError::BrowserIncomplete));

    let root = read_pids(&fixture.root_pid_file);
    let detached = read_pids(&fixture.detached_identity_file);
    assert_eq!(root.len(), 1, "fixture publishes one action root PID");
    assert_eq!(detached.len(), 3, "fixture publishes PID, PGID, and SID");
    assert_ne!(root[0], detached[0], "detached shard is a descendant");
    assert_eq!(detached[0], detached[1], "detached shard leads its group");
    assert_eq!(detached[0], detached[2], "detached shard leads its session");
    assert_processes_gone(&fixture.root_pid_file, "cancelled workspace action root");
    assert_processes_gone(
        &fixture.detached_identity_file,
        "cancelled detached workspace shard",
    );

    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(output.contains("disposition=cancelled"), "{output}");
    assert!(
        output.contains("primary_ordinal=8 interrupted=false cleanup_faults=0"),
        "{output}"
    );
}

#[test]
fn primary_failure_reports_a_deterministic_sibling_cleanup_fault() {
    let runner = CleanupFaultRunner {
        sibling_started: AtomicBool::new(false),
    };
    let mut output = Vec::new();
    let error = verify_with_control(
        &runner,
        &mut output,
        Budget::fixture(3, 1, 0),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
    )
    .expect_err("browser refusal remains primary");

    assert!(matches!(error, VerifyError::BrowserIncomplete));
    let output = String::from_utf8(output).expect("scheduler output is UTF-8");
    assert!(output.contains("scheduler=cleanup-fault ordinal=9"));
    assert!(output.contains("primary_ordinal=8 interrupted=false cleanup_faults=1"));
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
    assert!(output.contains("primary_ordinal=3 interrupted=false"));
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
    assert_processes_gone(&fixture.pid_file, "scoped verifier child");
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

fn assert_terminal_exit_zero(output: &str, ordinal: usize, label: &str) {
    assert_terminal_disposition(output, ordinal, label, "disposition=exit code=0");
}

fn assert_terminal_success(output: &str, ordinal: usize, label: &str) {
    assert_terminal_disposition(output, ordinal, label, "disposition=success");
}

fn assert_terminal_disposition(output: &str, ordinal: usize, label: &str, disposition: &str) {
    let ordinal = format!("stage=terminal ordinal={ordinal}/{}", VERIFY_ACTIONS.len());
    let identity = format!("identity={label:?}");
    assert!(
        output.lines().any(|line| line.contains(&ordinal)
            && line.contains(disposition)
            && line.contains(&identity)),
        "missing terminal record {ordinal} {identity} {disposition}"
    );
}

fn assert_line_order(output: &str, first: &str, second: &str) {
    let first_at = output
        .find(first)
        .unwrap_or_else(|| panic!("output did not contain first marker {first:?}"));
    let second_at = output
        .find(second)
        .unwrap_or_else(|| panic!("output did not contain second marker {second:?}"));
    assert!(
        first_at < second_at,
        "expected {first:?} before {second:?} in verifier output"
    );
}

mod process_fixtures;

use process_fixtures::*;
