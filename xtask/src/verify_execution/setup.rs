//! Synchronous setup gates interleaved with the verifier action graph.

use std::io::{self, Write};
use std::time::Instant;

use sill::CancellationToken;

use super::{
    ActionCompletion, Budget, NodeState, Runner, STRUCTURAL_PREFLIGHT_LABEL, VerifyError,
    WORKSPACE_TEST_PREBUILD_LABEL, execute_battery, execute_process,
};

pub(super) struct SetupRun<Rebalance> {
    structural_completed: bool,
    structural_failure: Option<VerifyError>,
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
            error if structural => self.structural_failure = Some(error),
            error => self.prebuild_failure = Some(error),
        }
        cancellation.cancel();
    }
}

impl<Rebalance: FnOnce(Budget) -> Budget> SetupRun<Rebalance> {
    pub(super) fn advance<R: Runner>(
        &mut self,
        runner: &R,
        nodes: &[NodeState],
        output: &mut impl Write,
        budget: Budget,
        cancellation: &CancellationToken,
    ) -> Option<Budget> {
        if !self.structural_completed && structural_preflight_ready(nodes) {
            let completion = execute_structural_preflight(runner, cancellation);
            self.structural_completed = completion.result.is_ok();
            let rendered = render_structural_preflight(completion, output);
            self.record(rendered, true, cancellation);
        }
        if self.failed() || self.workspace_prebuilt || !workspace_test_prebuild_ready(nodes) {
            return None;
        }
        let completion = execute_workspace_test_prebuild(runner, cancellation, budget);
        self.workspace_prebuilt = completion.result.is_ok();
        let rendered = render_workspace_test_prebuild(completion, output);
        self.record(rendered, false, cancellation);
        if self.failed() || !self.workspace_prebuilt {
            return None;
        }
        let rebalance = self
            .rebalance
            .take()
            .expect("workspace-test setup rebalances exactly once");
        let budget = rebalance(budget);
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
            "active_peer_cargo_jobs={} workspace_workers={}"
        ),
        budget.workers, budget.cargo_jobs, active, budget.workspace_workers,
    )?;
    output.flush()
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
