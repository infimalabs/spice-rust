//! Dependency-aware execution of the full verifier and serial scoped feedback.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use sill::{
    Argv, Cancellation, CancellationToken, Completion, Deadlines, Environment,
    Output as SillOutput, ProcessGroup, Redaction, RestartDisposition, Spec, StdinPolicy,
    StreamPolicy, WaitPolicy,
};

use super::verify_timing::{self, Conclusion, Disposition, Stage};
use super::{
    Action, BATTERY_LABEL, BROWSER_LABEL, BatteryRun, BrowserRun, CommandRun, ENTRY_POINT_LABEL,
    FORMAT_LABEL, ProcessOutput, RunError, Runner, SCOPED_COMMANDS, Status, VERIFY_ACTIONS,
    VerifyError, WORKSPACE_TEST_PREBUILD_LABEL, valid_package_name,
};

/// The explicit full-verifier dependency graph, indexed by report ordinal.
///
/// The compiler-evidence chain is serial and retains its warm build root.
/// Workspace tests reuse that root only after the Canvas app and resident build
/// have finished, so a Rust-test branch cannot mask a compiler failure or force
/// the selected Cargo root to publish tests while another Cargo build stage is
/// still active. Browser verification then overlaps the Rust-test branch while
/// performing no Cargo work of its own. Documentation follows the workspace
/// tests so the reusable native Cargo root is not forced into competing
/// documentation and test publication work. The evidence-consuming battery
/// overlaps that documentation tail once browser and workspace tests have
/// completed; contract landing remains the barrier over every action.
const PREREQUISITES: [&[usize]; VERIFY_ACTIONS.len()] = [
    &[],
    &[0],
    &[1],
    &[2],
    &[3],
    &[4],
    &[5],
    &[6],
    &[6],
    &[8],
    &[9],
    &[2, 4, 5, 6, 7, 8],
    &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
];

/// Three is the fleet-safe ceiling for independently ready outer actions.
const MAX_CONCURRENT_ACTIONS: usize = 3;
/// The full verifier prebuilds test binaries once in the native compiler output
/// root before parallel shard execution, so every coordinator lane can reuse the
/// same selected-root build cache while retaining its own final-publication path.
///
/// Standalone workspace verification may use the whole coordinator cap. With no
/// peer Cargo process, the full verifier may use that same cap so all deterministic
/// shards can finish inside the fixed test-phase window. One peer caps the branch
/// at two lanes; two or more peers collapse it to one so the verifier does not
/// become a cross-lane contention source while Canvas is also live.
const ONE_PEER_FULL_VERIFIER_WORKSPACE_LANES: usize = 2;
const _: () =
    assert!(ONE_PEER_FULL_VERIFIER_WORKSPACE_LANES <= super::WORKSPACE_TEST_CONCURRENCY_CAP);
/// Stop launching and cancel owned process groups after 180 seconds of actual
/// test-phase runtime. Compiler setup can be cold once; it is still serialized
/// for low contention, but it does not consume the test runtime budget.
const FULL_VERIFIER_TEST_RUNTIME: Duration = Duration::from_secs(3 * 60);
/// Give the child-owned scheduler enough room to report and reap after the
/// test-phase deadline while remaining well inside the five-minute wall guard.
const FULL_VERIFIER_SUPERVISOR_RUNTIME: Duration = Duration::from_secs(296);
const FULL_VERIFIER_HARD_RUNTIME: Duration = Duration::from_secs(5 * 60);
const FULL_VERIFIER_LABEL: &str =
    "full verifier five-minute envelope (three-minute test-phase cutoff)";
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

#[derive(Clone, Copy)]
struct ProcessBounds {
    runtime: Duration,
    output_drain: Duration,
    reap: Duration,
    grace: Duration,
    output_limit: usize,
}

