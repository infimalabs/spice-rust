//! One typed authority for long-running test and mutation evidence.
//!
//! The canonical workspace-test execution publishes test identities and their
//! source coverage into [`CanonicalTestExecution`]. The existing mutation
//! execution publishes [`MutationExecution`] from its completed [`KillRun`].
//! [`TestEvidenceGraph`] is the only place those authorities meet, and suite
//! seam, mutation coverage, and test subsumption are pure folds over it.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use wire::Digest;

use crate::kill::{KillRun, MutantDisposition, SurvivorRate};
use crate::manifest::GateId;

mod fold;

use fold::{rate_regressed, subsumption_findings};

/// The one combined test-evidence report schema this build emits.
pub const TEST_EVIDENCE_REPORT_VERSION: u64 = 1;

/// Why coverage cannot establish a clean result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub enum CoverageCompleteness {
    /// Every known suite test published all of its covered source points.
    Complete,
    /// The execution published some coverage but proved its denominator was incomplete.
    Incomplete { reason: String },
    /// The execution could not establish any authoritative coverage denominator.
    Unknown { reason: String },
}

/// One exact conservative source-to-test reach edge used for hook selection.
///
/// The edge says canonical Cargo metadata proves that the named test target
/// directly compiles the source through its declared target root. It does not
/// pretend to be line coverage; the long-running coverage folds retain that
/// separate authority.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestSelectionEdge {
    source: String,
    test: String,
}

impl TestSelectionEdge {
    /// Bind one changed Rust source to one selected package-qualified test target.
    pub fn new(
        source: impl Into<String>,
        test: impl Into<String>,
    ) -> Result<Self, TestEvidenceError> {
        let source = source.into();
        let test = test.into();
        require_path("selected Rust source", &source)?;
        require_text("selected test identity", &test)?;
        Ok(Self { source, test })
    }

    /// Exact repository-relative changed Rust source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Exact package-qualified Cargo test-target identity.
    #[must_use]
    pub fn test(&self) -> &str {
        &self.test
    }
}

/// Version of the staged-source test-selection report.
pub const TEST_SELECTION_REPORT_VERSION: u64 = 1;

/// Typed staged-source plan and its exact source-to-test provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestSelectionReport {
    version: u64,
    source: Digest,
    changed_sources: Vec<String>,
    tests: Vec<String>,
    edges: Vec<TestSelectionEdge>,
    coverage: CoverageCompleteness,
}

impl TestSelectionReport {
    /// Publish one canonical hook selection.
    ///
    /// Complete selection requires every changed source and every selected
    /// test to participate in at least one exact edge. Unknown or incomplete
    /// selection remains encodable but never admits execution as sufficient.
    pub(crate) fn new(
        source: Digest,
        mut changed_sources: Vec<String>,
        mut tests: Vec<String>,
        mut edges: Vec<TestSelectionEdge>,
        coverage: CoverageCompleteness,
    ) -> Result<Self, TestEvidenceError> {
        for path in &changed_sources {
            require_path("changed Rust source", path)?;
        }
        changed_sources.sort();
        if let Some(path) = repeated(&changed_sources) {
            return Err(TestEvidenceError::new(format!(
                "changed Rust source {path:?} is repeated"
            )));
        }
        for test in &tests {
            require_text("selected test identity", test)?;
        }
        tests.sort();
        if let Some(test) = repeated(&tests) {
            return Err(TestEvidenceError::new(format!(
                "selected test identity {test:?} is repeated"
            )));
        }
        edges.sort();
        if let Some(pair) = edges.windows(2).find(|pair| pair[0] == pair[1]) {
            let edge = &pair[0];
            return Err(TestEvidenceError::new(format!(
                "source-to-test selection edge {:?} -> {:?} is repeated",
                edge.source, edge.test
            )));
        }
        let sources = changed_sources
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let selected = tests.iter().map(String::as_str).collect::<BTreeSet<_>>();
        for edge in &edges {
            if !sources.contains(edge.source.as_str()) {
                return Err(TestEvidenceError::new(format!(
                    "selection edge names unchanged source {:?}",
                    edge.source
                )));
            }
            if !selected.contains(edge.test.as_str()) {
                return Err(TestEvidenceError::new(format!(
                    "selection edge names unselected test {:?}",
                    edge.test
                )));
            }
        }
        if matches!(coverage, CoverageCompleteness::Complete) {
            if let Some(source) = changed_sources
                .iter()
                .find(|source| !edges.iter().any(|edge| edge.source == **source))
            {
                return Err(TestEvidenceError::new(format!(
                    "complete test selection has no reaching test for changed source {source:?}"
                )));
            }
            if let Some(test) = tests
                .iter()
                .find(|test| !edges.iter().any(|edge| edge.test == **test))
            {
                return Err(TestEvidenceError::new(format!(
                    "complete test selection has no changed-source provenance for test {test:?}"
                )));
            }
        }
        Ok(Self {
            version: TEST_SELECTION_REPORT_VERSION,
            source,
            changed_sources,
            tests,
            edges,
            coverage,
        })
    }

