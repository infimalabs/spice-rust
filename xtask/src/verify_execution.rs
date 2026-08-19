//! Dependency-aware execution of the full verifier and serial scoped feedback.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment, Observation,
    ObserveError, ObservedRecord, Output as SillOutput, OutputStream, ProcessGroup, RecordObserver,
    Redaction, RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy, WaitStep,
};

#[cfg(test)]
use super::ENTRY_POINT_LABEL;
use super::verify_timing::{self, Conclusion, Disposition, Stage};
use super::{
    Action, ActionProgress, ActionReadiness, BATTERY_LABEL, BROWSER_LABEL, BatteryRun, BrowserRun,
    CANVAS_APP_LABEL, CommandRun, FORMAT_LABEL, ProcessOutput, RunError, Runner, SCOPED_COMMANDS,
    STRUCTURAL_PREFLIGHT_LABEL, Status, VERIFY_ACTIONS, VerifyError, WORKSPACE_TEST_PREBUILD_LABEL,
    valid_package_name,
};

mod action;
mod process;
mod setup;
mod test_phase_lease;

use action::{
    execute_action, execute_battery, execute_process, failure, panic_detail, write_failure_summary,
};
pub(super) use process::ProgressSource;
use process::{
    SignalGuard, StreamingRunError, active_peer_cargo_jobs, run_bounded_process_streaming,
};
pub(super) use process::{run_process, run_process_with_progress};
#[cfg(not(target_arch = "wasm32"))]
pub(super) use process::{run_process_at, run_process_at_with_progress};
use test_phase_lease::{
    Bounds as TestPhaseBounds, Event as TestPhaseEvent, Evidence as TestPhaseEvidence,
    Lease as TestPhaseLease,
};
#[cfg(test)]
use {action::success, process::count_peer_cargo};

/// The explicit full-verifier dependency graph, indexed by report ordinal.
///
/// The current Spice executable is built first and its exact bytes are then
/// checked against the installed receipt before source verification proceeds.
/// The compiler-evidence chain is serial and retains its warm build root.
/// Workspace-test prebuild reuses the native root as soon as native Clippy has
/// published it, overlapping the disjoint Canvas compiler root while retaining
/// the same aggregate worker and Cargo-job caps. After wasm analysis releases
/// its CPU-heavy slot, the final Canvas app build keeps sole Cargo-home access.
/// Its observed post-compiler record releases the native test branch from the
/// completed prebuild while selected-source validation and atomic publication
/// are still in progress. A native failure is held at the branch boundary until
/// that compiler chain terminates, so it
/// cannot mask compiler evidence. Browser verification starts only after Canvas
/// and overlaps the Rust-test branch while performing no Cargo work of its own.
/// Documentation follows the workspace tests so the reusable native Cargo root
/// is not forced into competing documentation and test publication work. The
/// evidence-consuming battery overlaps that documentation tail once browser and
/// workspace tests have completed; contract landing remains the barrier over
/// every action.
/// After formatting admits, the scheduler synchronously projects the capture-
/// time file-shape and complexity evidence before it can launch the compiler
/// chain; this setup gate is not a second report action or source capture.
const PREREQUISITES: [&[usize]; VERIFY_ACTIONS.len()] = [
    &[],
    &[0],
    &[1],
    &[2],
    &[3],
    &[4],
    &[5],
    &[6],
    &[5],
    &[8],
    &[9],
    &[0, 3, 5, 6, 7, 8],
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
];

/// Three is the fleet-safe ceiling for independently ready outer actions.
const MAX_CONCURRENT_ACTIONS: usize = 3;
/// Only workspace-test prebuild and one member of the serial Canvas compiler
/// chain can perform Cargo work concurrently. The third outer worker is
/// reserved for non-Cargo work, so charging every worker a Cargo share leaves
/// CPU capacity idle throughout the cold compiler path.
const MAX_CONCURRENT_CARGO_ACTIONS: usize = 2;
/// The full verifier prebuilds test binaries and the normal Canvas launcher once
/// in the native compiler output root before parallel shard execution, so every
/// coordinator lane can reuse the same selected-root build cache while retaining
/// its own final-publication path.
///
/// Standalone workspace verification may use the whole coordinator cap. The
/// full verifier derives its actual lane count continuously from observed CPUs,
/// live Cargo groups, concurrent outer actions, and each action's Cargo job
/// share. Its completed shared prebuild then caps that measured answer at six.
/// A test branch that produces no new trusted transition for five minutes is
/// stalled. Compiler setup remains outside this inactivity lease; the complete
/// process tree still cannot cross the six-minute absolute supervisor ceiling.
const FULL_VERIFIER_TEST_RUNTIME: Duration = Duration::from_secs(5 * 60);
/// A healthy child emits a trusted scheduler, setup, action, or terminal record
/// at least every 30 seconds. Permit two missed progress intervals before the
/// outer supervisor stops a silent process tree.
const FULL_VERIFIER_PROGRESS_INACTIVITY: Duration = Duration::from_secs(90);
/// Progress may renew the inactivity lease, but it can never carry the complete
/// process tree beyond this wall-time ceiling. Cleanup consumes the final three
/// seconds of the ceiling below.
const FULL_VERIFIER_ABSOLUTE_RUNTIME: Duration = Duration::from_secs(6 * 60);
const FULL_VERIFIER_PROCESS_RUNTIME: Duration = Duration::from_secs(6 * 60 - 3);
const FULL_VERIFIER_TEST_BOUNDS: TestPhaseBounds = TestPhaseBounds {
    inactivity: FULL_VERIFIER_TEST_RUNTIME,
    absolute: FULL_VERIFIER_PROCESS_RUNTIME,
};
const FULL_VERIFIER_LABEL: &str = concat!(
    "full verifier progress lease (five-minute test-phase inactivity; ",
    "six-minute absolute ceiling)"
);
const FULL_VERIFIER_CHILD_COMMAND: &str = "__verify-system";
/// Hard-supervisor cleanup is a last resort after the child-owned soft deadline.
const FULL_VERIFIER_TERMINATION_GRACE: Duration = Duration::from_secs(1);
const FULL_VERIFIER_REAP_DEADLINE: Duration = Duration::from_secs(1);
const FULL_VERIFIER_OUTPUT_DRAIN: Duration = Duration::from_secs(1);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULER_POLL: Duration = Duration::from_millis(50);
const PROCESS_RUNTIME: Duration = Duration::from_secs(30 * 60);
const OUTPUT_DRAIN: Duration = Duration::from_secs(30);
const REAP_DEADLINE: Duration = Duration::from_secs(10);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const FULL_VERIFIER_OUTPUT_LIMIT: usize = OUTPUT_LIMIT * VERIFY_ACTIONS.len() * 2;
/// Match the existing outer capture exactly: observation must not introduce a
/// tighter admitted-output ceiling than the complete supervised stream.
const FULL_VERIFIER_LIVE_FRAME_LIMIT: usize = FULL_VERIFIER_OUTPUT_LIMIT;
/// Only short scheduler records enter this queue. The complete child stream
/// remains independently captured by Sill under `FULL_VERIFIER_OUTPUT_LIMIT`.
const FULL_VERIFIER_LIVE_QUEUE_CAPACITY: usize = VERIFY_ACTIONS.len() * 2;
const FULL_VERIFIER_LIVE_RECORD_LIMIT: usize = 8 * 1024;
const FULL_VERIFIER_LIVE_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
struct ProcessBounds {
    runtime: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
    output_limit: usize,
}

