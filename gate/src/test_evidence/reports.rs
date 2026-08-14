//! Stable report surfaces produced by the shared test-evidence graph.

use serde::Serialize;
use wire::Digest;

use crate::kill::SurvivorRate;
use crate::manifest::GateId;

use super::{CanonicalTestExecution, CoveredSource, MutationExecution, StandingMutationWaiver};

/// One report surface retaining the shared evidence and all three folds.
///
/// The test and mutation executions are copied observations. They retain exact
/// selected-source, changed-path, test, covered-line, and mutant provenance but
/// contain no executable, environment, bound, contract, or runner authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestEvidenceReport {
    pub(super) version: u64,
    pub(super) source: Digest,
    pub(super) tests: CanonicalTestExecution,
    pub(super) mutation: MutationExecution,
    pub(super) waivers: Vec<StandingMutationWaiver>,
    pub(super) suite_seam: SuiteSeamReport,
    pub(super) mutation_coverage: MutationCoverageReport,
    pub(super) test_subsumption: TestSubsumptionReport,
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
    pub(super) test: String,
    pub(super) source: String,
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

    pub(super) fn measured(findings: Vec<SuiteSeamFinding>) -> Self {
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

    pub(super) fn unknown(reason: String) -> Self {
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
    pub(super) mutant: String,
    pub(super) source: String,
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

    pub(super) fn from_measurement(
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

    pub(super) fn unknown(reason: String, floor: Option<SurvivorRate>) -> Self {
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
    pub(super) test: String,
    pub(super) contained_by: String,
    pub(super) relation: SubsumptionRelation,
    pub(super) covered_sources: Vec<CoveredSource>,
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

    pub(super) fn measured(findings: Vec<TestSubsumptionFinding>) -> Self {
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

    pub(super) fn unknown(reason: String) -> Self {
        Self {
            standing: TestEvidenceStanding::Unknown,
            findings: Vec::new(),
            reason: Some(reason),
        }
    }
}