    /// Current report schema.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Exact selected-tree identity.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Sorted changed Rust sources considered by the selector.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
    }

    /// Sorted package-qualified test targets selected for execution.
    #[must_use]
    pub fn tests(&self) -> &[String] {
        &self.tests
    }

    /// Exact conservative source-to-test reach edges.
    #[must_use]
    pub fn edges(&self) -> &[TestSelectionEdge] {
        &self.edges
    }

    /// Whether the complete source-to-test denominator was proven.
    #[must_use]
    pub const fn coverage(&self) -> &CoverageCompleteness {
        &self.coverage
    }

    /// Only complete selection evidence admits the narrowed test execution.
    #[must_use]
    pub const fn admits(&self) -> bool {
        matches!(self.coverage, CoverageCompleteness::Complete)
    }

    /// Encode the stable machine-readable hook report.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Decode and validate one hook report from this schema.
    pub fn decode(bytes: &[u8]) -> Result<Self, TestEvidenceError> {
        let report: Self = serde_json::from_slice(bytes).map_err(|error| {
            TestEvidenceError::new(format!("invalid test selection report: {error}"))
        })?;
        if report.version != TEST_SELECTION_REPORT_VERSION {
            return Err(TestEvidenceError::new(format!(
                "unsupported test selection report version {}",
                report.version
            )));
        }
        Self::new(
            report.source,
            report.changed_sources,
            report.tests,
            report.edges,
            report.coverage,
        )
    }
}

impl CoverageCompleteness {
    /// Declare incomplete coverage with one exact reason.
    pub fn incomplete(reason: impl Into<String>) -> Result<Self, TestEvidenceError> {
        let reason = reason.into();
        require_text("incomplete coverage reason", &reason)?;
        Ok(Self::Incomplete { reason })
    }

    /// Declare unknown coverage with one exact reason.
    pub fn unknown(reason: impl Into<String>) -> Result<Self, TestEvidenceError> {
        let reason = reason.into();
        require_text("unknown coverage reason", &reason)?;
        Ok(Self::Unknown { reason })
    }

    fn gap(&self) -> Option<String> {
        match self {
            Self::Complete => None,
            Self::Incomplete { reason } => Some(format!("coverage is incomplete: {reason}")),
            Self::Unknown { reason } => Some(format!("coverage is unknown: {reason}")),
        }
    }
}

/// Exact executed source points attributed to one path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoveredSource {
    path: String,
    lines: BTreeSet<u32>,
}

impl CoveredSource {
    /// Bind one repository-relative production source to its covered lines.
    pub fn new(
        path: impl Into<String>,
        lines: impl IntoIterator<Item = u32>,
    ) -> Result<Self, TestEvidenceError> {
        let path = path.into();
        require_path("covered source", &path)?;
        let lines = lines.into_iter().collect::<BTreeSet<_>>();
        if lines.is_empty() || lines.contains(&0) {
            return Err(TestEvidenceError::new(format!(
                "covered source {path:?} requires positive covered lines"
            )));
        }
        Ok(Self { path, lines })
    }

    /// Exact repository-relative source path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Sorted unique one-based covered lines.
    #[must_use]
    pub const fn lines(&self) -> &BTreeSet<u32> {
        &self.lines
    }
}

