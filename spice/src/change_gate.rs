//! One fail-closed ordinary gate over exact staged-change evidence.

use std::env;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use gate::{
    ChangeSelectionMode, ChangeSelectionReport, FocusedArmReport, GateId, Selector,
    TestSelectionReport, WORKSPACE_TEST_CONCURRENCY_CAP, WorkspaceTestRoots,
};
use serde::{Deserialize, Serialize};
use sill::Environment;

use crate::repository::Repository;

/// Current combined ordinary-change-gate report schema.
pub const CHANGE_GATE_REPORT_VERSION: u64 = 1;

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

/// Canonical selected tests and focused policy evidence for one staged source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeGateReport {
    version: u64,
    selection: ChangeSelectionReport,
    tests: TestSelectionReport,
    policy: Vec<FocusedArmReport>,
}

impl ChangeGateReport {
    fn new(
        selection: ChangeSelectionReport,
        tests: TestSelectionReport,
        policy: Vec<FocusedArmReport>,
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

    /// Canonical focused arm reports in selected manifest order.
    #[must_use]
    pub fn policy(&self) -> &[FocusedArmReport] {
        &self.policy
    }

    /// Whether every selected test and policy arm admitted.
    #[must_use]
    pub fn admits(&self) -> bool {
        self.tests.admits() && self.policy.iter().all(FocusedArmReport::admits)
    }

    /// Compact deterministic evidence for selected policy refusals.
    #[must_use]
    pub fn refusal_fold(&self) -> String {
        let mut refusal = String::new();
        for report in self.policy.iter().filter(|report| !report.admits()) {
            let result = report.result();
            let _ = writeln!(
                refusal,
                "gate={} state={} reason={}",
                report.gate_id(),
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
        if self.selection.mode() == ChangeSelectionMode::Exhaustive
            && self.tests.changed_sources() != self.selection.changed_sources()
        {
            return Err(ChangeGateError::protocol(
                "change-gate-exhaustive-test-incomplete",
                "exhaustive policy selection requires test edges for every changed source",
            ));
        }
        let observed = self
            .policy
            .iter()
            .map(FocusedArmReport::gate_id)
            .collect::<Vec<_>>();
        if observed != self.selection.gate_ids() {
            return Err(ChangeGateError::protocol(
                "change-gate-policy-incomplete",
                "focused policy reports do not exactly cover the selected arm inventory",
            ));
        }
        if let Some(report) = self
            .policy
            .iter()
            .find(|report| report.context().source() != source)
        {
            return Err(ChangeGateError::protocol(
                "change-gate-source-moved",
                format!(
                    "focused arm {} observed a different staged source",
                    report.gate_id(),
                ),
            ));
        }
        Ok(())
    }
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

    let repository = Repository::discover(Path::new(".")).map_err(|error| {
        ChangeGateError::environment("change-gate-repository-unresolved", error)
    })?;
    let selected_root = anchor.root().to_path_buf();
    let writable_root = repository.worktree_root().to_path_buf();
    let environment = ambient_environment().map_err(|error| {
        ChangeGateError::environment("change-gate-environment-unavailable", error)
    })?;
    let test_source = anchor.source().clone();
    let test_changes = anchor.changes().to_vec();
    let mode = selection.mode();
    let source = selection.source().clone();
    let mut shared_progress = SharedProgress::new(progress);
    let (tests, policy) = thread::scope(|scope| {
        let mut test_progress = shared_progress.clone();
        let test_worker = scope.spawn(move || {
            let roots = WorkspaceTestRoots::new(&selected_root, &writable_root);
            match mode {
                ChangeSelectionMode::Local => gate::run_selected_workspace_tests_at(
                    roots,
                    test_source,
                    &test_changes,
                    environment,
                    WORKSPACE_TEST_CONCURRENCY_CAP,
                    &mut test_progress,
                ),
                ChangeSelectionMode::Exhaustive => {
                    gate::run_exhaustive_selected_workspace_tests_at(
                        roots,
                        test_source,
                        &test_changes,
                        environment,
                        WORKSPACE_TEST_CONCURRENCY_CAP,
                        &mut test_progress,
                    )
                }
            }
            .map_err(|error| ChangeGateError::refusal("change-gate-tests-refused", error))
        });
        let policy = run_policy(
            &mut anchor,
            selection.gate_ids(),
            &source,
            &mut shared_progress,
        );
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

    ChangeGateReport::new(selection, tests, policy)
}

fn run_policy(
    anchor: &mut crate::gate::PreparedArm<'_>,
    gate_ids: &[GateId],
    source: &wire::Digest,
    progress: &mut impl Write,
) -> Result<Vec<FocusedArmReport>, ChangeGateError> {
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
            crate::gate::run_arm_report(Selector::staged(), *gate_id)
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

        let error = ChangeGateReport::new(selection, tests, Vec::new())
            .expect_err("selected always-on arms require focused evidence");
        assert_eq!(error.reason(), "gate::change-gate-policy-incomplete");
    }

    #[test]
    fn exhaustive_report_requires_test_edges_for_every_changed_source() {
        let source = wire::Digest::compute(wire::Domain::GateTree, b"exhaustive report");
        let selection = gate::select_change_gates(source.clone(), &["assets/opaque.bin".into()])
            .expect("unknown path escalates");
        let tests = gate::TestEvidenceGraph::select_tests(
            source,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            gate::CoverageCompleteness::Complete,
        )
        .expect("empty test evidence remains structurally valid");

        let error = ChangeGateReport::new(selection, tests, Vec::new())
            .expect_err("exhaustive selection cannot omit its changed test source");
        assert_eq!(
            error.reason(),
            "gate::change-gate-exhaustive-test-incomplete"
        );
    }
}
