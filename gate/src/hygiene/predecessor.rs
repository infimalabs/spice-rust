use std::error::Error;
use std::fmt;

use crate::manifest::PREDECESSOR_GATE_RELEASE;

use super::measure::canonicalize_findings;
use super::*;

pub(super) const PREDECESSOR_PROVIDER_FIELD: &str = "policy.reachability_providers";

/// Normalized result retained from one predecessor analyzer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorResult {
    /// Predecessor arm completed cleanly.
    Passed,
    /// Predecessor arm emitted normalized structured findings.
    Findings(Vec<HygieneFinding>),
}

/// One normalized release-owned predecessor observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PredecessorObservation {
    /// Exact predecessor arm name.
    pub gate_name: String,
    /// Normalized decision.
    pub result: PredecessorResult,
}

/// Exact release-owned refusal and current recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRefusal {
    /// Release that owned the incompatible execution.
    pub release: &'static str,
    /// Stable current gate identity.
    pub gate_id: GateId,
    /// Exact predecessor field.
    pub source_field: &'static str,
    /// Why the evidence is not admissible now.
    pub reason: &'static str,
    /// Exact current recovery.
    pub recovery: &'static str,
}

/// Result of comparing normalized predecessor and current decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparison {
    /// Native built-in decisions and finding identities match.
    Equivalent,
    /// Unversioned external provider evidence cannot migrate.
    Refused(MigrationRefusal),
}

/// Why predecessor comparison was malformed or unequal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparisonError {
    /// Name was not one of the five release-owned arms.
    UnknownGate(String),
    /// Current measurement named a different arm.
    GateMismatch { expected: GateId, actual: GateId },
    /// Normalized built-in decisions differed.
    ResultMismatch(GateId),
}

impl fmt::Display for PredecessorComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownGate(gate) => write!(
                formatter,
                "release {PREDECESSOR_GATE_RELEASE} has no hygiene arm {gate:?}"
            ),
            Self::GateMismatch { expected, actual } => write!(
                formatter,
                "predecessor {} was compared with current {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::ResultMismatch(gate) => write!(
                formatter,
                "{} predecessor/current results differ",
                gate.as_str()
            ),
        }
    }
}

impl Error for PredecessorComparisonError {}

/// Compare a normalized predecessor observation with one current measurement.
///
/// # Errors
///
/// Returns an unknown name, gate mismatch, or unequal built-in decision.
pub fn compare_predecessor(
    predecessor: &PredecessorObservation,
    current: &HygieneMeasurement,
) -> Result<PredecessorComparison, PredecessorComparisonError> {
    let Some(gate) = predecessor_gate(&predecessor.gate_name) else {
        return Err(PredecessorComparisonError::UnknownGate(
            predecessor.gate_name.clone(),
        ));
    };
    if current.gate_id != gate {
        return Err(PredecessorComparisonError::GateMismatch {
            expected: gate,
            actual: current.gate_id,
        });
    }
    if predecessor_has_external_provider(&predecessor.result) || current.has_external_provider() {
        return Ok(PredecessorComparison::Refused(provider_refusal(gate)));
    }
    if let Some(refusal) = state_refusal(gate, current.outcome) {
        return Ok(PredecessorComparison::Refused(refusal));
    }
    if predecessor_matches(&predecessor.result, current) {
        Ok(PredecessorComparison::Equivalent)
    } else {
        Err(PredecessorComparisonError::ResultMismatch(gate))
    }
}

fn predecessor_gate(name: &str) -> Option<GateId> {
    match name {
        "reachability" => Some(GateId::Reachability),
        "symbol-reachability" => Some(GateId::SymbolReachability),
        "python-unused" => Some(GateId::PythonUnused),
        "assertion-free-tests" => Some(GateId::AssertionFreeTests),
        "private-internals" => Some(GateId::PrivateInternals),
        _ => None,
    }
}

fn predecessor_has_external_provider(result: &PredecessorResult) -> bool {
    match result {
        PredecessorResult::Passed => false,
        PredecessorResult::Findings(findings) => findings
            .iter()
            .any(|finding| finding.detail.has_external_provider()),
    }
}

fn predecessor_matches(result: &PredecessorResult, current: &HygieneMeasurement) -> bool {
    match (result, current.outcome) {
        (
            PredecessorResult::Passed,
            HygieneOutcome::Passed | HygieneOutcome::NotApplicable | HygieneOutcome::Excluded,
        ) => true,
        (PredecessorResult::Findings(findings), HygieneOutcome::Findings) => {
            let mut normalized = findings.clone();
            canonicalize_findings(&mut normalized);
            normalized == current.findings
        }
        _ => false,
    }
}

fn state_refusal(gate: GateId, outcome: HygieneOutcome) -> Option<MigrationRefusal> {
    let reason = match outcome {
        HygieneOutcome::Unavailable => concat!(
            "0.31.0 retained no independent analyzer or provider ",
            "availability identity"
        ),
        HygieneOutcome::Unsupported => concat!(
            "0.31.0 retained no exact analyzer or provider version ",
            "contract"
        ),
        HygieneOutcome::Unmeasured => "0.31.0 retained no explicit unmeasured stage evidence",
        HygieneOutcome::Timeout => concat!(
            "0.31.0 timeout evidence cannot establish the current ",
            "completed traversal"
        ),
        HygieneOutcome::ParseFailure => concat!(
            "0.31.0 Python AST analyzers silently omitted syntax and ",
            "read failures"
        ),
        HygieneOutcome::Failed => concat!(
            "0.31.0 exception text does not identify the current ",
            "typed failure stage"
        ),
        HygieneOutcome::Passed
        | HygieneOutcome::Findings
        | HygieneOutcome::NotApplicable
        | HygieneOutcome::Excluded => return None,
    };
    Some(MigrationRefusal {
        release: PREDECESSOR_GATE_RELEASE,
        gate_id: gate,
        source_field: predecessor_arm_field(gate),
        reason,
        recovery: concat!(
            "rerun the current compiled arm and retain its analyzer, ",
            "deadline, parser, traversal, and measurement evidence"
        ),
    })
}

pub(super) const fn predecessor_arm_field(gate: GateId) -> &'static str {
    match gate {
        GateId::Reachability => "policy.pre_commit_builtins.reachability",
        GateId::SymbolReachability => "policy.pre_commit_builtins.symbol-reachability",
        GateId::PythonUnused => "policy.pre_commit_builtins.python-unused",
        GateId::AssertionFreeTests => "policy.pre_commit_builtins.assertion-free-tests",
        GateId::PrivateInternals => "policy.pre_commit_builtins.private-internals",
        _ => "policy.pre_commit_builtins",
    }
}

fn provider_refusal(gate: GateId) -> MigrationRefusal {
    MigrationRefusal {
        release: PREDECESSOR_GATE_RELEASE,
        gate_id: gate,
        source_field: PREDECESSOR_PROVIDER_FIELD,
        reason: concat!(
            "0.31.0 admitted arbitrary provider argv without an exact ",
            "executable version or digest"
        ),
        recovery: concat!(
            "declare the provider program, version probe, exact version, ",
            "digest, arguments, scope, and timeout, then rerun the current arm"
        ),
    }
}
