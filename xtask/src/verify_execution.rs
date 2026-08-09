//! Dependency-aware execution of the full verifier and serial scoped feedback.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
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
    VerifyError, valid_package_name,
};

/// The explicit full-verifier dependency graph, indexed by report ordinal.
///
/// The compiler-evidence chain is serial and retains its warm build root. Once
/// the prebuilt app exists, Canvas and the sharded workspace tests may overlap.
/// Documentation follows the test coordinator, the evidence-consuming battery
/// waits for both branches, and entry-point audit and landing remain barriers.
const PREREQUISITES: [&[usize]; VERIFY_ACTIONS.len()] = [
    &[],
    &[0],
    &[1],
    &[2],
    &[3],
    &[4],
    &[5],
    &[5],
    &[7],
    &[8],
    &[2, 4, 5, 6, 9],
    &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
];

/// Three is the fleet-safe ceiling for independently ready outer actions.
const MAX_CONCURRENT_ACTIONS: usize = 3;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(30);
const SCHEDULER_POLL: Duration = Duration::from_millis(50);
const PROCESS_RUNTIME: Duration = Duration::from_secs(30 * 60);
const OUTPUT_DRAIN: Duration = Duration::from_secs(30);
const REAP_DEADLINE: Duration = Duration::from_secs(10);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;

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
        let workspace_workers = cpu_slots
            .saturating_sub(sibling_reserve)
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
            workspace_workers: cargo_jobs.clamp(1, super::WORKSPACE_TEST_CONCURRENCY_CAP),
            active_cargo_jobs: Some(active_cargo_jobs),
        }
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
    output_fault: Option<io::Error>,
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
    debug_assert_eq!(VERIFY_ACTIONS.len(), PREREQUISITES.len());
    let active = budget
        .active_cargo_jobs
        .map_or_else(|| "unknown".to_owned(), |count| count.to_string());
    writeln!(
        output,
        concat!(
            "[verify] scheduler workers={} cargo_jobs={} ",
            "active_peer_cargo_jobs={} workspace_workers={} action_count={}"
        ),
        budget.workers,
        budget.cargo_jobs,
        active,
        budget.workspace_workers,
        VERIFY_ACTIONS.len(),
    )
    .map_err(VerifyError::Output)?;
    output.flush().map_err(VerifyError::Output)?;

    let GraphRun {
        mut nodes,
        output_fault,
    } = schedule_graph(runner, output, budget, &cancellation);
    if let Some(error) = output_fault {
        return Err(VerifyError::Output(error));
    }

    let Failures {
        mut substantive,
        mut cleanup,
    } = render_completions(&mut nodes, output)?;

    if interrupted.load(Ordering::Acquire) {
        write_failure_summary(output, None, &cleanup, true)?;
        return Err(VerifyError::Interrupted);
    }
    substantive.sort_by_key(|(index, _)| *index);
    cleanup.sort_by_key(|(index, _)| *index);
    if let Some((primary_index, primary)) = substantive.into_iter().next() {
        write_failure_summary(output, Some(primary_index), &cleanup, false)?;
        return Err(primary);
    }
    if !cleanup.is_empty() {
        let (primary_index, primary) = cleanup.remove(0);
        write_failure_summary(output, Some(primary_index), &cleanup, false)?;
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
) -> GraphRun {
    let (sender, receiver) = mpsc::channel::<WorkerCompletion>();
    let mut nodes = (0..VERIFY_ACTIONS.len())
        .map(|_| NodeState::Pending)
        .collect::<Vec<_>>();
    let mut running = 0usize;
    let mut failure_seen = false;
    let mut output_fault = None;

    thread::scope(|scope| {
        loop {
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
        output_fault,
    }
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

fn execute_action<R: Runner>(
    runner: &R,
    action: Action,
    cancellation: &CancellationToken,
    budget: Budget,
) -> ActionCompletion {
    match action {
        Action::EntryPoint => match runner.entry_point() {
            Ok(report) => success(report.into_bytes(), Disposition::Success),
            Err(detail) => failure(VerifyError::EntryPoint(detail), Disposition::Error),
        },
        Action::Format => execute_process(
            runner.format(cancellation, budget.cargo_jobs),
            FORMAT_LABEL,
            |source| VerifyError::Launch {
                label: FORMAT_LABEL,
                source,
            },
            |code| VerifyError::Exit {
                label: FORMAT_LABEL,
                code,
            },
        ),
        Action::Command(step) => execute_process(
            runner.run(step, cancellation, budget.cargo_jobs),
            step.label,
            |source| VerifyError::Launch {
                label: step.label,
                source,
            },
            |code| VerifyError::Exit {
                label: step.label,
                code,
            },
        ),
        Action::Browser => execute_browser(runner.browser(cancellation, budget.cargo_jobs)),
        Action::WorkspaceTests => execute_process(
            runner.workspace_tests(cancellation, budget.cargo_jobs, budget.workspace_workers),
            super::WORKSPACE_TEST_LABEL,
            |source| VerifyError::Launch {
                label: super::WORKSPACE_TEST_LABEL,
                source,
            },
            |code| VerifyError::Exit {
                label: super::WORKSPACE_TEST_LABEL,
                code,
            },
        ),
        Action::Battery => execute_battery(runner.battery(cancellation)),
        Action::Landing => match runner.landing() {
            Err(source) => failure(VerifyError::Landing(source.to_string()), Disposition::Error),
            Ok(report) => {
                let mut document = Vec::new();
                match serde_json::to_writer(&mut document, &report) {
                    Ok(()) if report.admitted() => {
                        document.push(b'\n');
                        success(document, Disposition::Success)
                    }
                    Ok(()) => {
                        document.push(b'\n');
                        failure_with_bytes(
                            VerifyError::LandingRefused,
                            Disposition::Refused,
                            document,
                        )
                    }
                    Err(source) => failure(VerifyError::Encode(source), Disposition::RenderError),
                }
            }
        },
    }
}

fn execute_process(
    run: Result<CommandRun, RunError>,
    label: &str,
    launch_error: impl FnOnce(io::Error) -> VerifyError,
    exit_error: impl FnOnce(Option<i32>) -> VerifyError,
) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(launch_error(source), Disposition::LaunchError),
        Err(RunError::EvidenceRefused(detail)) => failure(
            VerifyError::EvidenceRefused {
                label: label.to_owned(),
                detail,
            },
            Disposition::Refused,
        ),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: label.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(CommandRun::Cancelled(output)) => failure_with_output(
            VerifyError::Cancelled {
                label: label.to_owned(),
            },
            Disposition::Cancelled,
            output,
        ),
        Ok(CommandRun::TimedOut(output)) => failure_with_output(
            VerifyError::TimedOut {
                label: label.to_owned(),
            },
            Disposition::TimedOut,
            output,
        ),
        Ok(CommandRun::Exited(output)) if output.status.success => success(
            process_bytes(&output),
            Disposition::from_status(output.status.code),
        ),
        Ok(CommandRun::Exited(output)) => {
            let code = output.status.code;
            failure_with_output(exit_error(code), Disposition::from_status(code), output)
        }
    }
}

fn panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(detail) = payload.downcast_ref::<&str>() {
        (*detail).to_owned()
    } else if let Some(detail) = payload.downcast_ref::<String>() {
        detail.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn execute_browser(run: Result<BrowserRun, RunError>) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(
            VerifyError::BrowserLaunch { source },
            Disposition::LaunchError,
        ),
        Err(RunError::EvidenceRefused(detail)) => failure(
            VerifyError::EvidenceRefused {
                label: BROWSER_LABEL.to_owned(),
                detail,
            },
            Disposition::Refused,
        ),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: BROWSER_LABEL.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(BrowserRun {
            process: CommandRun::Cancelled(output),
            ..
        }) => failure_with_output(
            VerifyError::Cancelled {
                label: BROWSER_LABEL.to_owned(),
            },
            Disposition::Cancelled,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::TimedOut(output),
            ..
        }) => failure_with_output(
            VerifyError::TimedOut {
                label: BROWSER_LABEL.to_owned(),
            },
            Disposition::TimedOut,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: _,
        }) if !output.status.success => {
            let code = output.status.code;
            failure_with_output(
                VerifyError::BrowserExit { code },
                Disposition::from_status(code),
                output,
            )
        }
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: false,
        }) => failure_with_output(
            VerifyError::BrowserIncomplete,
            Disposition::Incomplete,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: true,
        }) => success(
            process_bytes(&output),
            Disposition::from_status(output.status.code),
        ),
    }
}

