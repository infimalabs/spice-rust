use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::sync::OnceLock;
use std::time::Instant;

use rustpython_ast::{Expr, ExprContext, Visitor};
use rustpython_parser::Parse;
use wire::{Digest, Domain};

use crate::jurisdiction::Jurisdiction;

use super::measure::evidence_rank;
use super::*;

const DEFAULT_DEADLINE_MS: u64 = 30_000;

/// Tracked exception for one otherwise-private test coupling.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PrivateCouplingException {
    test_name: String,
    target: String,
}

impl PrivateCouplingException {
    /// Admit one exact test/target coupling from tracked policy.
    ///
    /// # Errors
    ///
    /// Refuses empty or multiline identities.
    pub fn new(
        test_name: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<Self, HygieneAnalysisError> {
        let value = Self {
            test_name: test_name.into(),
            target: target.into(),
        };
        if invalid_identity(&value.test_name) || invalid_identity(&value.target) {
            return Err(HygieneAnalysisError::Configuration(
                "private coupling exception identities must be nonempty and single-line".to_owned(),
            ));
        }
        Ok(value)
    }
}

/// One tracked decision to exclude a complete hygiene arm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HygieneExclusion {
    gate_id: GateId,
    provenance: HygieneProvenance,
    reason: String,
}

impl HygieneExclusion {
    /// Construct an exclusion with its exact tracked provenance.
    ///
    /// # Errors
    ///
    /// Refuses foreign gates and incomplete evidence.
    pub fn new(
        gate_id: GateId,
        provenance: HygieneProvenance,
        reason: impl Into<String>,
    ) -> Result<Self, HygieneAnalysisError> {
        let reason = reason.into();
        if !HYGIENE_ARMS.iter().any(|arm| arm.id == gate_id) {
            return Err(HygieneAnalysisError::Configuration(format!(
                "{} is not a hygiene arm",
                gate_id.as_str()
            )));
        }
        if !provenance.is_complete() || invalid_identity(&reason) {
            return Err(HygieneAnalysisError::Configuration(
                "hygiene exclusions require complete provenance and a one-line reason".to_owned(),
            ));
        }
        Ok(Self {
            gate_id,
            provenance,
            reason,
        })
    }
}

/// Admitted tracked inputs for one complete hygiene run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HygieneConfiguration {
    production_roots: Vec<String>,
    test_roots: Vec<String>,
    assertion_helpers: Vec<String>,
    private_exceptions: Vec<PrivateCouplingException>,
    reachability_limit: u32,
    assertion_free_limit: u32,
    deadline_ms: u64,
    exclusions: Vec<HygieneExclusion>,
}