#[derive(Clone, Copy)]
struct ProgressLeaseBounds {
    inactivity: Duration,
    process: ProcessBounds,
}

const ACTION_PROCESS_BOUNDS: ProcessBounds = ProcessBounds {
    runtime: PROCESS_RUNTIME,
    output_drain: OUTPUT_DRAIN,
    reap: REAP_DEADLINE,
    grace: TERMINATION_GRACE,
    output_limit: OUTPUT_LIMIT,
};
const FULL_VERIFIER_PROCESS_BOUNDS: ProcessBounds = ProcessBounds {
    runtime: FULL_VERIFIER_PROCESS_RUNTIME,
    output_drain: FULL_VERIFIER_OUTPUT_DRAIN,
    reap: FULL_VERIFIER_REAP_DEADLINE,
    grace: FULL_VERIFIER_TERMINATION_GRACE,
    output_limit: FULL_VERIFIER_OUTPUT_LIMIT,
};
const FULL_VERIFIER_PROGRESS_BOUNDS: ProgressLeaseBounds = ProgressLeaseBounds {
    inactivity: FULL_VERIFIER_PROGRESS_INACTIVITY,
    process: FULL_VERIFIER_PROCESS_BOUNDS,
};
const _: () = assert!(
    FULL_VERIFIER_PROCESS_BOUNDS.runtime.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.grace.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.reap.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.output_drain.as_secs()
        == FULL_VERIFIER_ABSOLUTE_RUNTIME.as_secs()
);
const _: () =
    assert!(FULL_VERIFIER_PROCESS_BOUNDS.runtime.as_secs() > FULL_VERIFIER_TEST_RUNTIME.as_secs());
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Budget {
    workers: usize,
    cargo_jobs: usize,
    compiler_cargo_jobs: usize,
    workspace_workers: usize,
    active_cargo_jobs: Option<usize>,
}

impl Budget {
    fn detect() -> Self {
        let cpus = thread::available_parallelism().map_or(1, usize::from);
        let active_cargo_jobs = active_peer_cargo_jobs().ok();
        Self::from_observation(cpus, active_cargo_jobs)
    }

    fn from_observation(cpus: usize, active_cargo_jobs: Option<usize>) -> Self {
        let Some(active) = active_cargo_jobs else {
            return Self {
                workers: 1,
                cargo_jobs: 1,
                compiler_cargo_jobs: 1,
                workspace_workers: 1,
                active_cargo_jobs: None,
            };
        };
        let fleet_slots =
            MAX_CONCURRENT_ACTIONS.saturating_sub(active.min(MAX_CONCURRENT_ACTIONS - 1));
        let cpu_slots = cpus.saturating_sub(active).max(1);
        let workers = fleet_slots.min(cpu_slots);
        let cargo_workers = workers.min(MAX_CONCURRENT_CARGO_ACTIONS);
        let participants = active + cargo_workers;
        let cargo_jobs = (cpus / participants.max(1)).max(1);
        // Workspace tests start only after the Canvas process has emitted its
        // post-compiler readiness record. No sibling compiler remains to claim
        // the Cargo-worker reserve at that boundary. Convert the outer slot
        // deliberately reserved for non-Cargo work into one additional lane of
        // already-built test processes; live peer pressure still removes that
        // slot through `workers` before this phase-specific budget is derived.
        let non_cargo_workers = workers.saturating_sub(cargo_workers);
        let workspace_job_cap = cargo_jobs.saturating_add(non_cargo_workers).max(1);
        let workspace_workers = cpu_slots
            .min(workspace_job_cap)
            .clamp(1, super::FULL_VERIFIER_WORKSPACE_TEST_CONCURRENCY_CAP);
        Self {
            workers,
            cargo_jobs,
            compiler_cargo_jobs: cargo_jobs,
            workspace_workers,
            active_cargo_jobs: Some(active),
        }
    }

