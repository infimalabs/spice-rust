//! One fail-closed ordinary gate over exact staged-change evidence.

use std::env;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use gate::{
    ArmDisposition, ArmFamily, BatteryArmResult, BatteryManifest, BatteryState,
    ChangeSelectionReport, FocusedArmReport, GateId, Selector, StateRunScope, TestSelectionReport,
    WORKSPACE_TEST_CONCURRENCY_CAP, WorkspaceTestRoots,
};
use serde::{Deserialize, Serialize};
use sill::Environment;

use crate::repository::Repository;

/// Current combined ordinary-change-gate report schema.
pub const CHANGE_GATE_REPORT_VERSION: u64 = 2;

struct SharedProgress<'a, W: Write + Send + ?Sized> {
    inner: Arc<Mutex<&'a mut W>>,
}

impl<'a, W: Write + Send + ?Sized> SharedProgress<'a, W> {
    fn new(progress: &'a mut W) -> Self {
        Self {
            inner: Arc::new(Mutex::new(progress)),
        }
    }

    fn sink(&self) -> io::Result<std::sync::MutexGuard<'_, &'a mut W>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("change-gate progress lock was poisoned"))
    }
}

impl<W: Write + Send + ?Sized> Clone for SharedProgress<'_, W> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<W: Write + Send + ?Sized> Write for SharedProgress<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.sink()?.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink()?.flush()
    }

    fn write_fmt(&mut self, arguments: fmt::Arguments<'_>) -> io::Result<()> {
        self.sink()?.write_fmt(arguments)
    }
}

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
    fn focused(
        source: &wire::Digest,
        reports: &[FocusedArmReport],
    ) -> Result<Self, ChangeGateError> {
        let mut results = Vec::with_capacity(reports.len());
        for report in reports {
            if report.context().source() != source {
                return Err(ChangeGateError::refusal(
                    "change-gate-source-moved",
                    format!(
                        "staged source changed while summarizing focused arm {}",
                        report.gate_id(),
                    ),
                ));
            }
            let encoded = report.encode().map_err(|error| {
                ChangeGateError::protocol("change-gate-focused-policy-unencodable", error)
            })?;
            let digest = wire::Digest::compute(wire::Domain::GateFocusedReport, &encoded);
            results.push(PolicyArmSummary::from_result(report.result(), digest));
        }
        Ok(Self::Focused {
            source: source.clone(),
            results,
        })
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

    /// Exact staged-build-source-to-test execution evidence.
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
    environment: Environment,
}

