//! Setup gates interleaved with the verifier action graph.

use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use sill::CancellationToken;

use super::{
    ActionCompletion, Budget, Disposition, NodeState, Runner, STRUCTURAL_PREFLIGHT_LABEL,
    VerifyError, WORKSPACE_TEST_PREBUILD_LABEL, WorkerCompletion, execute_battery, execute_process,
    failure, panic_detail,
};

pub(super) struct SetupRun<Rebalance> {
    structural_completed: bool,
    structural_failure: Option<VerifyError>,
    workspace_prebuild_started: bool,
    workspace_prebuilt: bool,
    prebuild_failure: Option<VerifyError>,
    output_fault: Option<io::Error>,
    rebalance: Option<Rebalance>,
}

impl<Rebalance> SetupRun<Rebalance> {
    pub(super) const fn new(rebalance: Rebalance) -> Self {
        Self {
            structural_completed: false,
            structural_failure: None,
            workspace_prebuild_started: false,
            workspace_prebuilt: false,
            prebuild_failure: None,
            output_fault: None,
            rebalance: Some(rebalance),
        }
    }

    pub(super) const fn failed(&self) -> bool {
        self.structural_failure.is_some()
            || self.prebuild_failure.is_some()
            || self.output_fault.is_some()
    }

    pub(super) const fn test_branches_ready(&self) -> bool {
        self.workspace_prebuilt
    }
    pub(super) fn into_failures(
        self,
    ) -> (Option<VerifyError>, Option<VerifyError>, Option<io::Error>) {
        (
            self.structural_failure,
            self.prebuild_failure,
            self.output_fault,
        )
    }

    fn record(
        &mut self,
        result: Result<(), VerifyError>,
        structural: bool,
        cancellation: &CancellationToken,
    ) {
        let Err(error) = result else {
            return;
        };
        match error {
            VerifyError::Output(error) => {
                self.output_fault.get_or_insert(error);
            }
            VerifyError::Cancelled { .. } => {}
            error if structural => self.structural_failure = Some(error),
            error => self.prebuild_failure = Some(error),
        }
        cancellation.cancel();
    }

    pub(super) fn complete_workspace_prebuild(
        &mut self,
        completion: ActionCompletion,
        output: &mut impl Write,
        cancellation: &CancellationToken,
    ) {
        self.workspace_prebuilt = completion.result.is_ok();
        let rendered = render_workspace_test_prebuild(completion, output);
        self.record(rendered, false, cancellation);
    }
}

impl<Rebalance: FnOnce(Budget) -> Budget> SetupRun<Rebalance> {
    #[expect(
        clippy::too_many_arguments,
        reason = "setup scheduling exposes the same bounded worker state as action scheduling"
    )]
    pub(super) fn advance<'scope, 'env: 'scope, R: Runner + 'env>(
        &mut self,
        scope: &'scope thread::Scope<'scope, 'env>,
        runner: &'scope R,
        nodes: &[NodeState],
        output: &mut impl Write,
        budget: Budget,
        cancellation: &CancellationToken,
        sender: &mpsc::Sender<WorkerCompletion>,
        running: &mut usize,
    ) -> Option<Budget> {
        if !self.structural_completed && structural_preflight_ready(nodes) {
            let completion = execute_structural_preflight(runner, cancellation);
            self.structural_completed = completion.result.is_ok();
            let rendered = render_structural_preflight(completion, output);
            self.record(rendered, true, cancellation);
        }
        if self.failed() {
            return None;
        }
        if !self.workspace_prebuild_started
            && workspace_test_prebuild_ready(nodes)
            && *running < budget.workers
        {
            if let Err(error) =
                start_workspace_test_prebuild(scope, runner, output, cancellation, budget, sender)
            {
                self.prebuild_failure = Some(VerifyError::Launch {
                    label: WORKSPACE_TEST_PREBUILD_LABEL,
                    source: error,
                });
                cancellation.cancel();
                return None;
            }
            self.workspace_prebuild_started = true;
            *running += 1;
        }
        if !self.workspace_prebuilt
            || !workspace_test_launch_slot_ready(nodes)
            || self.rebalance.is_none()
        {
            return None;
        }
        let rebalance = self
            .rebalance
            .take()
            .expect("workspace-test setup rebalances exactly once");
        let budget = budget.with_rebalanced_capacity(rebalance(budget));
        if let Err(error) = write_rebalanced_budget(output, budget) {
            self.output_fault = Some(error);
            cancellation.cancel();
            None
        } else {
            Some(budget)
        }
    }
}