/// Per-test coverage published for the canonical execution's selected source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestCoverage {
    test: String,
    sources: Vec<CoveredSource>,
}

impl TestCoverage {
    /// Bind one exact test identity to its complete source coverage.
    pub fn new(
        test: impl Into<String>,
        mut sources: Vec<CoveredSource>,
    ) -> Result<Self, TestEvidenceError> {
        let test = test.into();
        require_text("test identity", &test)?;
        sources.sort_by(|left, right| left.path.cmp(&right.path));
        if let Some(pair) = sources.windows(2).find(|pair| pair[0].path == pair[1].path) {
            return Err(TestEvidenceError::new(format!(
                "test {test:?} repeats covered source {:?}",
                pair[0].path
            )));
        }
        Ok(Self { test, sources })
    }

    /// Stable test identity emitted by the canonical execution.
    #[must_use]
    pub fn test(&self) -> &str {
        &self.test
    }

    /// Exact sorted covered-source provenance.
    #[must_use]
    pub fn sources(&self) -> &[CoveredSource] {
        &self.sources
    }

    fn features(&self) -> BTreeSet<(String, u32)> {
        self.sources
            .iter()
            .flat_map(|source| source.lines.iter().map(|line| (source.path.clone(), *line)))
            .collect()
    }
}

/// Typed output of one canonical workspace-test execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTestExecution {
    source: Digest,
    changed_sources: Vec<String>,
    tests_run: Vec<String>,
    test_coverage: Vec<TestCoverage>,
    coverage: CoverageCompleteness,
}

impl CanonicalTestExecution {
    /// Publish selected source, changed paths, executed tests, and suite coverage.
    pub fn new(
        source: Digest,
        mut changed_sources: Vec<String>,
        mut tests_run: Vec<String>,
        mut test_coverage: Vec<TestCoverage>,
        coverage: CoverageCompleteness,
    ) -> Result<Self, TestEvidenceError> {
        for path in &changed_sources {
            require_path("changed production source", path)?;
        }
        changed_sources.sort();
        if let Some(path) = repeated(&changed_sources) {
            return Err(TestEvidenceError::new(format!(
                "changed production source {path:?} is repeated"
            )));
        }
        for test in &tests_run {
            require_text("executed test identity", test)?;
        }
        tests_run.sort();
        if let Some(test) = repeated(&tests_run) {
            return Err(TestEvidenceError::new(format!(
                "canonical executed test identity {test:?} is repeated"
            )));
        }
        if tests_run.is_empty() {
            return Err(TestEvidenceError::new(
                "canonical test execution requires at least one executed test",
            ));
        }
        test_coverage.sort_by(|left, right| left.test.cmp(&right.test));
        if let Some(pair) = test_coverage
            .windows(2)
            .find(|pair| pair[0].test == pair[1].test)
        {
            return Err(TestEvidenceError::new(format!(
                "test coverage identity {:?} is repeated",
                pair[0].test
            )));
        }
        if matches!(coverage, CoverageCompleteness::Complete) {
            let covered_tests = test_coverage
                .iter()
                .map(|test| test.test.as_str())
                .collect::<BTreeSet<_>>();
            if let Some(missing) = tests_run
                .iter()
                .find(|test| !covered_tests.contains(test.as_str()))
            {
                return Err(TestEvidenceError::new(format!(
                    "complete coverage omits executed test {missing:?}"
                )));
            }
        }
        Ok(Self {
            source,
            changed_sources,
            tests_run,
            test_coverage,
            coverage,
        })
    }

    /// Selected source identity the tests executed.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Changed production sources whose test reach must be established.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
    }

    /// Stable identities for every test the canonical execution ran.
    #[must_use]
    pub fn tests_run(&self) -> &[String] {
        &self.tests_run
    }

    /// Exact per-test coverage across the known suite.
    #[must_use]
    pub fn test_coverage(&self) -> &[TestCoverage] {
        &self.test_coverage
    }

    /// Whether the coverage denominator is established.
    #[must_use]
    pub const fn coverage(&self) -> &CoverageCompleteness {
        &self.coverage
    }
}