/// Execute the ordinary staged-change gate without an ambient repository-wide selector.
///
/// The staging arm owns the first immutable capture. Its exact changes feed the
/// pure policy selector and canonical Cargo target reach. Every other selected
/// arm is a focused canonical run whose source digest must match that capture.
///
/// # Errors
///
/// Returns repository/environment faults, selected-source movement, incomplete
/// test reach, or canonical focused-arm acquisition failures.
pub fn run(progress: &mut (impl Write + Send)) -> Result<ChangeGateReport, ChangeGateError> {
    let mut anchor = crate::gate::prepare_arm(Selector::staged(), GateId::Staging)
        .map_err(ChangeGateError::gate)?;
    let selection = gate::select_change_gates(anchor.source().clone(), anchor.changes())
        .map_err(|error| ChangeGateError::refusal("change-selection-refused", error))?;
    publish_selection(progress, &selection)?;

    let repository = Repository::discover(Path::new(".")).map_err(|error| {
        ChangeGateError::environment("change-gate-repository-unresolved", error)
    })?;
    let execution = ChangeGateExecution {
        selected_root: anchor.root().to_path_buf(),
        writable_root: repository.worktree_root().to_path_buf(),
        test_source: anchor.source().clone(),
        test_changes: anchor.changes().to_vec(),
        environment: ambient_environment().map_err(|error| {
            ChangeGateError::environment("change-gate-environment-unavailable", error)
        })?,
    };
    let source = selection.source().clone();
    let mut shared_progress = SharedProgress::new(progress);
    let (tests, policy) = run_policy_and_tests(
        &mut anchor,
        &selection,
        &source,
        execution,
        &mut shared_progress,
    )?;
    writeln!(
        shared_progress,
        "[rust-test-selection-report] {}",
        String::from_utf8(
            tests.encode().map_err(|error| {
                ChangeGateError::protocol("test-selection-unencodable", error)
            })?,
        )
        .map_err(|error| ChangeGateError::protocol("test-selection-non-utf8", error))?,
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    shared_progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
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

    let report = ChangeGateReport::new(selection, tests, policy)?;
    if report.admits() {
        anchor
            .accept_state(StateRunScope::Partial)
            .map_err(ChangeGateError::gate)?;
    } else {
        anchor.reject_state().map_err(ChangeGateError::gate)?;
    }
    Ok(report)
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
        "[change-gate-selection-report] {}",
        String::from_utf8(selection.encode().map_err(|error| {
            ChangeGateError::protocol("change-selection-unencodable", error)
        })?,)
        .map_err(|error| ChangeGateError::protocol("change-selection-non-utf8", error))?,
    )
    .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    progress
        .flush()
        .map_err(|error| ChangeGateError::environment("change-gate-progress-unavailable", error))?;
    Ok(())
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
        environment,
    } = execution;
    let (tests, policy) = thread::scope(|scope| {
        let mut test_progress = progress.clone();
        let test_worker = scope.spawn(move || {
            let roots = WorkspaceTestRoots::new(&selected_root, &writable_root);
            gate::run_selected_workspace_tests_at(
                roots,
                test_source,
                &test_changes,
                environment,
                WORKSPACE_TEST_CONCURRENCY_CAP,
                &mut test_progress,
            )
            .map_err(|error| ChangeGateError::refusal("change-gate-tests-refused", error))
        });
        let policy = run_focused_policy(anchor, selection.gate_ids(), source, progress)
            .and_then(|reports| ChangePolicyReport::focused(source, &reports));
        let tests = test_worker.join().map_err(|_| {
            ChangeGateError::environment(
                "change-gate-test-worker-panicked",
                "selected workspace test worker panicked",
            )
        });
        (tests, policy)
    });
    let tests = tests??;
    let policy = policy?;
    Ok((tests, policy))
}

fn run_focused_policy(
    anchor: &mut crate::gate::PreparedArm<'_>,
    gate_ids: &[GateId],
    source: &wire::Digest,
    progress: &mut impl Write,
) -> Result<Vec<FocusedArmReport>, ChangeGateError> {
    let state_run = anchor.state_run();
    let mut policy = Vec::with_capacity(gate_ids.len());
    for gate_id in gate_ids {
        writeln!(progress, "[change-gate-policy] gate={}", gate_id.as_str()).map_err(|error| {
            ChangeGateError::environment("change-gate-progress-unavailable", error)
        })?;
        progress.flush().map_err(|error| {
            ChangeGateError::environment("change-gate-progress-unavailable", error)
        })?;
        let report = if *gate_id == GateId::Staging {
            anchor.execute().map_err(ChangeGateError::gate)?
        } else {
            crate::gate::prepare_arm_in_state(Selector::staged(), *gate_id, state_run.clone())
                .and_then(|mut prepared| prepared.execute())
                .map_err(ChangeGateError::gate)?
        };
        if report.context().source() != source {
            return Err(ChangeGateError::refusal(
                "change-gate-source-moved",
                format!(
                    "staged source changed while executing focused arm {}",
                    gate_id.as_str(),
                ),
            ));
        }
        policy.push(report);
    }
    Ok(policy)
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
mod tests {
    use super::*;

    #[test]
    fn report_rejects_missing_and_source_mismatched_policy_evidence() {
        let source = wire::Digest::compute(wire::Domain::GateTree, b"change gate report");
        let selection = gate::select_change_gates(source.clone(), &[]).expect("empty selection");
        let tests = gate::TestEvidenceGraph::select_tests(
            source,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            gate::CoverageCompleteness::Complete,
        )
        .expect("empty test selection");

        let policy = ChangePolicyReport::Focused {
            source: selection.source().clone(),
            results: Vec::new(),
        };
        let error = ChangeGateReport::new(selection, tests, policy)
            .expect_err("selected always-on arms require focused evidence");
        assert_eq!(error.reason(), "gate::change-gate-policy-incomplete");
    }
}