fn execute_battery(run: Result<BatteryRun, RunError>) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(
            VerifyError::Battery(source.to_string()),
            Disposition::LaunchError,
        ),
        Err(RunError::EvidenceRefused(detail)) => failure(
            VerifyError::EvidenceRefused {
                label: BATTERY_LABEL.to_owned(),
                detail,
            },
            Disposition::Refused,
        ),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: BATTERY_LABEL.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(BatteryRun::Admitted(document)) => {
            let mut output = document;
            output.push(b'\n');
            success(output, Disposition::Success)
        }
        Ok(BatteryRun::Refused(diagnostic)) => failure_with_bytes(
            VerifyError::BatteryRefused,
            Disposition::Refused,
            diagnostic,
        ),
        Ok(BatteryRun::Error(detail)) => failure(VerifyError::Battery(detail), Disposition::Error),
        Ok(BatteryRun::Cancelled) => failure(
            VerifyError::Cancelled {
                label: BATTERY_LABEL.to_owned(),
            },
            Disposition::Cancelled,
        ),
    }
}

fn success(output: Vec<u8>, disposition: Disposition) -> ActionCompletion {
    ActionCompletion {
        elapsed: Duration::ZERO,
        output,
        disposition,
        result: Ok(()),
    }
}

fn failure(error: VerifyError, disposition: Disposition) -> ActionCompletion {
    failure_with_bytes(error, disposition, Vec::new())
}