impl HygieneConfiguration {
    /// Admit the roots that seed production and test graph traversal.
    ///
    /// Roots are selected-tree-relative files or directories. They are
    /// normalized, sorted, and deduplicated before any source is read.
    ///
    /// # Errors
    ///
    /// Refuses an empty side or a non-normalized root.
    pub fn new(
        production_roots: impl IntoIterator<Item = impl Into<String>>,
        test_roots: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, HygieneAnalysisError> {
        let mut production_roots = collect_paths(production_roots)?;
        let mut test_roots = collect_paths(test_roots)?;
        if production_roots.is_empty() || test_roots.is_empty() {
            return Err(HygieneAnalysisError::Configuration(
                "hygiene analysis requires at least one production root and test root".to_owned(),
            ));
        }
        production_roots.sort();
        production_roots.dedup();
        test_roots.sort();
        test_roots.dedup();
        Ok(Self {
            production_roots,
            test_roots,
            assertion_helpers: Vec::new(),
            private_exceptions: Vec::new(),
            reachability_limit: 0,
            assertion_free_limit: 0,
            deadline_ms: DEFAULT_DEADLINE_MS,
            exclusions: Vec::new(),
        })
    }

    /// Replace configured assertion helpers with exact dotted call names.
    ///
    /// # Errors
    ///
    /// Refuses empty or multiline helper identities.
    pub fn with_assertion_helpers(
        mut self,
        helpers: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, HygieneAnalysisError> {
        self.assertion_helpers = collect_identities(helpers, "assertion helper")?;
        Ok(self)
    }

    /// Replace tracked private-coupling exceptions.
    #[must_use]
    pub fn with_private_exceptions(
        mut self,
        exceptions: impl IntoIterator<Item = PrivateCouplingException>,
    ) -> Self {
        self.private_exceptions = exceptions.into_iter().collect();
        self.private_exceptions.sort();
        self.private_exceptions.dedup();
        self
    }

    /// Set the two predecessor debt limits retained by these arms.
    #[must_use]
    pub const fn with_debt_limits(
        mut self,
        reachability_test_only: u32,
        assertion_free_tests: u32,
    ) -> Self {
        self.reachability_limit = reachability_test_only;
        self.assertion_free_limit = assertion_free_tests;
        self
    }

    /// Set the complete-run deadline.
    ///
    /// # Errors
    ///
    /// Refuses a zero deadline because it cannot distinguish scheduling from
    /// execution.
    pub fn with_deadline_ms(mut self, deadline_ms: u64) -> Result<Self, HygieneAnalysisError> {
        if deadline_ms == 0 {
            return Err(HygieneAnalysisError::Configuration(
                "hygiene deadline must be greater than zero milliseconds".to_owned(),
            ));
        }
        self.deadline_ms = deadline_ms;
        Ok(self)
    }

    /// Add tracked complete-arm exclusions.
    ///
    /// # Errors
    ///
    /// Refuses two exclusion decisions for the same arm.
    pub fn with_exclusions(
        mut self,
        exclusions: impl IntoIterator<Item = HygieneExclusion>,
    ) -> Result<Self, HygieneAnalysisError> {
        self.exclusions = exclusions.into_iter().collect();
        self.exclusions.sort_by_key(|item| item.gate_id);
        if self
            .exclusions
            .windows(2)
            .any(|pair| pair[0].gate_id == pair[1].gate_id)
        {
            return Err(HygieneAnalysisError::Configuration(
                "a hygiene arm may have at most one exclusion".to_owned(),
            ));
        }
        Ok(self)
    }
}

/// Why a source-driven hygiene run could not produce its canonical report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HygieneAnalysisError {
    /// The immutable selected source could not be captured whole.
    Source(String),
    /// Tracked inputs were not execution-grade.
    Configuration(String),
    /// Internally produced evidence violated the closed report contract.
    Measurement(HygieneError),
}

impl fmt::Display for HygieneAnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(evidence) => {
                write!(formatter, "hygiene source acquisition failed: {evidence}")
            }
            Self::Configuration(evidence) => formatter.write_str(evidence),
            Self::Measurement(source) => write!(formatter, "hygiene measurement failed: {source}"),
        }
    }
}

impl Error for HygieneAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(_) | Self::Configuration(_) => None,
            Self::Measurement(source) => Some(source),
        }
    }
}

