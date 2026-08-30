//! One fail-closed ordinary gate over exact staged-change evidence.

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gate::{
    ArmDisposition, ArmFamily, BatteryArmResult, BatteryManifest, BatteryState, ChangeReachObject,
    ChangeSelectionReport, GateId, Selector, StateRunScope, TestSelectionReport,
};
use serde::{Deserialize, Serialize};
use sill::Environment;

use crate::repository::Repository;

mod cargo_budget;
mod progress;
mod test_execution;

use cargo_budget::CargoBudget;
use progress::{SharedProgress, publish_test_selection};
use test_execution::{TestExecution, TestRun, run_selected_tests};

/// Current combined ordinary-change-gate report schema.
pub const CHANGE_GATE_REPORT_VERSION: u64 = 4;

/// One initial compare-and-swap plus bounded fresh-authority retries. The
/// enclosing pre-commit supervisor remains the wall-clock authority; this cap
/// prevents a continuously moving ledger from creating an unbounded inner
/// retry loop.
const STATE_PUBLICATION_ATTEMPTS: usize = 4;

/// Policy evidence shaped to the selector's exact authority scope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "coverage")]
pub enum ChangePolicyReport {
    /// Minimal source-bound summaries for a locally understood change.
    Focused {
        source: wire::Digest,
        results: Vec<PolicyArmSummary>,
    },
}

impl ChangePolicyReport {
    fn focused_summaries(source: &wire::Digest, results: Vec<PolicyArmSummary>) -> Self {
        Self::Focused {
            source: source.clone(),
            results,
        }
    }

    /// Number of canonical policy arms retained by this report.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Focused { results, .. } => results.len(),
        }
    }

    /// Whether this report retains no policy arms.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Canonical policy results in selected manifest order.
    pub fn iter(&self) -> std::slice::Iter<'_, PolicyArmSummary> {
        match self {
            Self::Focused { results, .. } => results.iter(),
        }
    }

    fn admits(&self) -> bool {
        self.iter().all(|result| result.state.admits())
    }

    fn validate(&self, gate_ids: &[GateId], source: &wire::Digest) -> Result<(), ChangeGateError> {
        match self {
            Self::Focused {
                source: observed,
                results,
            } => {
                validate_policy_results(results, gate_ids)?;
                validate_policy_source(observed, source, "focused")?;
            }
        }
        Ok(())
    }
}

fn validate_policy_source(
    observed: &wire::Digest,
    expected: &wire::Digest,
    coverage: &str,
) -> Result<(), ChangeGateError> {
    if observed != expected {
        return Err(ChangeGateError::protocol(
            "change-gate-source-moved",
            format!("{coverage} policy summary observed a different staged source"),
        ));
    }
    Ok(())
}

fn validate_policy_results(
    results: &[PolicyArmSummary],
    gate_ids: &[GateId],
) -> Result<(), ChangeGateError> {
    if results.len() != gate_ids.len() {
        return Err(ChangeGateError::protocol(
            "change-gate-policy-incomplete",
            "policy summary does not exactly cover the selected arm inventory",
        ));
    }
    let manifest = BatteryManifest::canonical()
        .map_err(|error| ChangeGateError::protocol("change-gate-policy-manifest-invalid", error))?;
    for (result, gate_id) in results.iter().zip(gate_ids) {
        let Some(arm) = manifest.arms().get(result.position) else {
            return Err(ChangeGateError::protocol(
                "change-gate-policy-incomplete",
                format!(
                    "policy summary position {} is not canonical",
                    result.position
                ),
            ));
        };
        let ArmDisposition::Compiled {
            gate_id: expected_gate,
            family: expected_family,
            declaration: expected_declaration,
        } = arm.disposition();
        if result.gate_id != *gate_id
            || result.gate_id != *expected_gate
            || result.family != *expected_family
            || result.declaration != *expected_declaration
        {
            return Err(ChangeGateError::protocol(
                "change-gate-policy-incomplete",
                format!(
                    "policy summary position {} disagrees with the canonical manifest",
                    result.position,
                ),
            ));
        }
        if result.reason.trim().is_empty() || result.reason.contains(['\0', '\r']) {
            return Err(ChangeGateError::protocol(
                "change-gate-policy-invalid",
                format!("policy summary {} has an invalid reason", result.gate_id),
            ));
        }
    }
    Ok(())
}