const ACTION_PROCESS_BOUNDS: ProcessBounds = ProcessBounds {
    runtime: PROCESS_RUNTIME,
    output_drain: OUTPUT_DRAIN,
    reap: REAP_DEADLINE,
    grace: TERMINATION_GRACE,
    output_limit: OUTPUT_LIMIT,
};
const FULL_VERIFIER_PROCESS_BOUNDS: ProcessBounds = ProcessBounds {
    runtime: FULL_VERIFIER_SUPERVISOR_RUNTIME,
    output_drain: FULL_VERIFIER_OUTPUT_DRAIN,
    reap: FULL_VERIFIER_REAP_DEADLINE,
    grace: FULL_VERIFIER_TERMINATION_GRACE,
    output_limit: FULL_VERIFIER_OUTPUT_LIMIT,
};
const _: () = assert!(
    FULL_VERIFIER_PROCESS_BOUNDS.runtime.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.grace.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.reap.as_secs()
        + FULL_VERIFIER_PROCESS_BOUNDS.output_drain.as_secs()
        < FULL_VERIFIER_HARD_RUNTIME.as_secs()
);
const _: () =
    assert!(FULL_VERIFIER_PROCESS_BOUNDS.runtime.as_secs() > FULL_VERIFIER_TEST_RUNTIME.as_secs());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Budget {
    workers: usize,
    cargo_jobs: usize,
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
                workspace_workers: 1,
                active_cargo_jobs: None,
            };
        };
        let fleet_slots =
            MAX_CONCURRENT_ACTIONS.saturating_sub(active.min(MAX_CONCURRENT_ACTIONS - 1));
        let cpu_slots = cpus.saturating_sub(active).max(1);
        let workers = fleet_slots.min(cpu_slots);
        let participants = active + workers;
        let cargo_jobs = (cpus / participants.max(1)).max(1);
        let sibling_reserve = cargo_jobs.saturating_mul(workers.saturating_sub(1));
        let workspace_job_cap = cargo_jobs.max(1);
        let workspace_workers = cpu_slots
            .saturating_sub(sibling_reserve)
            .min(workspace_job_cap)
            .min(workspace_lane_cap(active))
            .clamp(1, super::WORKSPACE_TEST_CONCURRENCY_CAP);
        Self {
            workers,
            cargo_jobs,
            workspace_workers,
            active_cargo_jobs: Some(active),
        }
    }

    #[cfg(test)]
    fn fixture(workers: usize, cargo_jobs: usize, active_cargo_jobs: usize) -> Self {
        Self {
            workers,
            cargo_jobs,
            workspace_workers: cargo_jobs
                .min(workspace_lane_cap(active_cargo_jobs))
                .clamp(1, super::WORKSPACE_TEST_CONCURRENCY_CAP),
            active_cargo_jobs: Some(active_cargo_jobs),
        }
    }
}

fn workspace_lane_cap(active_cargo_jobs: usize) -> usize {
    match active_cargo_jobs {
        0 => super::WORKSPACE_TEST_CONCURRENCY_CAP,
        1 => ONE_PEER_FULL_VERIFIER_WORKSPACE_LANES,
        _ => 1,
    }
}

enum NodeState {
    Pending,
    Running {
        started: Instant,
        next_progress: Duration,
    },
    Complete(ActionCompletion),
}

struct ActionCompletion {
    elapsed: Duration,
    output: Vec<u8>,
    disposition: Disposition,
    result: Result<(), VerifyError>,
}

struct WorkerCompletion {
    index: usize,
    completion: ActionCompletion,
}

struct GraphRun {
    nodes: Vec<NodeState>,
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
    let run = run_bounded_process(
        FULL_VERIFIER_LABEL,
        executable,
        [OsString::from(FULL_VERIFIER_CHILD_COMMAND)],
        &cwd,
        &cancellation,
        None,
        &[],
        FULL_VERIFIER_PROCESS_BOUNDS,
    );
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
            output
                .write_all(&child.stdout)
                .map_err(VerifyError::Output)?;
            return Err(VerifyError::Interrupted);
        }
        CommandRun::TimedOut(child) => {
            output
                .write_all(&child.stdout)
                .map_err(VerifyError::Output)?;
            return Err(VerifyError::TimedOut {
                label: FULL_VERIFIER_LABEL.to_owned(),
            });
        }
    };
    output
        .write_all(&child.stdout)
        .map_err(VerifyError::Output)?;
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

pub(super) fn verify_system(output: &mut impl Write) -> Result<(), VerifyError> {
    let cancellation = CancellationToken::new();
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal_guard = SignalGuard::install(cancellation.clone(), Arc::clone(&interrupted))?;
    let runner = super::SystemRunner::prepare().map_err(VerifyError::Battery)?;
    let result = verify_with_control(&runner, output, Budget::detect(), cancellation, interrupted);
    drop(signal_guard);
    result
}

fn verify_with_control<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
) -> Result<(), VerifyError> {
    verify_with_deadline(
        runner,
        output,
        budget,
        cancellation,
        interrupted,
        FULL_VERIFIER_TEST_RUNTIME,
    )
}