/// One mutation at one exact production source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationObservation {
    identity: String,
    source: String,
    disposition: MutantDisposition,
}

impl MutationObservation {
    /// Retain one canonical mutation identity, source, and confirmed disposition.
    pub fn new(
        identity: impl Into<String>,
        source: impl Into<String>,
        disposition: MutantDisposition,
    ) -> Result<Self, TestEvidenceError> {
        let identity = identity.into();
        let source = source.into();
        require_text("mutation identity", &identity)?;
        require_path("mutation source", &source)?;
        Ok(Self {
            identity,
            source,
            disposition,
        })
    }

    /// Stable mutation-tool identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Exact repository-relative source published by the mutation inventory.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Disposition derived from every declared mutation attempt.
    #[must_use]
    pub const fn disposition(&self) -> MutantDisposition {
        self.disposition
    }
}

/// Typed output projected from the existing canonical mutation execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationExecution {
    source: Digest,
    floor: Option<SurvivorRate>,
    mutants: Vec<MutationObservation>,
}

impl MutationExecution {
    /// Publish one mutation execution without granting any runner authority.
    pub fn new(
        source: Digest,
        floor: Option<SurvivorRate>,
        mut mutants: Vec<MutationObservation>,
    ) -> Result<Self, TestEvidenceError> {
        mutants.sort_by(|left, right| left.identity.cmp(&right.identity));
        if mutants.is_empty() {
            return Err(TestEvidenceError::new(
                "mutation execution requires a nonempty inventoried corpus",
            ));
        }
        if let Some(pair) = mutants
            .windows(2)
            .find(|pair| pair[0].identity == pair[1].identity)
        {
            return Err(TestEvidenceError::new(format!(
                "mutation identity {:?} is repeated",
                pair[0].identity
            )));
        }
        Ok(Self {
            source,
            floor,
            mutants,
        })
    }