/// Compact terminal identity for one fully validated policy result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyArmSummary {
    position: usize,
    gate_id: GateId,
    family: ArmFamily,
    declaration: String,
    state: BatteryState,
    reason: String,
    authority: wire::Digest,
}

impl PolicyArmSummary {
    fn from_result(result: &BatteryArmResult, authority: wire::Digest) -> Self {
        Self {
            position: result.position(),
            gate_id: result.gate_id(),
            family: result.family(),
            declaration: result.declaration().to_owned(),
            state: result.state(),
            reason: result.reason().to_owned(),
            authority,
        }
    }

    /// Canonical gate identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.gate_id
    }

    /// Terminal state from the validated policy result.
    #[must_use]
    pub const fn state(&self) -> BatteryState {
        self.state
    }

    /// Exact bounded terminal reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Canonical selected tests and scope-shaped policy evidence for one staged source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeGateReport {
    version: u64,
    selection: ChangeSelectionReport,
    tests: TestSelectionReport,
    policy: ChangePolicyReport,
}

impl ChangeGateReport {
    fn new(
        selection: ChangeSelectionReport,
        tests: TestSelectionReport,
        policy: ChangePolicyReport,
    ) -> Result<Self, ChangeGateError> {
        let report = Self {
            version: CHANGE_GATE_REPORT_VERSION,
            selection,
            tests,
            policy,
        };
        report.validate()?;
        Ok(report)
    }

    /// Current combined report schema.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Exact staged-path-to-policy selection evidence.
    #[must_use]
    pub const fn selection(&self) -> &ChangeSelectionReport {
        &self.selection
    }

    /// Exact staged-source-to-test-engine execution evidence.
    #[must_use]
    pub const fn tests(&self) -> &TestSelectionReport {
        &self.tests
    }

    /// Canonical policy evidence in selected manifest order.
    #[must_use]
    pub const fn policy(&self) -> &ChangePolicyReport {
        &self.policy
    }

    /// Whether every selected test and policy arm admitted.
    #[must_use]
    pub fn admits(&self) -> bool {
        self.tests.admits() && self.policy.admits()
    }

    /// Compact deterministic evidence for selected policy refusals.
    #[must_use]
    pub fn refusal_fold(&self) -> String {
        let mut refusal = String::new();
        for result in self.policy.iter().filter(|result| !result.state().admits()) {
            let _ = writeln!(
                refusal,
                "gate={} state={} reason={}",
                result.gate_id(),
                result.state().as_str(),
                result.reason(),
            );
        }
        refusal
    }

    /// Canonical JSON bytes for this ordinary-gate report.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only canonical, source-consistent selected evidence.
    pub fn decode(bytes: &[u8]) -> Result<Self, ChangeGateError> {
        let report: Self = wire::decode(bytes)
            .map_err(|error| ChangeGateError::protocol("change-gate-report-invalid", error))?;
        report.validate()?;
        let canonical = report
            .encode()
            .map_err(|error| ChangeGateError::protocol("change-gate-report-unencodable", error))?;
        if canonical != bytes {
            return Err(ChangeGateError::protocol(
                "change-gate-report-noncanonical",
                "ordinary change-gate report bytes are not canonical",
            ));
        }
        Ok(report)
    }

    fn validate(&self) -> Result<(), ChangeGateError> {
        if self.version != CHANGE_GATE_REPORT_VERSION {
            return Err(ChangeGateError::protocol(
                "change-gate-report-version",
                format!(
                    "unsupported ordinary change-gate version {}; expected {CHANGE_GATE_REPORT_VERSION}",
                    self.version,
                ),
            ));
        }
        let source = self.selection.source();
        if self.tests.source() != source {
            return Err(ChangeGateError::protocol(
                "change-gate-source-mismatch",
                "test selection and policy selection name different staged sources",
            ));
        }
        if self
            .tests
            .changed_sources()
            .iter()
            .any(|changed| !self.selection.changed_sources().contains(changed))
        {
            return Err(ChangeGateError::protocol(
                "change-gate-change-mismatch",
                "test selection names a source absent from policy selection",
            ));
        }
        self.policy.validate(self.selection.gate_ids(), source)?;
        Ok(())
    }
}

struct ChangeGateExecution {
    selected_root: PathBuf,
    writable_root: PathBuf,
    test_source: wire::Digest,
    test_changes: Vec<String>,
    test_execution: TestExecution,
    budget: CargoBudget,
    environment: Environment,
}