/// Execute all five compiled hygiene arms over one admitted captured tree.
///
/// Only measurable jurisdiction entries are read. Parsing, graph construction,
/// symbol extraction, test classification, private-boundary resolution, policy
/// decisions, and elapsed time are derived inside this function; callers cannot
/// provide observations or measurement counts.
///
/// # Errors
///
/// Returns an internal contract error if source-derived observations cannot be
/// represented by the closed report schema.
pub fn analyze_hygiene(
    jurisdiction: &Jurisdiction,
    configuration: &HygieneConfiguration,
) -> Result<HygieneReport, HygieneAnalysisError> {
    let started = Instant::now();
    let snapshot = match jurisdiction.snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return failed_report(
                configuration,
                "selected-tree-capture",
                &format!("selected tree capture failed: {error}"),
            )
            .map_err(HygieneAnalysisError::Measurement);
        }
    };
    let report = AnalyzerRun::execute_at(snapshot.jurisdiction(), configuration, started)
        .map_err(HygieneAnalysisError::Measurement)?;
    match snapshot.verify() {
        Ok(None) if elapsed_ms(started) > configuration.deadline_ms => {
            timeout_report(configuration, elapsed_ms(started))
                .map_err(HygieneAnalysisError::Measurement)
        }
        Ok(None) => Ok(report),
        Ok(Some(change)) => failed_report(
            configuration,
            "selected-tree-verification",
            &format!("captured tree changed at {}", change.path),
        )
        .map_err(HygieneAnalysisError::Measurement),
        Err(error) => failed_report(
            configuration,
            "selected-tree-verification",
            &format!("captured tree verification failed: {error}"),
        )
        .map_err(HygieneAnalysisError::Measurement),
    }
}

fn timeout_report(
    configuration: &HygieneConfiguration,
    elapsed_ms: u64,
) -> Result<HygieneReport, HygieneError> {
    measure_hygiene(HYGIENE_ARMS.map(|declaration| {
        if let Some(observation) = exclusion_observation(configuration, declaration.id) {
            return observation;
        }
        HygieneObservation {
            gate_id: declaration.id,
            outcome: HygieneOutcome::Timeout,
            reason: "native hygiene deadline expired".to_owned(),
            evidence: vec![AnalysisEvidence::Deadline {
                limit_ms: configuration.deadline_ms,
                elapsed_ms,
                state: EvidenceState::TimedOut,
            }],
            findings: Vec::new(),
        }
    }))
}

fn failed_report(
    configuration: &HygieneConfiguration,
    stage: &str,
    reason: &str,
) -> Result<HygieneReport, HygieneError> {
    measure_hygiene(HYGIENE_ARMS.map(|declaration| {
        if let Some(observation) = exclusion_observation(configuration, declaration.id) {
            return observation;
        }
        HygieneObservation {
            gate_id: declaration.id,
            outcome: HygieneOutcome::Failed,
            reason: one_line(reason),
            evidence: vec![AnalysisEvidence::Measurement {
                state: EvidenceState::Failed,
                stage: stage.to_owned(),
            }],
            findings: Vec::new(),
        }
    }))
}

fn exclusion_observation(
    configuration: &HygieneConfiguration,
    gate_id: GateId,
) -> Option<HygieneObservation> {
    let exclusion = configuration
        .exclusions
        .iter()
        .find(|item| item.gate_id == gate_id)?;
    Some(HygieneObservation {
        gate_id,
        outcome: HygieneOutcome::Excluded,
        reason: exclusion.reason.clone(),
        evidence: vec![AnalysisEvidence::Exclusion(ExclusionEvidence {
            provenance: exclusion.provenance.clone(),
            reason: exclusion.reason.clone(),
        })],
        findings: Vec::new(),
    })
}

fn one_line(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

fn invalid_identity(value: &str) -> bool {
    value.is_empty() || value.contains(['\n', '\r'])
}

fn collect_paths(
    values: impl IntoIterator<Item = impl Into<String>>,
) -> Result<Vec<String>, HygieneAnalysisError> {
    let mut output = Vec::new();
    for value in values {
        let value = value.into();
        if invalid_selected_path(&value) || value.ends_with('/') || value.contains('\\') {
            return Err(HygieneAnalysisError::Configuration(format!(
                "hygiene root is not a normalized selected-tree path: {value:?}"
            )));
        }
        output.push(value);
    }
    Ok(output)
}

fn invalid_selected_path(path: &str) -> bool {
    path.is_empty()
        || path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
}

fn collect_identities(
    values: impl IntoIterator<Item = impl Into<String>>,
    kind: &str,
) -> Result<Vec<String>, HygieneAnalysisError> {
    let mut output = Vec::new();
    for value in values {
        let value = value.into();
        if invalid_identity(&value) {
            return Err(HygieneAnalysisError::Configuration(format!(
                "{kind} identities must be nonempty and single-line"
            )));
        }
        output.push(value);
    }
    output.sort();
    output.dedup();
    Ok(output)
}

#[derive(Clone, Debug)]
enum Terminal {
    Parsed,
    TimedOut {
        elapsed_ms: u64,
    },
    ParseFailure {
        documents: u32,
        path: String,
        diagnostic: String,
    },
    Failed {
        stage: &'static str,
    },
}

struct AnalyzerRun<'a> {
    configuration: &'a HygieneConfiguration,
    boundary: &'a dyn ExecutionBoundary,
    started: Instant,
    project_markers: Vec<String>,
    selected_files: u32,
    corpus: Corpus,
    terminal: Terminal,
}