    /// Project a completed [`KillRun`] into observation-only graph evidence.
    pub fn from_kill(run: &KillRun) -> Result<Self, TestEvidenceError> {
        let mutants = run
            .mutants()
            .iter()
            .map(|mutant| {
                MutationObservation::new(
                    mutant.identity.clone(),
                    mutant.source.clone(),
                    mutant.disposition,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(run.source().clone(), run.floor(), mutants)
    }

    /// Selected source identity the mutations executed.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Committed lower-is-better survivor floor, if one was readable.
    #[must_use]
    pub const fn floor(&self) -> Option<SurvivorRate> {
        self.floor
    }

    /// Every inventoried mutation in canonical identity order.
    #[must_use]
    pub fn mutants(&self) -> &[MutationObservation] {
        &self.mutants
    }
}

/// One tracked exact mutation exception with its required rationale.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandingMutationWaiver {
    mutant: String,
    source: String,
    reason: String,
}

impl StandingMutationWaiver {
    /// Declare one exact survivor as standing debt for one exact reason.
    pub fn new(
        mutant: impl Into<String>,
        source: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, TestEvidenceError> {
        let mutant = mutant.into();
        let source = source.into();
        let reason = reason.into();
        require_text("waived mutation identity", &mutant)?;
        require_path("waived mutation source", &source)?;
        require_text("mutation waiver reason", &reason)?;
        Ok(Self {
            mutant,
            source,
            reason,
        })
    }

    /// Exact survivor identity.
    #[must_use]
    pub fn mutant(&self) -> &str {
        &self.mutant
    }

    /// Exact survivor source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Reviewable standing rationale.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The single authority consumed by all three test-evidence instruments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestEvidenceGraph {
    tests: CanonicalTestExecution,
    mutation: MutationExecution,
    waivers: Vec<StandingMutationWaiver>,
}

impl TestEvidenceGraph {
    /// Validate the pre-execution staged-source selection slice consumed by
    /// the commit hook.
    ///
    /// This keeps selection on the same typed evidence authority as the three
    /// post-execution folds. The selection report carries conservative Cargo
    /// reach edges rather than manufacturing line coverage or mutation input.
    pub fn select_tests(
        source: Digest,
        changed_sources: Vec<String>,
        tests: Vec<String>,
        edges: Vec<TestSelectionEdge>,
        coverage: CoverageCompleteness,
    ) -> Result<TestSelectionReport, TestEvidenceError> {
        TestSelectionReport::new(source, changed_sources, tests, edges, coverage)
    }

    /// Join canonical test evidence directly to one completed canonical kill run.
    ///
    /// # Errors
    ///
    /// Returns malformed mutation evidence, mismatched selected sources, or
    /// malformed/repeated waivers. The completed run grants no new execution.
    pub fn from_kill(
        tests: CanonicalTestExecution,
        run: &KillRun,
        waivers: Vec<StandingMutationWaiver>,
    ) -> Result<Self, TestEvidenceError> {
        Self::new(tests, MutationExecution::from_kill(run)?, waivers)
    }

    /// Join canonical test and mutation executions only when they name one source.
    pub fn new(
        tests: CanonicalTestExecution,
        mutation: MutationExecution,
        mut waivers: Vec<StandingMutationWaiver>,
    ) -> Result<Self, TestEvidenceError> {
        if tests.source != mutation.source {
            return Err(TestEvidenceError::new(
                "canonical test and mutation executions name different selected sources",
            ));
        }
        waivers.sort_by(|left, right| {
            (&left.mutant, &left.source).cmp(&(&right.mutant, &right.source))
        });
        if let Some(pair) = waivers
            .windows(2)
            .find(|pair| pair[0].mutant == pair[1].mutant && pair[0].source == pair[1].source)
        {
            return Err(TestEvidenceError::new(format!(
                "standing mutation waiver for {:?} at {:?} is repeated",
                pair[0].mutant, pair[0].source
            )));
        }
        Ok(Self {
            tests,
            mutation,
            waivers,
        })
    }

    /// Selected source shared by both executions.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.tests.source
    }

    /// Canonical workspace-test evidence.
    #[must_use]
    pub const fn tests(&self) -> &CanonicalTestExecution {
        &self.tests
    }

    /// Existing mutation-execution evidence.
    #[must_use]
    pub const fn mutation(&self) -> &MutationExecution {
        &self.mutation
    }

    /// Reasoned standing mutation exceptions.
    #[must_use]
    pub fn waivers(&self) -> &[StandingMutationWaiver] {
        &self.waivers
    }

    /// Binary fold: no changed production source may reach a test that did not run.
    #[must_use]
    pub fn suite_seam(&self) -> SuiteSeamReport {
        if let Some(reason) = self.tests.coverage.gap() {
            return SuiteSeamReport::unknown(reason);
        }
        let tests_run = self
            .tests
            .tests_run
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let changed_sources = self
            .tests
            .changed_sources
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let findings = self
            .tests
            .test_coverage
            .iter()
            .filter(|test| !tests_run.contains(test.test.as_str()))
            .flat_map(|test| {
                test.sources
                    .iter()
                    .filter(|source| changed_sources.contains(source.path.as_str()))
                    .map(|source| SuiteSeamFinding {
                        test: test.test.clone(),
                        source: source.path.clone(),
                    })
            })
            .collect::<Vec<_>>();
        SuiteSeamReport::measured(findings)
    }

    /// Differential fold: reasoned standing survivors are removed from debt and only regression fails.
    #[must_use]
    pub fn mutation_coverage(&self) -> MutationCoverageReport {
        if let Some(reason) = self.tests.coverage.gap() {
            return MutationCoverageReport::unknown(reason, self.mutation.floor);
        }
        let Some(floor) = self.mutation.floor else {
            return MutationCoverageReport::unknown(
                "mutation survivor floor is unavailable".to_owned(),
                None,
            );
        };
        if self
            .mutation
            .mutants
            .iter()
            .any(|mutant| mutant.disposition == MutantDisposition::Unconfirmed)
        {
            return MutationCoverageReport::unknown(
                "one or more mutation dispositions are unconfirmed".to_owned(),
                Some(floor),
            );
        }
        let waived = self
            .waivers
            .iter()
            .map(|waiver| (waiver.mutant.as_str(), waiver.source.as_str()))
            .collect::<BTreeSet<_>>();
        let survivors = self
            .mutation
            .mutants
            .iter()
            .filter(|mutant| {
                mutant.disposition == MutantDisposition::ConfirmedSurvived
                    && !waived.contains(&(mutant.identity.as_str(), mutant.source.as_str()))
            })
            .collect::<Vec<_>>();
        let measured =
            SurvivorRate::new(survivors.len() as u64, self.mutation.mutants.len() as u64)
                .expect("a nonempty mutation execution always forms a rate");
        let stale_waivers = self
            .waivers
            .iter()
            .filter(|waiver| {
                !self.mutation.mutants.iter().any(|mutant| {
                    mutant.identity == waiver.mutant
                        && mutant.source == waiver.source
                        && mutant.disposition == MutantDisposition::ConfirmedSurvived
                })
            })
            .cloned()
            .collect();
        let findings = if rate_regressed(measured, floor) {
            survivors
                .into_iter()
                .map(|mutant| MutationCoverageFinding {
                    mutant: mutant.identity.clone(),
                    source: mutant.source.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        MutationCoverageReport::from_measurement(floor, measured, findings, stale_waivers)
    }

    /// Advisory fold: report deterministic equal or strict coverage containment.
    #[must_use]
    pub fn test_subsumption(&self) -> TestSubsumptionReport {
        if let Some(reason) = self.tests.coverage.gap() {
            return TestSubsumptionReport::unknown(reason);
        }
        let findings = subsumption_findings(&self.tests.test_coverage);
        TestSubsumptionReport::measured(findings)
    }

    /// Project the graph and all three folds into one observation-only report.
    #[must_use]
    pub fn report(&self) -> TestEvidenceReport {
        TestEvidenceReport {
            version: TEST_EVIDENCE_REPORT_VERSION,
            source: self.source().clone(),
            tests: self.tests.clone(),
            mutation: self.mutation.clone(),
            waivers: self.waivers.clone(),
            suite_seam: self.suite_seam(),
            mutation_coverage: self.mutation_coverage(),
            test_subsumption: self.test_subsumption(),
        }
    }
}

/// One report surface retaining the shared evidence and all three folds.
///
/// The test and mutation executions are copied observations. They retain exact
/// selected-source, changed-path, test, covered-line, and mutant provenance but
/// contain no executable, environment, bound, contract, or runner authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestEvidenceReport {
    version: u64,
    source: Digest,
    tests: CanonicalTestExecution,
    mutation: MutationExecution,
    waivers: Vec<StandingMutationWaiver>,
    suite_seam: SuiteSeamReport,
    mutation_coverage: MutationCoverageReport,
    test_subsumption: TestSubsumptionReport,
}

impl TestEvidenceReport {
    /// Current report schema.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Source identity shared by both canonical executions.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Canonical workspace-test evidence retained by the report.
    #[must_use]
    pub const fn tests(&self) -> &CanonicalTestExecution {
        &self.tests
    }

    /// Canonical mutation evidence retained by the report.
    #[must_use]
    pub const fn mutation(&self) -> &MutationExecution {
        &self.mutation
    }

    /// Exact standing mutation waivers applied by the folds.
    #[must_use]
    pub fn waivers(&self) -> &[StandingMutationWaiver] {
        &self.waivers
    }

    /// Blocking suite-seam fold.
    #[must_use]
    pub const fn suite_seam(&self) -> &SuiteSeamReport {
        &self.suite_seam
    }

    /// Blocking mutation-coverage fold.
    #[must_use]
    pub const fn mutation_coverage(&self) -> &MutationCoverageReport {
        &self.mutation_coverage
    }

    /// Advisory test-subsumption fold.
    #[must_use]
    pub const fn test_subsumption(&self) -> &TestSubsumptionReport {
        &self.test_subsumption
    }

    /// Whether one registered test-evidence instrument admits publication.
    ///
    /// Subsumption findings are advisory and admit; unknown subsumption evidence
    /// does not. IDs outside the test-evidence roster have no decision here.
    #[must_use]
    pub fn instrument_admits(&self, gate_id: GateId) -> Option<bool> {
        match gate_id {
            GateId::SuiteSeam => Some(self.suite_seam.admits()),
            GateId::MutationCoverage => Some(self.mutation_coverage.admits()),
            GateId::TestSubsumption => Some(self.test_subsumption.admits()),
            _ => None,
        }
    }

    /// Encode one stable JSON document containing evidence and every fold.
    ///
    /// # Errors
    ///
    /// Returns a serialization failure without emitting partial bytes.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Common standing vocabulary for the three graph folds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestEvidenceStanding {
    Clean,
    Findings,
    Unknown,
}

/// One exact changed-source reach edge to a test the execution did not run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteSeamFinding {
    test: String,
    source: String,
}

impl SuiteSeamFinding {
    /// Exact test reached by the change but absent from the canonical execution.
    #[must_use]
    pub fn test(&self) -> &str {
        &self.test
    }

    /// Exact changed production source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Binary suite-seam fold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteSeamReport {
    standing: TestEvidenceStanding,
    findings: Vec<SuiteSeamFinding>,
    reason: Option<String>,
}

impl SuiteSeamReport {
    /// Registered instrument identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        GateId::SuiteSeam
    }

    /// Clean, blocking findings, or unknown coverage.
    #[must_use]
    pub const fn standing(&self) -> TestEvidenceStanding {
        self.standing
    }

    /// Exact unrun-test/changed-source reach edges.
    #[must_use]
    pub fn findings(&self) -> &[SuiteSeamFinding] {
        &self.findings
    }

    /// Loud reason retained for unknown evidence.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Only complete coverage with no seam finding admits landing.
    #[must_use]
    pub const fn admits(&self) -> bool {
        matches!(self.standing, TestEvidenceStanding::Clean)
    }

    fn measured(findings: Vec<SuiteSeamFinding>) -> Self {
        Self {
            standing: if findings.is_empty() {
                TestEvidenceStanding::Clean
            } else {
                TestEvidenceStanding::Findings
            },
            findings,
            reason: None,
        }
    }

    fn unknown(reason: String) -> Self {
        Self {
            standing: TestEvidenceStanding::Unknown,
            findings: Vec::new(),
            reason: Some(reason),
        }
    }
}

/// One unwaived confirmed survivor retained by a regressed mutation fold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationCoverageFinding {
    mutant: String,
    source: String,
}

impl MutationCoverageFinding {
    /// Exact survivor identity.
    #[must_use]
    pub fn mutant(&self) -> &str {
        &self.mutant
    }