struct PartialStateContext<'a> {
    selection: &'a ChangeSelectionReport,
    source: &'a wire::Digest,
    surface: SelectionSurface,
    budget: CargoBudget,
}

struct PackagedPolicy {
    selection: gate::ChangeSelectionPolicy,
    test_engines: Vec<crate::config::TestEngine>,
}

/// Execute the ordinary staged-change gate without an ambient repository-wide selector.
///
/// The staging arm owns the first immutable capture. Its exact changes feed the
/// pure policy selector and every explicitly enabled packaged test engine.
/// Every other selected arm is a focused canonical run whose source digest
/// must match that capture.
///
/// # Errors
///
/// Returns repository/environment faults, selected-source movement, incomplete
/// test reach, or canonical focused-arm acquisition failures.
pub fn run(progress: &mut (impl Write + Send)) -> Result<ChangeGateReport, ChangeGateError> {
    run_with_selection(progress, SelectionSurface::Packaged)
}

/// Execute the richer current-source selector from the explicit xtask rim.
///
/// Every capture uses the strict self-hosting inputs: both tracked contract
/// documents and the complete compiled tool preflight are required.
///
/// # Errors
///
/// Returns the same staged selection, test, or policy failures as [`run`].
pub fn run_current_source(
    progress: &mut (impl Write + Send),
) -> Result<ChangeGateReport, ChangeGateError> {
    run_with_selection(progress, SelectionSurface::CurrentSource)
}

/// Run the packaged selector over the resolved index captured for a merge.
///
/// Resolution derives its selection from the same staged source and compiled
/// reach policy as an ordinary commit. It reads no retained verifier state.
pub fn run_resolution(
    progress: &mut (impl Write + Send),
) -> Result<ChangeGateReport, ChangeGateError> {
    run_with_selection(progress, SelectionSurface::Resolution)
}
#[derive(Clone, Copy)]
enum SelectionSurface {
    Packaged,
    CurrentSource,
    Resolution,
}

impl SelectionSurface {
    fn prepare_anchor(self) -> Result<crate::gate::PreparedArm<'static>, crate::gate::RunError> {
        match self {
            Self::Packaged | Self::Resolution => {
                crate::gate::prepare_arm(Selector::staged(), GateId::Staging)
            }
            Self::CurrentSource => {
                crate::gate::prepare_arm_strict(Selector::staged(), GateId::Staging)
            }
        }
    }
}

fn run_with_selection(
    progress: &mut (impl Write + Send),
    surface: SelectionSurface,
) -> Result<ChangeGateReport, ChangeGateError> {
    let total_started = Instant::now();
    publish_phase(progress, "capture", None)?;
    let capture_started = Instant::now();
    let budget = CargoBudget::detect(gate::WORKSPACE_TEST_CONCURRENCY_CAP);
    publish_budget(progress, budget, "overlapped-policy-tests")?;
    let mut anchor = surface.prepare_anchor().map_err(ChangeGateError::gate)?;
    publish_phase_terminal(progress, "capture", capture_started, "success")?;
    publish_phase(progress, "selection", Some(anchor.changes().len()))?;
    let selection_started = Instant::now();
    let (selection, test_execution) = select_changes(&anchor, surface)?;
    publish_selection(progress, &selection)?;
    publish_phase_terminal(progress, "selection", selection_started, "success")?;

    let repository = Repository::discover(Path::new(".")).map_err(|error| {
        ChangeGateError::environment("change-gate-repository-unresolved", error)
    })?;
    let execution = ChangeGateExecution {
        selected_root: anchor.root().to_path_buf(),
        writable_root: repository.worktree_root().to_path_buf(),
        test_source: anchor.source().clone(),
        test_changes: anchor.changes().to_vec(),
        test_execution,
        budget,
        environment: ambient_environment().map_err(|error| {
            ChangeGateError::environment("change-gate-environment-unavailable", error)
        })?,
    };
    let source = selection.source().clone();
    let mut shared_progress = SharedProgress::new(progress);
    let execution_started = Instant::now();
    let (tests, policy) = run_policy_and_tests(
        &mut anchor,
        &selection,
        &source,
        execution,
        &mut shared_progress,
    )?;
    publish_phase_terminal(
        &mut shared_progress,
        "policy-tests",
        execution_started,
        "success",
    )?;
    let publication_started = Instant::now();
    publish_test_selection(&mut shared_progress, &tests)?;
    if !tests.admits() {
        let evidence = String::from_utf8(
            tests
                .encode()
                .map_err(|error| ChangeGateError::protocol("test-selection-unencodable", error))?,
        )
        .map_err(|error| ChangeGateError::protocol("test-selection-non-utf8", error))?;
        return Err(ChangeGateError::refusal(
            "change-gate-test-coverage-unknown",
            evidence,
        ));
    }

    let policy = settle_partial_state(
        &mut anchor,
        PartialStateContext {
            selection: &selection,
            source: &source,
            surface,
            budget,
        },
        policy,
        &mut shared_progress,
    )?;
    let report = ChangeGateReport::new(selection, tests, policy)?;
    publish_phase_terminal(
        &mut shared_progress,
        "publication",
        publication_started,
        "success",
    )?;
    publish_phase_terminal(&mut shared_progress, "complete", total_started, "success")?;
    Ok(report)
}