fn failure_with_output(
    error: VerifyError,
    disposition: Disposition,
    output: ProcessOutput,
) -> ActionCompletion {
    failure_with_bytes(error, disposition, process_bytes(&output))
}

fn failure_with_bytes(
    error: VerifyError,
    disposition: Disposition,
    output: Vec<u8>,
) -> ActionCompletion {
    ActionCompletion {
        elapsed: Duration::ZERO,
        output,
        disposition,
        result: Err(error),
    }
}

fn process_bytes(output: &ProcessOutput) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(output.stdout.len() + output.stderr.len());
    bytes.extend_from_slice(&output.stdout);
    bytes.extend_from_slice(&output.stderr);
    bytes
}

fn write_failure_summary(
    output: &mut impl Write,
    primary: Option<usize>,
    cleanup: &[(usize, VerifyError)],
    interrupted: bool,
) -> Result<(), VerifyError> {
    for (index, error) in cleanup {
        writeln!(
            output,
            "[verify] scheduler=cleanup-fault ordinal={} identity={:?} detail={:?}",
            index + 1,
            VERIFY_ACTIONS[*index].label(),
            error.to_string(),
        )
        .map_err(VerifyError::Output)?;
    }
    writeln!(
        output,
        "[verify] scheduler=failed primary_ordinal={} interrupted={} cleanup_faults={}",
        primary.map_or_else(|| "operator".to_owned(), |index| (index + 1).to_string()),
        interrupted,
        cleanup.len(),
    )
    .map_err(VerifyError::Output)
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
    let skipped_arms = gate::GateId::ALL
        .into_iter()
        .map(gate::GateId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "[verify] scoped gate arms skipped: {skipped_arms}")
        .map_err(VerifyError::Output)
}