    /// Exact production source from structured mutation inventory evidence.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Differential mutation-coverage fold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationCoverageReport {
    standing: TestEvidenceStanding,
    floor: Option<SurvivorRate>,
    measured: Option<SurvivorRate>,
    findings: Vec<MutationCoverageFinding>,
    stale_waivers: Vec<StandingMutationWaiver>,
    reason: Option<String>,
}

impl MutationCoverageReport {
    /// Registered instrument identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        GateId::MutationCoverage
    }

    /// Clean, regressed, or unknown mutation evidence.
    #[must_use]
    pub const fn standing(&self) -> TestEvidenceStanding {
        self.standing
    }

    /// Committed lower-is-better survivor floor.
    #[must_use]
    pub const fn floor(&self) -> Option<SurvivorRate> {
        self.floor
    }

    /// Current survivor rate after exact standing waivers.
    #[must_use]
    pub const fn measured(&self) -> Option<SurvivorRate> {
        self.measured
    }

    /// Exact unwaived survivors when and only when the rate regressed.
    #[must_use]
    pub fn findings(&self) -> &[MutationCoverageFinding] {
        &self.findings
    }

    /// Standing entries that no longer name a current confirmed survivor.
    #[must_use]
    pub fn stale_waivers(&self) -> &[StandingMutationWaiver] {
        &self.stale_waivers
    }