impl<'a> AnalyzerRun<'a> {
    fn execute_at(
        jurisdiction: &'a Jurisdiction,
        configuration: &'a HygieneConfiguration,
        started: Instant,
    ) -> Result<HygieneReport, HygieneError> {
        Self::execute_with_at(jurisdiction, configuration, &NativeExecution, started)
    }

    #[cfg(test)]
    fn execute_with(
        jurisdiction: &'a Jurisdiction,
        configuration: &'a HygieneConfiguration,
        boundary: &'a dyn ExecutionBoundary,
    ) -> Result<HygieneReport, HygieneError> {
        Self::execute_with_at(jurisdiction, configuration, boundary, Instant::now())
    }

    fn execute_with_at(
        jurisdiction: &'a Jurisdiction,
        configuration: &'a HygieneConfiguration,
        boundary: &'a dyn ExecutionBoundary,
        started: Instant,
    ) -> Result<HygieneReport, HygieneError> {
        let project_markers = jurisdiction
            .measurable()
            .filter(|entry| matches!(entry.path(), "pyproject.toml" | "spice.toml"))
            .map(|entry| entry.path().to_owned())
            .collect();
        let (selected_files, corpus, terminal) = Corpus::load(jurisdiction, configuration, started);
        let run = Self {
            configuration,
            boundary,
            started,
            project_markers,
            selected_files,
            corpus,
            terminal,
        };
        let observations = HYGIENE_ARMS.map(|arm| run.observe(arm));
        measure_hygiene(observations)
    }

    fn observe(&self, declaration: HygieneDeclaration) -> HygieneObservation {
        if let Some(observation) = exclusion_observation(self.configuration, declaration.id) {
            return observation;
        }
        let analyzer = match self.discover(declaration) {
            Ok(analyzer) => analyzer,
            Err(observation) => return observation,
        };
        if let Some(observation) = self.terminal_state_observation(declaration.id) {
            return observation;
        }
        self.measure_observation(declaration, analyzer)
    }

    fn discover(
        &self,
        declaration: HygieneDeclaration,
    ) -> Result<AnalyzerEvidence, HygieneObservation> {
        if !self.boundary.scheduled(declaration.id) {
            return Err(self.terminal_observation(
                declaration.id,
                HygieneOutcome::Unmeasured,
                "hygiene scheduler did not dispatch the selected arm",
                AnalysisEvidence::Measurement {
                    state: EvidenceState::Unmeasured,
                    stage: "scheduler-dispatch".to_owned(),
                },
            ));
        }
        let analyzer = self.boundary.analyzer(declaration);
        let failure = if analyzer.is_unavailable() {
            Some((
                HygieneOutcome::Unavailable,
                "compiled hygiene analyzer is unavailable",
            ))
        } else if analyzer.is_unsupported() {
            Some((
                HygieneOutcome::Unsupported,
                "compiled hygiene analyzer violates its exact version contract",
            ))
        } else {
            None
        };
        if let Some((outcome, reason)) = failure {
            return Err(self.terminal_observation(
                declaration.id,
                outcome,
                reason,
                AnalysisEvidence::Analyzer(analyzer),
            ));
        }
        Ok(analyzer)
    }