fn settle_partial_state<W: Write + Send + ?Sized>(
    anchor: &mut crate::gate::PreparedArm<'static>,
    context: PartialStateContext<'_>,
    mut policy: ChangePolicyReport,
    progress: &mut SharedProgress<'_, W>,
) -> Result<ChangePolicyReport, ChangeGateError> {
    for attempt in 1..=STATE_PUBLICATION_ATTEMPTS {
        if !policy.admits() {
            anchor.reject_state().map_err(ChangeGateError::gate)?;
            return Ok(policy);
        }
        match anchor.accept_state(StateRunScope::Partial) {
            Ok(()) => return Ok(policy),
            Err(error)
                if error.is_stale_state_publication() && attempt < STATE_PUBLICATION_ATTEMPTS =>
            {
                writeln!(
                    progress,
                    "[change-gate-state] disposition=stale-retry publication_attempt={attempt}",
                )
                .map_err(|error| {
                    ChangeGateError::environment("change-gate-progress-unavailable", error)
                })?;
                progress.flush().map_err(|error| {
                    ChangeGateError::environment("change-gate-progress-unavailable", error)
                })?;

                let mut retry = context
                    .surface
                    .prepare_anchor()
                    .map_err(ChangeGateError::gate)?;
                let (retry_selection, _) = select_changes(&retry, context.surface)?;
                if &retry_selection != context.selection || retry.source() != context.source {
                    return Err(ChangeGateError::refusal(
                        "change-gate-source-moved",
                        "staged source or selected policy changed while retrying state publication",
                    ));
                }
                let focused = run_focused_policy(
                    &mut retry,
                    context.selection,
                    context.source,
                    context.budget,
                    BTreeMap::new(),
                    progress,
                )?;
                policy = ChangePolicyReport::focused_summaries(context.source, focused.summaries);
                policy.validate(context.selection.gate_ids(), context.source)?;
                *anchor = retry;
            }
            Err(error) => return Err(ChangeGateError::gate(error)),
        }
    }
    unreachable!("the fixed state publication attempt loop always returns")
}

fn select_changes(
    anchor: &crate::gate::PreparedArm<'_>,
    surface: SelectionSurface,
) -> Result<(ChangeSelectionReport, TestExecution), ChangeGateError> {
    let (selection, test_execution) =
        match surface {
            SelectionSurface::Packaged | SelectionSurface::Resolution => {
                let policy = packaged_policy(anchor.root(), anchor.paths())?;
                let selection = gate::select_change_gates(
                    anchor.source().clone(),
                    anchor.changes(),
                    &policy.selection,
                )
                .and_then(|selection| {
                    gate::compile_language_reach(
                        anchor.root(),
                        anchor.paths(),
                        policy.selection.package_roots(),
                        policy.selection.test_paths(),
                        selection,
                    )
                });
                (
                    selection,
                    TestExecution::Packaged {
                        engines: policy.test_engines,
                    },
                )
            }
            SelectionSurface::CurrentSource => {
                let selection = gate::select_current_source_change_gates(
                    anchor.source().clone(),
                    anchor.changes(),
                )
                .and_then(|selection| {
                    gate::compile_language_reach(anchor.root(), anchor.paths(), &[], &[], selection)
                });
                (selection, TestExecution::CurrentSourceCargo)
            }
        };
    let selection =
        selection.map_err(|error| ChangeGateError::refusal("change-selection-refused", error))?;
    Ok((selection, test_execution))
}