    /// Loud reason retained for unknown evidence.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Only a proven non-regression admits; stale waiver review remains advisory.
    #[must_use]
    pub const fn admits(&self) -> bool {
        matches!(self.standing, TestEvidenceStanding::Clean)
    }

    fn from_measurement(
        floor: SurvivorRate,
        measured: SurvivorRate,
        findings: Vec<MutationCoverageFinding>,
        stale_waivers: Vec<StandingMutationWaiver>,
    ) -> Self {
        Self {
            standing: if findings.is_empty() {
                TestEvidenceStanding::Clean
            } else {
                TestEvidenceStanding::Findings
            },
            floor: Some(floor),
            measured: Some(measured),
            findings,
            stale_waivers,
            reason: None,
        }
    }

    fn unknown(reason: String, floor: Option<SurvivorRate>) -> Self {
        Self {
            standing: TestEvidenceStanding::Unknown,
            floor,
            measured: None,
            findings: Vec::new(),
            stale_waivers: Vec::new(),
            reason: Some(reason),
        }
    }
}

/// Exact coverage-containment relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubsumptionRelation {
    Equal,
    StrictSubset,
}

/// One advisory coverage-containment candidate with source provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestSubsumptionFinding {
    test: String,
    contained_by: String,
    relation: SubsumptionRelation,
    covered_sources: Vec<CoveredSource>,
}