fn structural_preflight_ready(nodes: &[NodeState]) -> bool {
    matches!(
        nodes.get(2),
        Some(NodeState::Complete(ActionCompletion { result: Ok(()), .. }))
    ) && matches!(nodes.get(3), Some(NodeState::Pending))
}

fn execute_structural_preflight<R: Runner>(
    runner: &R,
    cancellation: &CancellationToken,
) -> ActionCompletion {
    let started = Instant::now();
    let mut completion = execute_battery(
        runner.structural_preflight(cancellation),
        STRUCTURAL_PREFLIGHT_LABEL,
    );
    completion.elapsed = started.elapsed();
    completion
}

fn render_structural_preflight(
    completion: ActionCompletion,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    writeln!(
        output,
        "[verify] stage=setup identity={STRUCTURAL_PREFLIGHT_LABEL:?}"
    )
    .map_err(VerifyError::Output)?;
    output
        .write_all(&completion.output)
        .map_err(VerifyError::Output)?;
    writeln!(
        output,
        "[verify] stage=setup-terminal elapsed_ms={} disposition={} identity={STRUCTURAL_PREFLIGHT_LABEL:?}",
        completion.elapsed.as_millis(),
        completion.disposition,
    )
    .map_err(VerifyError::Output)?;
    completion.result
}

fn write_rebalanced_budget(output: &mut impl Write, budget: Budget) -> io::Result<()> {
    let active = budget
        .active_cargo_jobs
        .map_or_else(|| "unknown".to_owned(), |count| count.to_string());
    writeln!(
        output,
        concat!(
            "[verify] scheduler-rebalanced trigger=workspace-test-setup workers={} cargo_jobs={} ",
            "active_peer_cargo_jobs={} workspace_workers={} compiler_cargo_jobs={}"
        ),
        budget.workers,
        budget.cargo_jobs,
        active,
        budget.workspace_workers,
        budget.compiler_cargo_jobs,
    )?;
    output.flush()
}

fn workspace_test_prebuild_ready(nodes: &[NodeState]) -> bool {
    matches!(
        nodes.get(3),
        Some(NodeState::Complete(ActionCompletion { result: Ok(()), .. }))
    ) && matches!(nodes.get(7), Some(NodeState::Pending))
        && matches!(nodes.get(8), Some(NodeState::Pending))
}

fn workspace_test_launch_slot_ready(nodes: &[NodeState]) -> bool {
    matches!(
        nodes.get(5),
        Some(NodeState::Complete(ActionCompletion { result: Ok(()), .. }))
    )
}

fn start_workspace_test_prebuild<'scope, 'env: 'scope, R: Runner + 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    runner: &'scope R,
    output: &mut impl Write,
    cancellation: &CancellationToken,
    budget: Budget,
    sender: &mpsc::Sender<WorkerCompletion>,
) -> io::Result<()> {
    writeln!(
        output,
        "[verify] stage=setup identity={WORKSPACE_TEST_PREBUILD_LABEL:?}"
    )?;
    output.flush()?;
    let sender = sender.clone();
    let cancellation = cancellation.clone();
    thread::Builder::new()
        .name("verification-workspace-test-prebuild".to_owned())
        .spawn_scoped(scope, move || {
            let started = Instant::now();
            let mut completion = panic::catch_unwind(AssertUnwindSafe(|| {
                execute_workspace_test_prebuild(runner, &cancellation, budget)
            }))
            .unwrap_or_else(|payload| {
                failure(
                    VerifyError::WorkerPanic {
                        label: WORKSPACE_TEST_PREBUILD_LABEL,
                        detail: panic_detail(payload),
                    },
                    Disposition::Panic,
                )
            });
            completion.elapsed = started.elapsed();
            let _ = sender.send(WorkerCompletion::WorkspacePrebuild(completion));
        })
        .map(|_| ())
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