    fn terminal_state_observation(&self, gate_id: GateId) -> Option<HygieneObservation> {
        let (outcome, reason, evidence) = match &self.terminal {
            Terminal::TimedOut { elapsed_ms } => (
                HygieneOutcome::Timeout,
                "native hygiene deadline expired",
                AnalysisEvidence::Deadline {
                    limit_ms: self.configuration.deadline_ms,
                    elapsed_ms: *elapsed_ms,
                    state: EvidenceState::TimedOut,
                },
            ),
            Terminal::ParseFailure {
                documents,
                path,
                diagnostic,
            } => (
                HygieneOutcome::ParseFailure,
                "selected Python source did not parse",
                AnalysisEvidence::Parse {
                    documents: *documents,
                    state: EvidenceState::ParseFailed,
                    failed_path: Some(path.clone()),
                    diagnostic: Some(diagnostic.clone()),
                },
            ),
            Terminal::Failed { stage } => (
                HygieneOutcome::Failed,
                "selected Python source could not be measured",
                AnalysisEvidence::Measurement {
                    state: EvidenceState::Failed,
                    stage: (*stage).to_owned(),
                },
            ),
            Terminal::Parsed => return None,
        };
        Some(self.terminal_observation(gate_id, outcome, reason, evidence))
    }

    fn measure_observation(
        &self,
        declaration: HygieneDeclaration,
        analyzer: AnalyzerEvidence,
    ) -> HygieneObservation {
        if self.elapsed_ms() > self.configuration.deadline_ms {
            return self.timeout_observation(declaration.id);
        }
        if !self.corpus.applicable(declaration.id) {
            return HygieneObservation {
                gate_id: declaration.id,
                outcome: HygieneOutcome::NotApplicable,
                reason: "compiled hygiene applicability predicate is false".to_owned(),
                evidence: vec![self.language_evidence(false)],
                findings: Vec::new(),
            };
        }
        let findings = self.corpus.findings(declaration.id, self.configuration);
        if self.elapsed_ms() > self.configuration.deadline_ms {
            return self.timeout_observation(declaration.id);
        }
        let limit = self.configuration.limit(declaration.id);
        let outcome = if u32::try_from(findings.len()).unwrap_or(u32::MAX) > limit {
            HygieneOutcome::Findings
        } else {
            HygieneOutcome::Passed
        };
        let evidence = self.completed_evidence(declaration, analyzer, findings.len(), limit);
        HygieneObservation {
            gate_id: declaration.id,
            outcome,
            reason: if outcome == HygieneOutcome::Passed {
                "source-derived findings are within the admitted policy limit".to_owned()
            } else {
                "source-derived findings exceed the admitted policy limit".to_owned()
            },
            evidence,
            findings,
        }
    }

    fn completed_evidence(
        &self,
        declaration: HygieneDeclaration,
        analyzer: AnalyzerEvidence,
        finding_count: usize,
        limit: u32,
    ) -> Vec<AnalysisEvidence> {
        let mut evidence = vec![
            self.language_evidence(true),
            AnalysisEvidence::Analyzer(analyzer),
            AnalysisEvidence::PolicyDecision {
                field: decision_field(declaration.id).to_owned(),
                limit,
                observed: u32::try_from(finding_count).unwrap_or(u32::MAX),
            },
            AnalysisEvidence::Deadline {
                limit_ms: self.configuration.deadline_ms,
                elapsed_ms: self.elapsed_ms().min(self.configuration.deadline_ms),
                state: EvidenceState::Completed,
            },
            AnalysisEvidence::Parse {
                documents: self.selected_files,
                state: EvidenceState::Completed,
                failed_path: None,
                diagnostic: None,
            },
            AnalysisEvidence::Measurement {
                state: EvidenceState::Completed,
                stage: "policy-decision".to_owned(),
            },
        ];
        for traversal in declaration.traversals {
            let item = self
                .corpus
                .traversal_evidence(*traversal, self.configuration);
            if !evidence
                .iter()
                .any(|existing| evidence_rank(existing) == evidence_rank(&item))
            {
                evidence.push(item);
            }
        }
        evidence
    }