impl TestSubsumptionFinding {
    /// Candidate test whose coverage is contained.
    #[must_use]
    pub fn test(&self) -> &str {
        &self.test
    }

    /// Deterministic representative whose coverage contains the candidate.
    #[must_use]
    pub fn contained_by(&self) -> &str {
        &self.contained_by
    }

    /// Equal-feature or strict-subset relation.
    #[must_use]
    pub const fn relation(&self) -> SubsumptionRelation {
        self.relation
    }

    /// Exact paths and lines covered by the candidate.
    #[must_use]
    pub fn covered_sources(&self) -> &[CoveredSource] {
        &self.covered_sources
    }
}

/// Non-blocking test-subsumption fold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestSubsumptionReport {
    standing: TestEvidenceStanding,
    findings: Vec<TestSubsumptionFinding>,
    reason: Option<String>,
}

impl TestSubsumptionReport {
    /// Registered instrument identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        GateId::TestSubsumption
    }

    /// Clean, advisory findings, or unknown coverage.
    #[must_use]
    pub const fn standing(&self) -> TestEvidenceStanding {
        self.standing
    }

    /// Deterministic advisory coverage-containment candidates.
    #[must_use]
    pub fn findings(&self) -> &[TestSubsumptionFinding] {
        &self.findings
    }

    /// Loud reason retained for unknown evidence.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Findings are advisory, but unknown coverage never admits a clean report.
    #[must_use]
    pub const fn admits(&self) -> bool {
        !matches!(self.standing, TestEvidenceStanding::Unknown)
    }

    fn measured(findings: Vec<TestSubsumptionFinding>) -> Self {
        Self {
            standing: if findings.is_empty() {
                TestEvidenceStanding::Clean
            } else {
                TestEvidenceStanding::Findings
            },
            findings,
            reason: None,
        }
    }

    fn unknown(reason: String) -> Self {
        Self {
            standing: TestEvidenceStanding::Unknown,
            findings: Vec::new(),
            reason: Some(reason),
        }
    }
}

/// A malformed or mismatched test-evidence authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestEvidenceError {
    detail: String,
}

impl TestEvidenceError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// Exact refusal detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for TestEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for TestEvidenceError {}

fn repeated(values: &[String]) -> Option<&str> {
    values
        .windows(2)
        .find_map(|pair| (pair[0] == pair[1]).then_some(pair[0].as_str()))
}

fn require_text(field: &str, value: &str) -> Result<(), TestEvidenceError> {
    if value.trim().is_empty() || value.contains(['\0', '\r', '\n']) {
        Err(TestEvidenceError::new(format!(
            "{field} must be nonempty single-line text"
        )))
    } else {
        Ok(())
    }
}

fn require_path(field: &str, value: &str) -> Result<(), TestEvidenceError> {
    require_text(field, value)?;
    let invalid = value.starts_with('/')
        || value.contains('\\')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."));
    if invalid {
        Err(TestEvidenceError::new(format!(
            "{field} must be a normalized repository-relative path: {value:?}"
        )))
    } else {
        Ok(())
    }
}