pub(super) fn run_process<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    overrides: &[(&str, &OsStr)],
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let mut environment = ambient_environment()?
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    environment.insert(
        OsString::from("CARGO_BUILD_JOBS"),
        OsString::from(cargo_jobs.to_string()),
    );
    for (key, value) in overrides {
        environment.insert(OsString::from(key), (*value).to_owned());
    }
    let environment = Environment::new(environment).map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })?;
    let deadlines =
        Deadlines::new(PROCESS_RUNTIME, OUTPUT_DRAIN, REAP_DEADLINE).map_err(|error| {
            RunError::Launch(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?;
    let wait = WaitPolicy::new(
        format!("verifier action {label}"),
        Cancellation::Token(cancellation.clone()),
        RestartDisposition::Never,
    )
    .map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })?;
    let spec = Spec {
        argv: Argv::new(program, arguments).map_err(|error| {
            RunError::Launch(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?,
        cwd: std::env::current_dir().map_err(RunError::Launch)?,
        environment,
        group: ProcessGroup::New,
        deadlines,
        wait,
        output: SillOutput::new(
            StdinPolicy::Null,
            StreamPolicy::Capture {
                limit: OUTPUT_LIMIT,
            },
            StreamPolicy::Capture {
                limit: OUTPUT_LIMIT,
            },
        ),
        redaction: Redaction::None,
        grace: TERMINATION_GRACE,
    };
    let running = sill::spawn(spec).map_err(RunError::Launch)?;
    let outcome = running
        .wait()
        .map_err(|error| RunError::Cleanup(error.to_string()))?;
    if !outcome.outputs_drained() {
        return Err(RunError::Cleanup(format!(
            "{label} output did not drain before cleanup"
        )));
    }
    let stdout = captured(label, "stdout", outcome.stdout())?;
    let stderr = captured(label, "stderr", outcome.stderr())?;
    let status = outcome.completion().status();
    let output = ProcessOutput {
        status: Status {
            success: status.success(),
            code: status.code(),
        },
        stdout,
        stderr,
    };
    Ok(match outcome.completion() {
        Completion::Exited(_) => CommandRun::Exited(output),
        Completion::Cancelled(_) => CommandRun::Cancelled(output),
        Completion::TimedOut(_) => CommandRun::TimedOut(output),
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the verifier's native process rim captures the exact child environment"
)]
fn ambient_environment() -> Result<Environment, RunError> {
    Environment::new(std::env::vars_os()).map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })
}

fn captured(
    label: &str,
    stream: &str,
    output: Option<&sill::CapturedOutput>,
) -> Result<Vec<u8>, RunError> {
    let output = output.ok_or_else(|| {
        RunError::Cleanup(format!("{label} {stream} capture is unexpectedly absent"))
    })?;
    if output.truncated() || !output.complete() {
        return Err(RunError::Cleanup(format!(
            "{label} {stream} capture exceeded its bounded complete-output contract"
        )));
    }
    Ok(output.bytes().to_vec())
}

fn active_peer_cargo_jobs() -> io::Result<usize> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ps exited {:?} while counting peer Cargo jobs",
            output.status.code()
        )));
    }
    let listing = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    Ok(count_peer_cargo(&listing, std::process::id()))
}

fn count_peer_cargo(listing: &str, current: u32) -> usize {
    let processes = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    let parents = processes
        .iter()
        .map(|(pid, parent, _)| (*pid, *parent))
        .collect::<BTreeMap<_, _>>();
    let mut ancestors = BTreeSet::new();
    let mut cursor = current;
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == 0 || !ancestors.insert(parent) {
            break;
        }
        cursor = parent;
    }
    processes
        .iter()
        .filter(|(pid, _, command)| {
            !ancestors.contains(pid)
                && PathBuf::from(command)
                    .file_name()
                    .is_some_and(|name| name == OsStr::new("cargo"))
        })
        .count()
}

#[cfg(not(target_arch = "wasm32"))]
struct SignalGuard {
    handle: signal_hook::iterator::Handle,
    join: Option<thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SignalGuard {
    fn install(
        cancellation: CancellationToken,
        interrupted: Arc<AtomicBool>,
    ) -> Result<Self, VerifyError> {
        use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])
            .map_err(VerifyError::InterruptInstall)?;
        let handle = signals.handle();
        let join = thread::Builder::new()
            .name("verify-signal-bridge".to_owned())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    interrupted.store(true, Ordering::Release);
                    cancellation.cancel();
                }
            })
            .map_err(VerifyError::InterruptInstall)?;
        Ok(Self {
            handle,
            join: Some(join),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SignalGuard {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(target_arch = "wasm32")]
struct SignalGuard;

#[cfg(target_arch = "wasm32")]
impl SignalGuard {
    fn install(
        _cancellation: CancellationToken,
        _interrupted: Arc<AtomicBool>,
    ) -> Result<Self, VerifyError> {
        Ok(Self)
    }
}

#[cfg(test)]
include!("verify_execution_tests.rs");