fn publish_selection(
    progress: &mut (impl Write + Send),
    selection: &ChangeSelectionReport,
) -> Result<(), ChangeGateError> {
    writeln!(
        progress,
        "[change-gate-selection] mode={:?} changes={} policy_arms={}",
        selection.mode(),
        selection.changed_sources().len(),
        selection.gate_ids().len(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    writeln!(
        progress,
        "[change-gate-selection-report] detail=canonical-report source={} changes={} arms={} edges={}",
        selection.source(),
        selection.changed_sources().len(),
        selection.gate_ids().len(),
        selection.edges().len(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

fn publish_phase(
    progress: &mut (impl Write + Send),
    stage: &str,
    changes: Option<usize>,
) -> Result<(), ChangeGateError> {
    match changes {
        Some(changes) => writeln!(
            progress,
            "[change-gate-phase] stage={stage} changes={changes}"
        ),
        None => writeln!(progress, "[change-gate-phase] stage={stage}"),
    }
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

fn publish_phase_terminal(
    progress: &mut (impl Write + Send + ?Sized),
    stage: &str,
    started: Instant,
    disposition: &str,
) -> Result<(), ChangeGateError> {
    writeln!(
        progress,
        "[change-gate-phase] stage={stage}-terminal elapsed_ms={} disposition={disposition}",
        started.elapsed().as_millis(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

fn publish_budget(
    progress: &mut (impl Write + Send),
    budget: CargoBudget,
    scheduling: &str,
) -> Result<(), ChangeGateError> {
    writeln!(
        progress,
        concat!(
            "[change-gate-budget] cpus={} policy_jobs={} ",
            "test_concurrency={} shard_jobs={} test_threads={} scheduling={}"
        ),
        budget.cpus(),
        budget.policy_jobs(),
        budget.test_concurrency(),
        budget.shard_jobs(),
        budget.test_threads(),
        scheduling,
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

#[cfg(test)]
fn packaged_selection_policy(
    root: &Path,
    tracked_sources: &[String],
) -> Result<gate::ChangeSelectionPolicy, ChangeGateError> {
    packaged_policy(root, tracked_sources).map(|policy| policy.selection)
}

fn packaged_policy(
    root: &Path,
    tracked_sources: &[String],
) -> Result<PackagedPolicy, ChangeGateError> {
    const TRACKED_POLICY: &str = "spice-rust.toml";

    let path = root.join(TRACKED_POLICY);
    let tracked = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ChangeGateError::environment(
                "change-selection-policy-unavailable",
                format!("{}: {error}", path.display()),
            ));
        }
    };
    let document = tracked
        .as_deref()
        .map(|text| crate::config::Document::new(TRACKED_POLICY, text));
    let resolved = crate::config::resolve(crate::config::Inputs::new(document, None, &[]))
        .map_err(|error| ChangeGateError::refusal("change-selection-policy-refused", error))?;
    let policy = resolved.root().policy();
    let selection = gate::ChangeSelectionPolicy::new(gate::ChangeSelectionPolicyInputs {
        tracked_sources,
        policy_authorities: &[TRACKED_POLICY.to_owned(), "baseline.toml".to_owned()],
        configured: tracked.is_some(),
        repository_documents: policy.repo_truth().docs(),
        file_shape_suffixes: policy.file_shape().source_suffixes(),
        complexity_suffixes: policy.languages().complexity(),
        magic_suffixes: policy.languages().magic(),
        package_roots: policy.package_roots(),
        generated_paths: policy.generated_paths(),
        test_paths: policy.test_paths(),
    })
    .map_err(|error| ChangeGateError::refusal("change-selection-policy-refused", error))?;
    Ok(PackagedPolicy {
        selection,
        test_engines: policy.test_engines().to_vec(),
    })
}

fn run_policy_and_tests<W: Write + Send + ?Sized>(
    anchor: &mut crate::gate::PreparedArm<'_>,
    selection: &ChangeSelectionReport,
    source: &wire::Digest,
    execution: ChangeGateExecution,
    progress: &mut SharedProgress<'_, W>,
) -> Result<(TestSelectionReport, ChangePolicyReport), ChangeGateError> {
    let ChangeGateExecution {
        selected_root,
        writable_root,
        test_source,
        test_changes,
        test_execution,
        budget,
        environment,
    } = execution;
    let preexecuted = preexecute_policy_screen(anchor, selection, source, budget, progress)?;
    let test_run = TestRun::new(
        &selected_root,
        &writable_root,
        test_source,
        &test_changes,
        budget,
        environment,
    );
    let test_run = match preexecuted.rust_workspace.as_ref() {
        Some(workspace) => test_run.with_reusable(workspace),
        None => test_run,
    };
    let mut test_progress = progress.clone();
    let (focused, tests) = std::thread::scope(|scope| {
        let tests = scope.spawn(move || {
            let started = Instant::now();
            let result = run_selected_tests(test_run, test_execution, &mut test_progress);
            let disposition = if result.is_ok() { "success" } else { "failed" };
            publish_phase_terminal(&mut test_progress, "tests", started, disposition)?;
            result
        });
        let policy_started = Instant::now();
        let focused = run_focused_policy(
            anchor,
            selection,
            source,
            budget,
            preexecuted.reports,
            progress,
        );
        let policy_disposition = if focused.is_ok() { "success" } else { "failed" };
        publish_phase_terminal(
            progress,
            "policy-remainder",
            policy_started,
            policy_disposition,
        )?;
        let tests = tests.join().map_err(|_| {
            ChangeGateError::protocol(
                "change-gate-tests-panicked",
                "selected workspace-test execution panicked",
            )
        })?;
        Ok::<_, ChangeGateError>((focused?, tests?))
    })?;
    let policy = ChangePolicyReport::focused_summaries(source, focused.summaries);
    Ok((tests, policy))
}

struct FocusedPolicyExecution {
    summaries: Vec<PolicyArmSummary>,
}

struct PreexecutedPolicy {
    reports: BTreeMap<GateId, gate::FocusedArmReport>,
    rust_workspace: Option<gate::RustAnalysisWorkspace>,
}

fn preexecute_policy_screen<W: Write + Send + ?Sized>(
    anchor: &mut crate::gate::PreparedArm<'_>,
    selection: &ChangeSelectionReport,
    source: &wire::Digest,
    budget: CargoBudget,
    progress: &mut SharedProgress<'_, W>,
) -> Result<PreexecutedPolicy, ChangeGateError> {
    let mut prepared = Vec::new();
    for reach in selection
        .reach()
        .iter()
        .filter(|reach| pretest_screen_gate(reach.gate_id()))
    {
        let gate_id = reach.gate_id();
        publish_policy_start(progress, gate_id)?;
        let started = Instant::now();
        let arm = prepare_policy_arm(anchor, selection, reach, budget, progress)?;
        prepared.push((gate_id, started, arm));
    }

    let executed = std::thread::scope(|scope| {
        let handles = prepared
            .into_iter()
            .map(|(gate_id, started, mut arm)| {
                let handle = scope.spawn(move || {
                    let report = arm.execute().map_err(ChangeGateError::gate)?;
                    let workspace = (gate_id == GateId::RustTypecheck)
                        .then(|| arm.reusable_rust_workspace().cloned())
                        .flatten();
                    Ok::<_, ChangeGateError>((report, workspace))
                });
                (gate_id, started, handle)
            })
            .collect::<Vec<_>>();
        let mut executed = Vec::with_capacity(handles.len());
        for (gate_id, started, handle) in handles {
            let result = handle.join().map_err(|_| {
                ChangeGateError::protocol(
                    "change-gate-policy-panicked",
                    format!("focused {} policy arm panicked", gate_id.as_str()),
                )
            })?;
            let disposition = match &result {
                Ok((report, _)) if report.admits() => "completed",
                Ok(_) => "refused",
                Err(_) => "failed",
            };
            publish_policy_terminal(progress, gate_id, started, disposition)?;
            let (report, workspace) = result?;
            executed.push((gate_id, report, workspace));
        }
        Ok::<_, ChangeGateError>(executed)
    })?;

    let mut reports = BTreeMap::new();
    let mut rust_workspace = None;
    for (gate_id, report, workspace) in executed {
        validate_focused_policy_source(&report, source)?;
        if gate_id == GateId::RustTypecheck {
            rust_workspace = workspace;
        }
        reports.insert(gate_id, report);
    }
    if reports.values().any(|report| !report.admits()) {
        anchor.reject_state().map_err(ChangeGateError::gate)?;
        let reports = reports.into_values().collect::<Vec<_>>();
        return Err(ChangeGateError::refusal(
            "change-gate-policy-regressed",
            gate::BatteryRefusalFold::from_focused_reports(&reports),
        ));
    }
    Ok(PreexecutedPolicy {
        reports,
        rust_workspace,
    })
}

const fn pretest_screen_gate(gate_id: GateId) -> bool {
    matches!(
        gate_id,
        GateId::RustTypecheck | GateId::CanvasWebTypecheck | GateId::Complexity
    )
}

fn run_focused_policy(
    anchor: &mut crate::gate::PreparedArm<'_>,
    selection: &ChangeSelectionReport,
    source: &wire::Digest,
    budget: CargoBudget,
    mut preexecuted: BTreeMap<GateId, gate::FocusedArmReport>,
    progress: &mut impl Write,
) -> Result<FocusedPolicyExecution, ChangeGateError> {
    let mut policy = Vec::with_capacity(selection.reach().len());
    let mut reaches = selection.reach().iter().peekable();
    while let Some(reach) = reaches.next() {
        let gate_id = reach.gate_id();
        if let Some(report) = preexecuted.remove(&gate_id) {
            record_policy_report(&mut policy, report, source)?;
            continue;
        }
        publish_policy_start(progress, gate_id)?;
        let started = Instant::now();
        if gate_id == GateId::RustTypecheck
            && reaches
                .peek()
                .is_some_and(|next| next.gate_id() == GateId::CanvasWebTypecheck)
        {
            let rust = prepare_policy_arm(anchor, selection, reach, budget, progress)?;
            let canvas_reach = reaches
                .next()
                .expect("the adjacent Canvas reach was observed above");
            publish_policy_start(progress, GateId::CanvasWebTypecheck)?;
            let canvas = prepare_policy_arm(anchor, selection, canvas_reach, budget, progress)?;
            let (rust, canvas) = std::thread::scope(|scope| {
                let canvas = scope.spawn(|| execute_prepared_policy_arm(canvas));
                let rust = execute_prepared_policy_arm(rust);
                let canvas = canvas.join().map_err(|_| {
                    ChangeGateError::protocol(
                        "change-gate-policy-panicked",
                        "focused Canvas policy arm panicked",
                    )
                })?;
                Ok::<_, ChangeGateError>((rust?, canvas?))
            })?;
            publish_policy_terminal(progress, GateId::RustTypecheck, started, "completed")?;
            publish_policy_terminal(progress, GateId::CanvasWebTypecheck, started, "completed")?;
            record_policy_report(&mut policy, rust, source)?;
            record_policy_report(&mut policy, canvas, source)?;
            continue;
        }

        let report = if gate_id == GateId::Staging {
            anchor
                .bind_cargo_jobs(budget.policy_jobs())
                .map_err(ChangeGateError::gate)?;
            publish_execution_scope(
                progress,
                gate_id,
                reach.objects(),
                &[ChangeReachObject::Repository],
            )?;
            anchor.execute().map_err(ChangeGateError::gate)
        } else {
            let prepared = prepare_policy_arm(anchor, selection, reach, budget, progress)?;
            execute_prepared_policy_arm(prepared)
        };
        let disposition = if report.is_ok() {
            "completed"
        } else {
            "failed"
        };
        publish_policy_terminal(progress, gate_id, started, disposition)?;
        let report = report?;
        record_policy_report(&mut policy, report, source)?;
    }
    Ok(FocusedPolicyExecution { summaries: policy })
}

fn publish_policy_start(progress: &mut impl Write, gate_id: GateId) -> Result<(), ChangeGateError> {
    writeln!(
        progress,
        "[change-gate-policy] gate={} disposition=executed",
        gate_id.as_str(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

fn publish_policy_terminal(
    progress: &mut impl Write,
    gate_id: GateId,
    started: Instant,
    disposition: &str,
) -> Result<(), ChangeGateError> {
    writeln!(
        progress,
        "[change-gate-policy] gate={} stage=terminal elapsed_ms={} disposition={disposition}",
        gate_id.as_str(),
        started.elapsed().as_millis(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

fn prepare_policy_arm(
    anchor: &crate::gate::PreparedArm<'_>,
    selection: &ChangeSelectionReport,
    reach: &gate::ChangeArmReach,
    budget: CargoBudget,
    progress: &mut impl Write,
) -> Result<crate::gate::PreparedArm<'static>, ChangeGateError> {
    let gate_id = reach.gate_id();
    let mut prepared = anchor
        .derive_reached_arm(selection, gate_id)
        .map_err(ChangeGateError::gate)?;
    prepared
        .bind_cargo_jobs(budget.policy_jobs())
        .map_err(ChangeGateError::gate)?;
    let execution_objects = prepared.execution_objects().ok_or_else(|| {
        ChangeGateError::protocol(
            "change-gate-reach-unbound",
            format!(
                "focused arm {} retained no execution object inventory",
                gate_id.as_str()
            ),
        )
    })?;
    publish_execution_scope(progress, gate_id, reach.objects(), execution_objects)?;
    Ok(prepared)
}

fn execute_prepared_policy_arm(
    mut prepared: crate::gate::PreparedArm<'static>,
) -> Result<gate::FocusedArmReport, ChangeGateError> {
    prepared.execute().map_err(ChangeGateError::gate)
}

fn record_policy_report(
    policy: &mut Vec<PolicyArmSummary>,
    report: gate::FocusedArmReport,
    source: &wire::Digest,
) -> Result<(), ChangeGateError> {
    validate_focused_policy_source(&report, source)?;
    let encoded = report.encode().map_err(|error| {
        ChangeGateError::protocol("change-gate-focused-policy-unencodable", error)
    })?;
    policy.push(PolicyArmSummary::from_result(
        report.result(),
        wire::Digest::compute(wire::Domain::GateFocusedReport, &encoded),
    ));
    Ok(())
}

fn validate_focused_policy_source(
    report: &gate::FocusedArmReport,
    source: &wire::Digest,
) -> Result<(), ChangeGateError> {
    if report.context().source() != source {
        return Err(ChangeGateError::refusal(
            "change-gate-source-moved",
            format!(
                "staged source changed while executing focused arm {}",
                report.gate_id().as_str(),
            ),
        ));
    }
    Ok(())
}

fn publish_execution_scope(
    progress: &mut impl Write,
    gate_id: GateId,
    planned: &[ChangeReachObject],
    executed: &[ChangeReachObject],
) -> Result<(), ChangeGateError> {
    if executed != planned {
        return Err(ChangeGateError::protocol(
            "change-gate-reach-mismatch",
            format!(
                "focused arm {} bound an execution inventory different from its reach plan",
                gate_id.as_str()
            ),
        ));
    }
    let objects = serde_json::to_string(executed)
        .map_err(|error| ChangeGateError::protocol("change-gate-reach-unencodable", error))?;
    writeln!(
        progress,
        "[change-gate-execution] gate={} objects={objects}",
        gate_id.as_str(),
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the ordinary change-gate command rim captures the caller environment once"
)]
fn ambient_environment() -> Result<Environment, sill::SpecError> {
    Environment::new(env::vars_os())
}

/// Fail-closed ordinary-gate execution or report failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeGateError {
    reason: String,
    detail: String,
    fault: bool,
}

impl ChangeGateError {
    fn gate(error: crate::gate::RunError) -> Self {
        Self {
            reason: error.reason().to_owned(),
            detail: error.detail().to_owned(),
            fault: error.is_fault(),
        }
    }

    fn refusal(reason: &str, detail: impl fmt::Display) -> Self {
        Self {
            reason: format!("gate::{reason}"),
            detail: detail.to_string(),
            fault: false,
        }
    }

    fn protocol(reason: &str, detail: impl fmt::Display) -> Self {
        Self {
            reason: format!("gate::{reason}"),
            detail: detail.to_string(),
            fault: true,
        }
    }

    fn environment(reason: &str, detail: impl fmt::Display) -> Self {
        Self {
            reason: format!("gate::{reason}"),
            detail: detail.to_string(),
            fault: true,
        }
    }

    /// Stable refusal or fault identity.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Exact retained evidence.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Whether this is an environmental/protocol fault rather than a refusal.
    #[must_use]
    pub const fn is_fault(&self) -> bool {
        self.fault
    }
}

impl fmt::Display for ChangeGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.detail)
    }
}

impl Error for ChangeGateError {}

#[cfg(test)]
#[path = "change_gate_tests.rs"]
mod tests;