    /// Once setup has completed, the two longest ready branches are the Cargo
    /// workspace coordinator and the Cargo-free browser suite. When the fresh
    /// observation still grants multiple workspace lanes, retain two outer
    /// workers so those branches cannot consume the fixed test window serially.
    fn after_workspace_prebuild(mut self) -> Self {
        if self.workspace_workers > 1 {
            self.workers = self.workers.max(2);
        }
        self
    }

    #[cfg(test)]
    fn fixture(workers: usize, cargo_jobs: usize, active_cargo_jobs: usize) -> Self {
        Self {
            workers,
            cargo_jobs,
            compiler_cargo_jobs: cargo_jobs,
            workspace_workers: cargo_jobs
                .clamp(1, super::FULL_VERIFIER_WORKSPACE_TEST_CONCURRENCY_CAP),
            active_cargo_jobs: Some(active_cargo_jobs),
        }
    }

    fn with_rebalanced_capacity(self, mut observed: Self) -> Self {
        observed.compiler_cargo_jobs = self.compiler_cargo_jobs;
        observed
    }
}

enum NodeState {
    Pending,
    Running(ProgressState),
    Complete(ActionCompletion),
}

struct ProgressState {
    started: Instant,
    interval: Duration,
    next_report: Duration,
    cursor: Arc<AtomicU64>,
    reported_cursor: u64,
    lease_cursor: u64,
}

impl ProgressState {
    fn new(started: Instant, interval: Duration) -> Self {
        Self {
            started,
            interval,
            next_report: interval,
            cursor: Arc::new(AtomicU64::new(0)),
            reported_cursor: 0,
            lease_cursor: 0,
        }
    }

    fn callback(&self) -> ActionProgress {
        let cursor = Arc::clone(&self.cursor);
        Arc::new(move || {
            cursor.fetch_add(1, Ordering::Relaxed);
        })
    }

    fn due_cursor(&mut self) -> Option<(Duration, Option<u64>)> {
        if self.started.elapsed() < self.next_report {
            return None;
        }
        let elapsed = self.next_report;
        self.next_report += self.interval;
        let cursor = self.cursor.load(Ordering::Relaxed);
        if cursor == self.reported_cursor {
            Some((elapsed, None))
        } else {
            self.reported_cursor = cursor;
            Some((elapsed, Some(cursor)))
        }
    }

    fn take_lease_cursor(&mut self) -> Option<u64> {
        let cursor = self.cursor.load(Ordering::Relaxed);
        if cursor == self.lease_cursor {
            None
        } else {
            self.lease_cursor = cursor;
            Some(cursor)
        }
    }
}

struct ActionCompletion {
    elapsed: Duration,
    output: Vec<u8>,
    disposition: Disposition,
    result: Result<(), VerifyError>,
}

enum WorkerCompletion {
    Action {
        index: usize,
        completion: ActionCompletion,
    },
    CanvasReady {
        index: usize,
    },
    WorkspacePrebuild(ActionCompletion),
}

struct GraphRun {
    nodes: Vec<NodeState>,
    structural_preflight_failure: Option<VerifyError>,
    prebuild_failure: Option<VerifyError>,
    output_fault: Option<io::Error>,
    deadline_exceeded: bool,
}

struct Failures {
    substantive: Vec<(usize, VerifyError)>,
    cleanup: Vec<(usize, VerifyError)>,
}

#[cfg(test)]
pub(super) fn verify_with<R: Runner>(
    runner: &R,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    verify_with_control(
        runner,
        output,
        Budget::fixture(MAX_CONCURRENT_ACTIONS, 1, 0),
        CancellationToken::new(),
        Arc::new(AtomicBool::new(false)),
    )
}

pub(super) fn verify_supervised(output: &mut impl Write) -> Result<(), VerifyError> {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))?;
    let executable = std::env::current_exe().map_err(|source| VerifyError::Launch {
        label: FULL_VERIFIER_LABEL,
        source,
    })?;
    let cwd = std::env::current_dir().map_err(|source| VerifyError::Launch {
        label: FULL_VERIFIER_LABEL,
        source,
    })?;
    let run = run_bounded_process_streaming(
        FULL_VERIFIER_LABEL,
        executable,
        [OsString::from(FULL_VERIFIER_CHILD_COMMAND)],
        &cwd,
        &cancellation,
        None,
        &[],
        FULL_VERIFIER_PROGRESS_BOUNDS,
        output,
    );
    let run = match run {
        Ok(run) => Ok(run),
        Err(StreamingRunError::Process(error)) => Err(error),
        Err(StreamingRunError::Output(error)) => {
            drop(signal_guard);
            return Err(VerifyError::Output(error));
        }
    };
    let result = render_supervised_verifier(run, output, interrupted.load(Ordering::Acquire));
    drop(signal_guard);
    result
}