fn verify_with_deadline<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: CancellationToken,
    interrupted: Arc<AtomicBool>,
    runtime: Duration,
) -> Result<(), VerifyError> {
    debug_assert_eq!(VERIFY_ACTIONS.len(), PREREQUISITES.len());
    let active = budget
        .active_cargo_jobs
        .map_or_else(|| "unknown".to_owned(), |count| count.to_string());
    writeln!(
        output,
        concat!(
            "[verify] scheduler workers={} cargo_jobs={} ",
            "active_peer_cargo_jobs={} workspace_workers={} action_count={} deadline_scope=test-phase deadline_ms={}"
        ),
        budget.workers,
        budget.cargo_jobs,
        active,
        budget.workspace_workers,
        VERIFY_ACTIONS.len(),
        runtime.as_millis(),
    )
    .map_err(VerifyError::Output)?;
    output.flush().map_err(VerifyError::Output)?;

    let GraphRun {
        mut nodes,
        prebuild_failure,
        output_fault,
        deadline_exceeded,
    } = schedule_graph(runner, output, budget, &cancellation, runtime);
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
    Ok(())
}

fn schedule_graph<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    budget: Budget,
    cancellation: &CancellationToken,
    runtime: Duration,
) -> GraphRun {
    let (sender, receiver) = mpsc::channel::<WorkerCompletion>();
    let mut nodes = (0..VERIFY_ACTIONS.len())
        .map(|_| NodeState::Pending)
        .collect::<Vec<_>>();
    let mut running = 0usize;
    let mut failure_seen = false;
    let mut output_fault = None;
    let mut deadline_exceeded = false;
    let mut test_phase_started = None;
    let mut workspace_tests_prebuilt = false;
    let mut prebuild_failure = None;

    thread::scope(|scope| {
        loop {
            if !failure_seen
                && test_phase_started.is_some_and(|started: Instant| started.elapsed() >= runtime)
            {
                deadline_exceeded = true;
                failure_seen = true;
                cancellation.cancel();
            }
            if !failure_seen
                && !cancellation.is_cancelled()
                && !workspace_tests_prebuilt
                && workspace_test_prebuild_ready(&nodes)
            {
                let completion = execute_workspace_test_prebuild(runner, cancellation, budget);
                workspace_tests_prebuilt = completion.result.is_ok();
                if let Err(error) = render_workspace_test_prebuild(completion, output) {
                    match error {
                        VerifyError::Output(error) => {
                            output_fault.get_or_insert(error);
                            failure_seen = true;
                            cancellation.cancel();
                        }
                        error => {
                            prebuild_failure = Some(error);
                            failure_seen = true;
                            cancellation.cancel();
                        }
                    }
                }
                if !workspace_tests_prebuilt {
                    failure_seen = true;
                    cancellation.cancel();
                }
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
                    &mut test_phase_started,
                );
            } else if cancellation.is_cancelled() {
                failure_seen = true;
            }
            if running == 0 {
                break;
            }
            if receive_completion(
                &receiver,
                &mut nodes,
                &mut running,
                &mut failure_seen,
                cancellation,
            ) {
                break;
            }
            write_due_progress(
                &mut nodes,
                output,
                cancellation,
                &mut failure_seen,
                &mut output_fault,
            );
        }
    });
    GraphRun {
        nodes,
        prebuild_failure,
        output_fault,
        deadline_exceeded,
    }
}

fn workspace_test_prebuild_ready(nodes: &[NodeState]) -> bool {
    matches!(
        nodes.get(6),
        Some(NodeState::Complete(ActionCompletion { result: Ok(()), .. }))
    ) && matches!(nodes.get(7), Some(NodeState::Pending))
        && matches!(nodes.get(8), Some(NodeState::Pending))
}

fn execute_workspace_test_prebuild<R: Runner>(
    runner: &R,
    cancellation: &CancellationToken,
    budget: Budget,
) -> ActionCompletion {
    let started = Instant::now();
    let mut completion = execute_process(
        runner.workspace_test_prebuild(cancellation, budget.cargo_jobs),
        WORKSPACE_TEST_PREBUILD_LABEL,
        |source| VerifyError::Launch {
            label: WORKSPACE_TEST_PREBUILD_LABEL,
            source,
        },
        |code| VerifyError::Exit {
            label: WORKSPACE_TEST_PREBUILD_LABEL,
            code,
        },
    );
    completion.elapsed = started.elapsed();
    completion
}