    fn terminal_observation(
        &self,
        gate_id: GateId,
        outcome: HygieneOutcome,
        reason: &str,
        evidence: AnalysisEvidence,
    ) -> HygieneObservation {
        HygieneObservation {
            gate_id,
            outcome,
            reason: reason.to_owned(),
            evidence: vec![evidence],
            findings: Vec::new(),
        }
    }

    fn timeout_observation(&self, gate_id: GateId) -> HygieneObservation {
        self.terminal_observation(
            gate_id,
            HygieneOutcome::Timeout,
            "native hygiene deadline expired",
            AnalysisEvidence::Deadline {
                limit_ms: self.configuration.deadline_ms,
                elapsed_ms: self.elapsed_ms(),
                state: EvidenceState::TimedOut,
            },
        )
    }

    fn language_evidence(&self, applicable: bool) -> AnalysisEvidence {
        AnalysisEvidence::LanguageApplicability {
            language: "python".to_owned(),
            selected_files: self.selected_files,
            project_markers: self.project_markers.clone(),
            applicable,
            provenance: HygieneProvenance {
                release: "current".to_owned(),
                layer: "compiled".to_owned(),
                document: "<compiled-gate-battery>".to_owned(),
                field: "gate::hygiene::HygieneJurisdiction::applicability".to_owned(),
            },
        }
    }

    fn elapsed_ms(&self) -> u64 {
        elapsed_ms(self.started)
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl HygieneConfiguration {
    const fn limit(&self, gate: GateId) -> u32 {
        match gate {
            GateId::Reachability => self.reachability_limit,
            GateId::AssertionFreeTests => self.assertion_free_limit,
            _ => 0,
        }
    }
}

fn decision_field(gate: GateId) -> &'static str {
    match gate {
        GateId::Reachability => "policy.debt.reachability_test_only",
        GateId::AssertionFreeTests => "policy.debt.assertion_free_tests",
        _ => "compiled:zero-findings",
    }
}

trait ExecutionBoundary {
    fn scheduled(&self, gate: GateId) -> bool;
    fn analyzer(&self, declaration: HygieneDeclaration) -> AnalyzerEvidence;
}

struct NativeExecution;

impl ExecutionBoundary for NativeExecution {
    fn scheduled(&self, _gate: GateId) -> bool {
        true
    }

    fn analyzer(&self, declaration: HygieneDeclaration) -> AnalyzerEvidence {
        match native_executable_digest() {
            Some(digest) => AnalyzerEvidence::Identified {
                name: declaration.analyzer.evidence_name().to_owned(),
                requested: declaration.analyzer.component().to_owned(),
                resolved: declaration.analyzer.component().to_owned(),
                declared_version: NATIVE_ANALYZER_VERSION.to_owned(),
                observed_version: NATIVE_ANALYZER_VERSION.to_owned(),
                digest: digest.to_owned(),
            },
            None => AnalyzerEvidence::Unavailable {
                name: declaration.analyzer.evidence_name().to_owned(),
                requested: declaration.analyzer.component().to_owned(),
            },
        }
    }
}

fn native_executable_digest() -> Option<&'static str> {
    static DIGEST: OnceLock<Option<String>> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            let executable = std::env::current_exe().ok()?;
            let bytes = fs::read(executable).ok()?;
            Some(Digest::compute(Domain::GateTool, &bytes).to_string())
        })
        .as_deref()
}

include!("analyze/syntax.rs");
include!("analyze/corpus.rs");

#[cfg(test)]
include!("analyze/tests.rs");