fn render_supervised_verifier(
    run: Result<CommandRun, RunError>,
    output: &mut impl Write,
    interrupted: bool,
) -> Result<(), VerifyError> {
    let run = run.map_err(|error| match error {
        RunError::Launch(source) => VerifyError::Launch {
            label: FULL_VERIFIER_LABEL,
            source,
        },
        RunError::Cleanup(detail) => VerifyError::Cleanup {
            label: FULL_VERIFIER_LABEL.to_owned(),
            detail,
        },
    })?;
    let child = match run {
        CommandRun::Exited(child) => child,
        CommandRun::Cancelled(child) => {
            write_deferred_supervisor_output(output, &child.stdout)?;
            return Err(VerifyError::Interrupted);
        }
        CommandRun::TimedOut(child) => {
            write_deferred_supervisor_output(output, &child.stdout)?;
            return Err(VerifyError::TimedOut {
                label: FULL_VERIFIER_LABEL.to_owned(),
            });
        }
    };
    write_deferred_supervisor_output(output, &child.stdout)?;
    if interrupted {
        return Err(VerifyError::Interrupted);
    }
    if child.status.success {
        return Ok(());
    }
    Err(VerifyError::Supervised {
        code: child.status.code,
        detail: String::from_utf8_lossy(&child.stderr).trim_end().to_owned(),
    })
}

fn write_deferred_supervisor_output(
    output: &mut impl Write,
    captured: &[u8],
) -> Result<(), VerifyError> {
    for record in captured.split_inclusive(|byte| *byte == b'\n') {
        if !is_live_progress_record(record) {
            output.write_all(record).map_err(VerifyError::Output)?;
        }
    }
    output.flush().map_err(VerifyError::Output)
}

fn is_live_progress_record(record: &[u8]) -> bool {
    record.len() <= FULL_VERIFIER_LIVE_RECORD_LIMIT && record.starts_with(b"[verify] ")
}

fn is_lease_renewal_record(record: &[u8]) -> bool {
    if !is_live_progress_record(record) {
        return false;
    }
    let body = &record[b"[verify] ".len()..];
    [
        b"stage=advance ".as_slice(),
        b"stage=setup-advance ".as_slice(),
        b"stage=start ".as_slice(),
        b"stage=terminal ".as_slice(),
        b"stage=setup ".as_slice(),
        b"stage=setup-terminal ".as_slice(),
        b"scheduler-rebalanced ".as_slice(),
    ]
    .iter()
    .any(|prefix| body.starts_with(prefix))
}

pub(super) fn verify_system(output: &mut impl Write) -> Result<(), VerifyError> {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))?;
    let runner =
        super::SystemRunner::prepare(cancellation.clone()).map_err(VerifyError::Battery)?;
    let announced_source = runner.announced_source().map_err(VerifyError::Battery)?;
    writeln!(output, "[verify] staged-source digest={announced_source}")
        .map_err(VerifyError::Output)?;
    output.flush().map_err(VerifyError::Output)?;
    runner
        .verify_announced_source()
        .map_err(VerifyError::Battery)?;
    let result = verify_with_rebalance(
        &runner,
        output,
        Budget::detect(),
        cancellation,
        interrupted,
        FULL_VERIFIER_TEST_BOUNDS,
        PROGRESS_INTERVAL,
        |_| Budget::detect(),
    );
    drop(signal_guard);
    result
}

#[cfg(test)]
fn verify_with_control<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
) -> Result<(), VerifyError> {
    verify_with_rebalance(
        runner,
        output,
        budget,
        cancellation,
        interrupted,
        FULL_VERIFIER_TEST_BOUNDS,
        PROGRESS_INTERVAL,
        |budget| budget,
    )
}

#[cfg(test)]
fn verify_with_deadline<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
    runtime: Duration,
) -> Result<(), VerifyError> {
    verify_with_rebalance(
        runner,
        output,
        budget,
        cancellation,
        interrupted,
        TestPhaseBounds {
            inactivity: runtime,
            absolute: runtime,
        },
        PROGRESS_INTERVAL,
        |budget| budget,
    )
}

#[cfg(test)]
fn verify_with_test_bounds<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
    bounds: TestPhaseBounds,
    progress_interval: Duration,
) -> Result<(), VerifyError> {
    verify_with_rebalance(
        runner,
        output,
        budget,
        cancellation,
        interrupted,
        bounds,
        progress_interval,
        |budget| budget,
    )
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "the deadline fixture keeps timing policy and rebalance authority explicit"
)]
fn verify_with_deadline_and_rebalance<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
    runtime: Duration,
    progress_interval: Duration,
    rebalance: impl FnOnce(Budget) -> Budget,
) -> Result<(), VerifyError> {
    verify_with_rebalance(
        runner,
        output,
        budget,
        cancellation,
        interrupted,
        TestPhaseBounds {
            inactivity: runtime,
            absolute: runtime,
        },
        progress_interval,
        rebalance,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the verifier control rim keeps supervision and rebalance authority explicit"
)]
fn verify_with_rebalance<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
    test_bounds: TestPhaseBounds,
    progress_interval: Duration,
    rebalance: impl FnOnce(Budget) -> Budget,
) -> Result<(), VerifyError> {
    debug_assert_eq!(VERIFY_ACTIONS.len(), PREREQUISITES.len());
    write_scheduler_start(output, budget, test_bounds)?;

    let GraphRun {
        mut nodes,
        structural_preflight_failure,
        prebuild_failure,
        output_fault,
        deadline_exceeded,
    } = schedule_graph(
        runner,
        output,
        budget,
        &cancellation,
        test_bounds,
        progress_interval,
        |initial| rebalance(initial).after_workspace_prebuild(),
    );
    if let Some(error) = output_fault {
        return Err(VerifyError::Output(error));
    }

    let Failures {
        mut substantive,
        mut cleanup,
    } = render_completions(&mut nodes, output)?;

    if interrupted.load(Ordering::Acquire) {
        write_failure_summary(output, "operator", &cleanup, true)?;
        return Err(VerifyError::Interrupted);
    }
    if let Some(primary) = structural_preflight_failure {
        write_failure_summary(output, "structural-preflight", &cleanup, false)?;
        return Err(primary);
    }
    if let Some(primary) = prebuild_failure {
        write_failure_summary(output, "9", &cleanup, false)?;
        return Err(primary);
    }
    substantive.sort_by_key(|(index, _)| *index);
    cleanup.sort_by_key(|(index, _)| *index);
    if let Some((primary_index, primary)) = substantive.into_iter().next() {
        write_failure_summary(output, &(primary_index + 1).to_string(), &cleanup, false)?;
        return Err(primary);
    }
    if deadline_exceeded {
        write_failure_summary(output, "deadline", &cleanup, false)?;
        return Err(VerifyError::TimedOut {
            label: FULL_VERIFIER_LABEL.to_owned(),
        });
    }
    if !cleanup.is_empty() {
        let (primary_index, primary) = cleanup.remove(0);
        write_failure_summary(output, &(primary_index + 1).to_string(), &cleanup, false)?;
        return Err(primary);
    }
    if cancellation.is_cancelled() {
        return Err(VerifyError::Interrupted);
    }
    runner
        .accept_battery_state()
        .map_err(VerifyError::Battery)?;
    runner
        .publish_full_verifier_evidence()
        .map_err(VerifyError::Battery)?;
    Ok(())
}