fn render_workspace_test_prebuild(
    completion: ActionCompletion,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    writeln!(
        output,
        "[verify] stage=setup identity={WORKSPACE_TEST_PREBUILD_LABEL:?}"
    )
    .map_err(VerifyError::Output)?;
    output
        .write_all(&completion.output)
        .map_err(VerifyError::Output)?;
    writeln!(
        output,
        "[verify] stage=setup-terminal elapsed_ms={} disposition={} identity={WORKSPACE_TEST_PREBUILD_LABEL:?}",
        completion.elapsed.as_millis(),
        completion.disposition,
    )
    .map_err(VerifyError::Output)?;
    completion.result
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
    test_phase_started: &mut Option<Instant>,
) {
    for index in ready_nodes(nodes) {
        if *running >= budget.workers {
            break;
        }
        let action = VERIFY_ACTIONS[index];
        if let Err(error) = verify_timing::write_start(output, stage(index, action)) {
            output_fault.get_or_insert(error);
            cancellation.cancel();
            *failure_seen = true;
            break;
        }
        let started = Instant::now();
        if test_phase_started.is_none() && starts_test_phase_deadline(action) {
            *test_phase_started = Some(started);
        }
        match spawn_action(scope, runner, action, index, budget, cancellation, sender) {
            Ok(()) => {
                nodes[index] = NodeState::Running {
                    started,
                    next_progress: PROGRESS_INTERVAL,
                };
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

fn spawn_action<'scope, 'env: 'scope, R: Runner + 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    runner: &'scope R,
    action: Action,
    index: usize,
    budget: Budget,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<WorkerCompletion>,
) -> io::Result<()> {
    let sender = sender.clone();
    let cancellation = cancellation.clone();
    thread::Builder::new()
        .name(format!("verify-action-{}", index + 1))
        .spawn_scoped(scope, move || {
            let started = Instant::now();
            let mut completion = panic::catch_unwind(AssertUnwindSafe(|| {
                execute_action(runner, action, &cancellation, budget)
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
            let _ = sender.send(WorkerCompletion { index, completion });
        })
        .map(|_| ())
}

fn receive_completion(
    receiver: &mpsc::Receiver<WorkerCompletion>,
    nodes: &mut [NodeState],
    running: &mut usize,
    failure_seen: &mut bool,
    cancellation: &CancellationToken,
) -> bool {
    match receiver.recv_timeout(SCHEDULER_POLL) {
        Ok(worker) => {
            *running -= 1;
            if !matches!(
                worker.completion.result,
                Ok(()) | Err(VerifyError::Cancelled { .. })
            ) {
                *failure_seen = true;
                cancellation.cancel();
            }
            nodes[worker.index] = NodeState::Complete(worker.completion);
            false
        }
        Err(mpsc::RecvTimeoutError::Timeout) => false,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            *failure_seen = true;
            cancellation.cancel();
            true
        }
    }
}

fn write_due_progress(
    nodes: &mut [NodeState],
    output: &mut impl Write,
    cancellation: &CancellationToken,
    failure_seen: &mut bool,
    output_fault: &mut Option<io::Error>,
) {
    for (index, node) in nodes.iter_mut().enumerate() {
        let NodeState::Running {
            started,
            next_progress,
        } = node
        else {
            continue;
        };
        if started.elapsed() >= *next_progress {
            if let Err(error) = verify_timing::write_progress(
                output,
                stage(index, VERIFY_ACTIONS[index]),
                *next_progress,
            ) {
                output_fault.get_or_insert(error);
                cancellation.cancel();
                *failure_seen = true;
            }
            *next_progress += PROGRESS_INTERVAL;
        }
    }
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
    nodes
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
        .collect()
}

fn stage(index: usize, action: Action) -> Stage<'static> {
    Stage {
        ordinal: index + 1,
        total: VERIFY_ACTIONS.len(),
        identity: action.label(),
        family: action.family(),
    }
}

include!("verify_execution/action.rs");

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
    let total = SCOPED_COMMANDS.len() + 1;
    run_entry_point_serial(
        runner,
        output,
        Stage {
            ordinal: 1,
            total,
            identity: ENTRY_POINT_LABEL,
            family: "in-process:entry-point-audit",
        },
    )?;
    for (index, command) in SCOPED_COMMANDS.into_iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err(VerifyError::Interrupted);
        }
        let label = command.label(package);
        verify_timing::observe(
            output,
            Stage {
                ordinal: index + 2,
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

fn run_entry_point_serial<R: Runner>(
    runner: &R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.entry_point(),
        |result, output| match result {
            Ok(report) => {
                output
                    .write_all(report.as_bytes())
                    .map_err(VerifyError::Output)?;
                Ok(Conclusion::success((), Disposition::Success))
            }
            Err(detail) => Ok(Conclusion::failure(
                VerifyError::EntryPoint(detail),
                Disposition::Error,
            )),
        },
    )
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
        .filter(|action| !matches!(action, Action::EntryPoint))
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

include!("verify_execution/process.rs");

#[cfg(test)]
include!("verify_execution_tests.rs");