fn write_scheduler_start(
    output: &mut impl Write,
    budget: Budget,
    test_bounds: TestPhaseBounds,
) -> Result<(), VerifyError> {
    let active = budget
        .active_cargo_jobs
        .map_or_else(|| "unknown".to_owned(), |count| count.to_string());
    write!(
        output,
        concat!(
            "[verify] scheduler workers={} cargo_jobs={} ",
            "active_peer_cargo_jobs={} workspace_workers={} action_count={} "
        ),
        budget.workers,
        budget.cargo_jobs,
        active,
        budget.workspace_workers,
        VERIFY_ACTIONS.len(),
    )
    .map_err(VerifyError::Output)?;
    write!(
        output,
        "deadline_scope=test-phase inactivity_ms={} absolute_ms={} ",
        test_bounds.inactivity.as_millis(),
        test_bounds.absolute.as_millis(),
    )
    .map_err(VerifyError::Output)?;
    writeln!(
        output,
        "supervisor_inactivity_ms={} supervisor_absolute_ms={}",
        FULL_VERIFIER_PROGRESS_INACTIVITY.as_millis(),
        FULL_VERIFIER_ABSOLUTE_RUNTIME.as_millis(),
    )
    .map_err(VerifyError::Output)?;
    output.flush().map_err(VerifyError::Output)
}

fn schedule_graph<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    mut budget: Budget,
    cancellation: &CancellationToken,
    test_bounds: TestPhaseBounds,
    progress_interval: Duration,
    rebalance: impl FnOnce(Budget) -> Budget,
) -> GraphRun {
    let (sender, receiver) = mpsc::channel::<WorkerCompletion>();
    let mut nodes = (0..VERIFY_ACTIONS.len())
        .map(|_| NodeState::Pending)
        .collect::<Vec<_>>();
    let mut running = 0usize;
    let mut failure_seen = false;
    let mut output_fault = None;
    let mut deadline_exceeded = false;
    let mut test_lease = TestPhaseLease::new(Instant::now(), test_bounds);
    let mut canvas_app_ready = false;
    let mut setups = setup::SetupRun::new(rebalance, progress_interval);

    thread::scope(|scope| {
        loop {
            observe_test_phase_progress(&mut nodes, &mut test_lease);
            enforce_test_phase_deadline(
                &test_lease,
                test_phase_complete(&nodes),
                &mut deadline_exceeded,
                &mut failure_seen,
                cancellation,
                output,
                &mut output_fault,
            );
            if !failure_seen && !cancellation.is_cancelled() {
                if let Some(rebalanced) = setups.advance(
                    scope,
                    runner,
                    &nodes,
                    output,
                    budget,
                    cancellation,
                    &sender,
                    &mut running,
                ) {
                    budget = rebalanced;
                }
                failure_seen = setups.failed();
            }
            if !failure_seen
                && !cancellation.is_cancelled()
                && deferred_workspace_failure_ready(&nodes)
            {
                failure_seen = true;
                cancellation.cancel();
            }
            if !failure_seen && !cancellation.is_cancelled() {
                start_ready_actions(
                    scope,
                    runner,
                    output,
                    budget,
                    cancellation,
                    &sender,
                    &mut nodes,
                    &mut running,
                    &mut failure_seen,
                    &mut output_fault,
                    &mut test_lease,
                    progress_interval,
                    setups.test_branches_ready(),
                    canvas_app_ready,
                );
            } else if cancellation.is_cancelled() {
                failure_seen = true;
            }
            if running == 0 {
                break;
            }
            let completion_state = CompletionState {
                nodes: &mut nodes,
                running: &mut running,
                failure_seen: &mut failure_seen,
                canvas_app_ready: &mut canvas_app_ready,
            };
            if receive_completion(
                &receiver,
                cancellation,
                &mut setups,
                &mut test_lease,
                output,
                completion_state,
            ) {
                break;
            }
            write_due_progress(
                &mut nodes,
                &mut setups,
                &mut test_lease,
                output,
                cancellation,
                &mut failure_seen,
                &mut output_fault,
            );
        }
    });
    finish_graph(nodes, setups, output_fault, deadline_exceeded)
}

fn finish_graph<Rebalance>(
    nodes: Vec<NodeState>,
    setups: setup::SetupRun<Rebalance>,
    output_fault: Option<io::Error>,
    deadline_exceeded: bool,
) -> GraphRun {
    let (structural_preflight_failure, prebuild_failure, setup_fault) = setups.into_failures();
    GraphRun {
        nodes,
        structural_preflight_failure,
        prebuild_failure,
        output_fault: output_fault.or(setup_fault),
        deadline_exceeded,
    }
}

fn enforce_test_phase_deadline(
    lease: &TestPhaseLease,
    complete: bool,
    deadline_exceeded: &mut bool,
    failure_seen: &mut bool,
    cancellation: &CancellationToken,
    output: &mut impl Write,
    output_fault: &mut Option<io::Error>,
) {
    if *failure_seen {
        return;
    }
    let observed = Instant::now();
    let Some(expiry) = lease.expiry(observed, complete) else {
        return;
    };
    if let Some(evidence) = lease.current_evidence(observed)
        && let Err(error) = writeln!(
            output,
            concat!(
                "[verify] test-lease-expired generation={} reason={} last_event={} ",
                "inactivity_remaining_ms={} absolute_remaining_ms={}"
            ),
            evidence.generation,
            expiry,
            evidence.reason,
            evidence.inactivity_remaining.as_millis(),
            evidence.absolute_remaining.as_millis(),
        )
        .and_then(|()| output.flush())
    {
        output_fault.get_or_insert(error);
    }
    *deadline_exceeded = true;
    *failure_seen = true;
    cancellation.cancel();
}

#[expect(
    clippy::too_many_arguments,
    reason = "the launch rim makes every mutable scheduler state transition explicit"
)]
fn start_ready_actions<'scope, 'env: 'scope, R: Runner + 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    runner: &'scope R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<WorkerCompletion>,
    nodes: &mut [NodeState],
    running: &mut usize,
    failure_seen: &mut bool,
    output_fault: &mut Option<io::Error>,
    test_lease: &mut TestPhaseLease,
    progress_interval: Duration,
    test_branches_ready: bool,
    canvas_app_ready: bool,
) {
    for index in ready_nodes(nodes) {
        if *running >= budget.workers {
            break;
        }
        let action = VERIFY_ACTIONS[index];
        if starts_test_phase_deadline(action) && !test_branches_ready {
            continue;
        }
        if matches!(action, Action::WorkspaceTests) && !canvas_app_ready {
            continue;
        }
        if let Err(error) = verify_timing::write_start(output, stage(index, action)) {
            output_fault.get_or_insert(error);
            cancellation.cancel();
            *failure_seen = true;
            break;
        }
        let started = Instant::now();
        if starts_test_phase_deadline(action) {
            test_lease.renew(started, TestPhaseEvent::ActionStart { ordinal: index + 1 });
        }
        let progress = ProgressState::new(started, progress_interval);
        match spawn_action(
            scope,
            runner,
            action,
            index,
            budget,
            cancellation,
            sender,
            progress.callback(),
        ) {
            Ok(()) => {
                nodes[index] = NodeState::Running(progress);
                *running += 1;
            }
            Err(source) => {
                let mut completion = failure(
                    VerifyError::Launch {
                        label: action.label(),
                        source,
                    },
                    Disposition::LaunchError,
                );
                completion.elapsed = started.elapsed();
                nodes[index] = NodeState::Complete(completion);
                cancellation.cancel();
                *failure_seen = true;
                break;
            }
        }
    }
}

fn starts_test_phase_deadline(action: Action) -> bool {
    matches!(action, Action::Browser | Action::WorkspaceTests)
}

/// The test-phase bound protects the two test branches, not their terminal
/// documentation, evidence, and landing consumers. Those consumers remain
/// bounded by the child's 357-second runtime inside the complete six-minute
/// supervisor envelope.
fn test_phase_complete(nodes: &[NodeState]) -> bool {
    VERIFY_ACTIONS
        .iter()
        .enumerate()
        .filter(|(_, action)| starts_test_phase_deadline(**action))
        .all(|(index, _)| matches!(nodes[index], NodeState::Complete(_)))
}

#[expect(
    clippy::too_many_arguments,
    reason = "the action thread receives explicit scheduling, cancellation, and progress authority"
)]
fn spawn_action<'scope, 'env: 'scope, R: Runner + 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    runner: &'scope R,
    action: Action,
    index: usize,
    budget: Budget,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<WorkerCompletion>,
    progress: ActionProgress,
) -> io::Result<()> {
    let sender = sender.clone();
    let cancellation = cancellation.clone();
    thread::Builder::new()
        .name(format!("verification-action-{}", index + 1))
        .spawn_scoped(scope, move || {
            let started = Instant::now();
            let readiness = matches!(
                action,
                Action::Command(step) if step.label == CANVAS_APP_LABEL
            )
            .then(|| {
                let sender = sender.clone();
                Arc::new(move || {
                    let _ = sender.send(WorkerCompletion::CanvasReady { index });
                }) as ActionReadiness
            });
            let mut completion = panic::catch_unwind(AssertUnwindSafe(|| {
                execute_action(runner, action, &cancellation, budget, readiness, progress)
            }))
            .unwrap_or_else(|payload| {
                failure(
                    VerifyError::WorkerPanic {
                        label: action.label(),
                        detail: panic_detail(payload),
                    },
                    Disposition::Panic,
                )
            });
            completion.elapsed = started.elapsed();
            let _ = sender.send(WorkerCompletion::Action { index, completion });
        })
        .map(|_| ())
}

struct CompletionState<'a> {
    nodes: &'a mut [NodeState],
    running: &'a mut usize,
    failure_seen: &'a mut bool,
    canvas_app_ready: &'a mut bool,
}

fn receive_completion<Rebalance>(
    receiver: &mpsc::Receiver<WorkerCompletion>,
    cancellation: &CancellationToken,
    setups: &mut setup::SetupRun<Rebalance>,
    test_lease: &mut TestPhaseLease,
    output: &mut impl Write,
    state: CompletionState<'_>,
) -> bool {
    match receiver.recv_timeout(SCHEDULER_POLL) {
        Ok(WorkerCompletion::Action { index, completion }) => {
            if starts_test_phase_deadline(VERIFY_ACTIONS[index]) {
                test_lease.renew(
                    Instant::now(),
                    TestPhaseEvent::ActionTerminal { ordinal: index + 1 },
                );
            }
            *state.running -= 1;
            let substantive = !matches!(
                completion.result,
                Ok(()) | Err(VerifyError::Cancelled { .. })
            );
            let defer_until_compilers_finish = index == 8 && !compiler_chain_complete(state.nodes);
            if substantive && !defer_until_compilers_finish {
                *state.failure_seen = true;
                cancellation.cancel();
            }
            state.nodes[index] = NodeState::Complete(completion);
            false
        }
        Ok(WorkerCompletion::WorkspacePrebuild(completion)) => {
            let admitted = completion.result.is_ok();
            *state.running -= 1;
            setups.complete_workspace_prebuild(completion, output, cancellation);
            if admitted {
                test_lease.renew(Instant::now(), TestPhaseEvent::WorkspacePrebuildTerminal);
            }
            *state.failure_seen = setups.failed();
            false
        }
        Ok(WorkerCompletion::CanvasReady { index }) => {
            debug_assert_eq!(index, 6);
            *state.canvas_app_ready = true;
            false
        }
        Err(mpsc::RecvTimeoutError::Timeout) => false,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            *state.failure_seen = true;
            cancellation.cancel();
            true
        }
    }
}

fn compiler_chain_complete(nodes: &[NodeState]) -> bool {
    matches!(
        nodes.get(6),
        Some(NodeState::Complete(ActionCompletion { result: Ok(()), .. }))
    )
}

fn deferred_workspace_failure_ready(nodes: &[NodeState]) -> bool {
    compiler_chain_complete(nodes)
        && matches!(
            nodes.get(8),
            Some(NodeState::Complete(ActionCompletion {
                result: Err(error),
                ..
            })) if !matches!(error, VerifyError::Cancelled { .. })
        )
}

fn write_due_progress<Rebalance>(
    nodes: &mut [NodeState],
    setups: &mut setup::SetupRun<Rebalance>,
    test_lease: &mut TestPhaseLease,
    output: &mut impl Write,
    cancellation: &CancellationToken,
    failure_seen: &mut bool,
    output_fault: &mut Option<io::Error>,
) {
    observe_test_phase_progress(nodes, test_lease);
    if let Err(error) = setups.write_due_progress(output) {
        output_fault.get_or_insert(error);
        cancellation.cancel();
        *failure_seen = true;
        return;
    }
    let mut report_test_advance = false;
    for (index, node) in nodes.iter_mut().enumerate() {
        let NodeState::Running(progress) = node else {
            continue;
        };
        let Some((elapsed, cursor)) = progress.due_cursor() else {
            continue;
        };
        let result = match cursor {
            Some(cursor) => {
                report_test_advance |= starts_test_phase_deadline(VERIFY_ACTIONS[index]);
                verify_timing::write_advancement(
                    output,
                    stage(index, VERIFY_ACTIONS[index]),
                    elapsed,
                    cursor,
                )
            }
            None => {
                verify_timing::write_progress(output, stage(index, VERIFY_ACTIONS[index]), elapsed)
            }
        };
        if let Err(error) = result {
            output_fault.get_or_insert(error);
            cancellation.cancel();
            *failure_seen = true;
        }
    }
    if let Some(evidence) = test_lease.take_evidence(Instant::now(), report_test_advance)
        && let Err(error) = write_test_phase_evidence(output, evidence)
    {
        output_fault.get_or_insert(error);
        cancellation.cancel();
        *failure_seen = true;
    }
}

fn observe_test_phase_progress(nodes: &mut [NodeState], lease: &mut TestPhaseLease) {
    let observed = Instant::now();
    for (index, node) in nodes.iter_mut().enumerate() {
        if !starts_test_phase_deadline(VERIFY_ACTIONS[index]) {
            continue;
        }
        let NodeState::Running(progress) = node else {
            continue;
        };
        let Some(cursor) = progress.take_lease_cursor() else {
            continue;
        };
        lease.renew(
            observed,
            TestPhaseEvent::ActionAdvance {
                ordinal: index + 1,
                cursor,
            },
        );
    }
}

fn write_test_phase_evidence(
    output: &mut impl Write,
    evidence: TestPhaseEvidence,
) -> io::Result<()> {
    writeln!(
        output,
        concat!(
            "[verify] test-lease generation={} reason={} inactivity_remaining_ms={} ",
            "absolute_remaining_ms={}"
        ),
        evidence.generation,
        evidence.reason,
        evidence.inactivity_remaining.as_millis(),
        evidence.absolute_remaining.as_millis(),
    )?;
    output.flush()
}

fn render_completions(
    nodes: &mut [NodeState],
    output: &mut impl Write,
) -> Result<Failures, VerifyError> {
    let mut substantive = Vec::new();
    let mut cleanup = Vec::new();
    for (index, node) in nodes.iter_mut().enumerate() {
        let NodeState::Complete(completion) = node else {
            continue;
        };
        if let Err(error) = output.write_all(&completion.output) {
            let _ = verify_timing::write_terminal(
                output,
                stage(index, VERIFY_ACTIONS[index]),
                completion.elapsed,
                &Disposition::RenderError.to_string(),
            );
            return Err(VerifyError::Output(error));
        }
        verify_timing::write_terminal(
            output,
            stage(index, VERIFY_ACTIONS[index]),
            completion.elapsed,
            &completion.disposition.to_string(),
        )
        .map_err(VerifyError::Output)?;
        if let Err(error) = std::mem::replace(&mut completion.result, Ok(())) {
            match error {
                VerifyError::Cancelled { .. } => {}
                VerifyError::Cleanup { .. } => cleanup.push((index, error)),
                _ => substantive.push((index, error)),
            }
        }
    }
    Ok(Failures {
        substantive,
        cleanup,
    })
}

fn ready_nodes(nodes: &[NodeState]) -> Vec<usize> {
    let mut ready = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            if !matches!(node, NodeState::Pending) {
                return None;
            }
            PREREQUISITES[index]
                .iter()
                .all(|predecessor| {
                    matches!(
                        &nodes[*predecessor],
                        NodeState::Complete(ActionCompletion { result: Ok(()), .. })
                    )
                })
                .then_some(index)
        })
        .collect::<Vec<_>>();
    ready.sort_by_key(|index| (Reverse(remaining_path_depth(*index)), *index));
    ready
}

/// Prefer the ready action with the longest remaining dependency path.
///
/// Report ordinals remain stable presentation identities. They are only the
/// deterministic tie-breaker after critical-path depth, so the workspace-test
/// branch cannot sit behind the shorter browser branch under a tight worker
/// budget and terminal consumers receive the earliest possible start.
fn remaining_path_depth(index: usize) -> usize {
    PREREQUISITES
        .iter()
        .enumerate()
        .filter(|(_, prerequisites)| prerequisites.contains(&index))
        .map(|(successor, _)| 1 + remaining_path_depth(successor))
        .max()
        .unwrap_or_default()
}

fn stage(index: usize, action: Action) -> Stage<'static> {
    Stage {
        ordinal: index + 1,
        total: VERIFY_ACTIONS.len(),
        identity: action.label(),
        family: action.family(),
    }
}

pub(super) fn verify_package_system(
    package: &str,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))?;
    let runner = super::SystemRunner::standalone();
    let result = verify_package_with_control(package, &runner, output, &cancellation);
    drop(signal_guard);
    if interrupted.load(Ordering::Acquire) {
        Err(VerifyError::Interrupted)
    } else {
        result
    }
}

#[cfg(test)]
pub(super) fn verify_package_with<R: Runner>(
    package: &str,
    runner: &R,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    verify_package_with_control(package, runner, output, &CancellationToken::new())
}

fn verify_package_with_control<R: Runner>(
    package: &str,
    runner: &R,
    output: &mut impl Write,
    cancellation: &CancellationToken,
) -> Result<(), VerifyError> {
    if !valid_package_name(package) {
        return Err(VerifyError::Package(package.to_owned()));
    }

    write_scope_inventory(package, output)?;
    let total = SCOPED_COMMANDS.len();
    for (index, command) in SCOPED_COMMANDS.into_iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(VerifyError::Interrupted);
        }
        let label = command.label(package);
        verify_timing::observe(
            output,
            Stage {
                ordinal: index + 1,
                total,
                identity: &label,
                family: "child-process:cargo",
            },
            || runner.run_scoped(command, package, cancellation, 1),
            |result, output| {
                let completion = execute_process(
                    result,
                    &label,
                    |source| VerifyError::ScopedLaunch {
                        label: label.clone(),
                        source,
                    },
                    |code| VerifyError::ScopedExit {
                        label: label.clone(),
                        code,
                    },
                );
                output
                    .write_all(&completion.output)
                    .map_err(VerifyError::Output)?;
                match completion.result {
                    Ok(()) => Ok(Conclusion::success((), completion.disposition)),
                    Err(error) => Ok(Conclusion::failure(error, completion.disposition)),
                }
            },
        )?;
    }

    Ok(())
}

fn write_scope_inventory(package: &str, output: &mut impl Write) -> Result<(), VerifyError> {
    writeln!(
        output,
        concat!(
            "[verify] scoped package {package}; feedback only: ",
            "run `cargo xtask verify` before task completion"
        ),
        package = package,
    )
    .map_err(VerifyError::Output)?;
    let skipped_actions = VERIFY_ACTIONS
        .iter()
        .map(|action| action.label())
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        "[verify] scoped full actions skipped: {skipped_actions}"
    )
    .map_err(VerifyError::Output)?;
    writeln!(output, "[verify] scoped gate arms run: none").map_err(VerifyError::Output)?;
    let skipped_arms = gate::GateId::BATTERY
        .into_iter()
        .map(gate::GateId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "[verify] scoped gate arms skipped: {skipped_arms}")
        .map_err(VerifyError::Output)
}

#[cfg(test)]
mod tests;
