use std::error::Error;
use std::fmt;

use crate::manifest::{GateId, PREDECESSOR_GATE_RELEASE};

use super::{
    ArmFinding, ArmMeasurement, ArmOutcome, EnvironmentLedgerKind, FindingDetail,
    JavascriptUnusedStatus, SourceLocus, normalize_findings, validate_findings,
};

/// Exact policy profile that governed one predecessor observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorConfiguration {
    /// Release 0.31.0 packaged values with no repository/operator override.
    PackagedDefaults,
    /// A layered predecessor field changed the packaged scanner profile.
    Override {
        /// Exact winning predecessor source field.
        source_field: String,
    },
}

/// Release-shaped finding retained without first converting it to current data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorFinding {
    /// Opaque formatter/linter output retained across the executor refusal.
    FormattingOutput { bytes: Vec<u8> },
    /// Absolute macOS user-path marker.
    LocalPath { path: String, line: u32 },
    /// Configured prose token and suggestion.
    Taste {
        path: String,
        line: u32,
        word: String,
        suggestion: String,
    },
    /// Opaque predecessor web typecheck output retained across the executor refusal.
    AppWebOutput { bytes: Vec<u8> },
    /// Complete predecessor JavaScript declaration evidence.
    JavascriptUnused {
        path: String,
        line: u32,
        declaration_kind: String,
        symbol: String,
        status: JavascriptUnusedStatus,
        reason: String,
        reference_count: u32,
        test_reference_count: u32,
    },
    /// Opaque Pyright output retained across the executor refusal.
    PythonTypeOutput { bytes: Vec<u8> },
    /// Exact unwaived environment source line.
    EnvironmentAccess {
        path: String,
        line: u32,
        name: String,
        source: String,
    },
    /// Repository-wide environment declaration/reference mismatch.
    EnvironmentLedger {
        kind: EnvironmentLedgerKind,
        name: String,
        references: Vec<SourceLocus>,
    },
}

impl PredecessorFinding {
    const fn gate(&self) -> GateId {
        match self {
            Self::FormattingOutput { .. } => GateId::Formatters,
            Self::LocalPath { .. } => GateId::LocalPaths,
            Self::Taste { .. } => GateId::Taste,
            Self::AppWebOutput { .. } => GateId::AppWebTypecheck,
            Self::JavascriptUnused { .. } => GateId::JavascriptUnused,
            Self::PythonTypeOutput { .. } => GateId::PythonTypecheck,
            Self::EnvironmentAccess { .. } => GateId::EnvPolicy,
            Self::EnvironmentLedger { .. } => GateId::EnvNameLedger,
        }
    }

    fn to_current(&self) -> ArmFinding {
        self.scanner_to_current()
    }

    fn has_valid_shape(&self) -> bool {
        match self {
            Self::FormattingOutput { bytes }
            | Self::AppWebOutput { bytes }
            | Self::PythonTypeOutput { bytes } => !bytes.is_empty(),
            _ => {
                let mut finding = self.to_current();
                normalize_findings(std::slice::from_mut(&mut finding));
                validate_findings(self.gate(), ArmOutcome::Findings, &[finding]).is_ok()
            }
        }
    }

    /// The current-shaped finding one release-shaped scanner result names.
    ///
    /// Every column here is empty because release 0.31.0 reported a path and a
    /// line and nothing finer, so the column is a locus the predecessor cannot
    /// supply rather than one it disagreed about. Scanner equality therefore
    /// drops the field on both sides; comparing it would report every current
    /// finding as changed for saying where in its line it sits, and filling one
    /// in here would invent a number no predecessor run ever produced.
    ///
    /// Two occurrences on one line are one place to the predecessor and two to
    /// the current scanners, which is the granularity this comparison holds.
    fn scanner_to_current(&self) -> ArmFinding {
        match self {
            Self::LocalPath { path, line } => ArmFinding {
                path: Some(path.clone()),
                line: Some(*line),
                column: None,
                detail: FindingDetail::LocalPath,
            },
            Self::Taste {
                path,
                line,
                word,
                suggestion,
            } => ArmFinding {
                path: Some(path.clone()),
                line: Some(*line),
                column: None,
                detail: FindingDetail::Taste {
                    word: word.clone(),
                    replacement: suggestion.clone(),
                },
            },
            Self::JavascriptUnused { .. } => self.javascript_to_current(),
            Self::EnvironmentAccess {
                path,
                line,
                name,
                source,
            } => ArmFinding {
                path: Some(path.clone()),
                line: Some(*line),
                column: None,
                detail: FindingDetail::EnvironmentAccess {
                    name: name.clone(),
                    source: source.clone(),
                },
            },
            Self::EnvironmentLedger {
                kind,
                name,
                references,
            } => ArmFinding {
                path: None,
                line: None,
                column: None,
                detail: FindingDetail::EnvironmentLedger {
                    mismatch: *kind,
                    name: name.clone(),
                    references: references.clone(),
                },
            },
            _ => unreachable!("scanner conversion is selected from the closed finding variant"),
        }
    }

    fn javascript_to_current(&self) -> ArmFinding {
        let Self::JavascriptUnused {
            path,
            line,
            declaration_kind,
            symbol,
            status,
            reason,
            reference_count,
            test_reference_count,
        } = self
        else {
            unreachable!("JavaScript conversion is selected from the closed finding variant");
        };
        ArmFinding {
            path: Some(path.clone()),
            line: Some(*line),
            column: None,
            detail: FindingDetail::JavascriptUnused {
                symbol: symbol.clone(),
                declaration_kind: declaration_kind.clone(),
                status: *status,
                reason: reason.clone(),
                reference_count: *reference_count,
                test_reference_count: *test_reference_count,
            },
        }
    }
}

/// Result retained from one supported predecessor scanner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorResult {
    /// Predecessor scanner completed cleanly.
    Passed,
    /// Predecessor scanner emitted release-shaped structured findings.
    Findings(Vec<PredecessorFinding>),
}

/// One release-owned predecessor observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PredecessorObservation {
    /// Exact 0.31.0 arm name, including `serve-web-typecheck`.
    pub gate_name: String,
    /// Exact release policy profile which governed the observation.
    pub configuration: PredecessorConfiguration,
    /// Retained release-shaped result.
    pub result: PredecessorResult,
}

/// Exact refusal and recovery for a predecessor observation without parity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRefusal {
    /// Release that owned the incompatible behavior.
    pub release: &'static str,
    /// Current stable identity that supersedes the old arm.
    pub gate_id: GateId,
    /// Exact predecessor configuration field.
    pub source_field: String,
    /// Why old evidence cannot be interpreted as current evidence.
    pub reason: &'static str,
    /// Exact current recovery action.
    pub recovery: &'static str,
}

/// Result of comparing one predecessor observation with a current measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparison {
    /// Normalized scanner result is identical.
    Equivalent,
    /// Incompatible execution semantics are retained as an explicit boundary.
    Refused(MigrationRefusal),
}

/// Why a supported predecessor comparison was malformed or unequal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorComparisonError {
    /// Name was not one of the eight release-owned predecessor arms.
    UnknownGate(String),
    /// Current measurement named a different stable gate.
    GateMismatch { expected: GateId, actual: GateId },
    /// Supported pure-scanner results differed.
    ResultMismatch(GateId),
    /// An override refusal omitted a usable source field.
    InvalidConfigurationField(GateId),
}

impl fmt::Display for PredecessorComparisonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownGate(gate) => write!(
                formatter,
                "release {PREDECESSOR_GATE_RELEASE} has no toolchain arm {gate:?}"
            ),
            Self::GateMismatch { expected, actual } => write!(
                formatter,
                "predecessor {} observation was compared with current {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::ResultMismatch(gate) => {
                write!(
                    formatter,
                    "{} predecessor/current results differ",
                    gate.as_str()
                )
            }
            Self::InvalidConfigurationField(gate) => write!(
                formatter,
                "{} predecessor configuration field is empty or multiline",
                gate.as_str()
            ),
        }
    }
}

impl Error for PredecessorComparisonError {}

/// Compare a supported release-shaped scanner observation or return the exact
/// release boundary for an incompatible predecessor executor.
///
/// # Errors
///
/// Returns an unknown name, gate mismatch, malformed override field, or unequal
/// supported result.
pub fn compare_predecessor(
    predecessor: &PredecessorObservation,
    current: &ArmMeasurement,
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
    validate_predecessor_observation(predecessor, gate)?;
    if let Some(refusal) = predecessor_refusal(gate, &predecessor.configuration) {
        return Ok(PredecessorComparison::Refused(refusal));
    }

    let equivalent = match (&predecessor.result, current.outcome) {
        (PredecessorResult::Passed, ArmOutcome::Passed) => true,
        (PredecessorResult::Findings(findings), ArmOutcome::Findings) => {
            let released: Vec<_> = findings
                .iter()
                .map(PredecessorFinding::to_current)
                .collect();
            comparable(released) == comparable(current.findings.clone())
        }
        _ => false,
    };
    if equivalent {
        Ok(PredecessorComparison::Equivalent)
    } else {
        Err(PredecessorComparisonError::ResultMismatch(gate))
    }
}

/// One canonical finding list for scanner equality, without the column.
///
/// Both sides pass through here rather than the predecessor alone, so dropping
/// the column is a property of the comparison instead of an artifact of which
/// producer left the field empty. Erasing it can reorder a list the current
/// measurement already sorted, which is why the sort is repeated after.
fn comparable(mut findings: Vec<ArmFinding>) -> Vec<ArmFinding> {
    for finding in &mut findings {
        finding.column = None;
    }
    normalize_findings(&mut findings);
    findings.sort();
    findings
}

fn validate_predecessor_observation(
    predecessor: &PredecessorObservation,
    gate: GateId,
) -> Result<(), PredecessorComparisonError> {
    if let PredecessorConfiguration::Override { source_field } = &predecessor.configuration
        && (source_field.is_empty() || source_field.contains(['\n', '\r']))
    {
        return Err(PredecessorComparisonError::InvalidConfigurationField(gate));
    }
    if let PredecessorResult::Findings(findings) = &predecessor.result
        && (findings.is_empty()
            || findings
                .iter()
                .any(|finding| finding.gate() != gate || !finding.has_valid_shape()))
    {
        return Err(PredecessorComparisonError::ResultMismatch(gate));
    }
    Ok(())
}

fn predecessor_gate(name: &str) -> Option<GateId> {
    match name {
        "formatters" => Some(GateId::Formatters),
        "local-paths" => Some(GateId::LocalPaths),
        "taste" => Some(GateId::Taste),
        "serve-web-typecheck" => Some(GateId::AppWebTypecheck),
        "javascript-unused" => Some(GateId::JavascriptUnused),
        "python-typecheck" => Some(GateId::PythonTypecheck),
        "env-policy" => Some(GateId::EnvPolicy),
        "env-name-ledger" => Some(GateId::EnvNameLedger),
        _ => None,
    }
}

fn predecessor_refusal(
    gate: GateId,
    configuration: &PredecessorConfiguration,
) -> Option<MigrationRefusal> {
    let (name, reason, recovery) = match gate {
        GateId::Formatters => (
            "formatters",
            "0.31.0 mutated and re-staged Python files through a range-selected ruff",
            concat!(
                "rerun formatters with cargo 1.97.1 and rustfmt 1.9.0-stable ",
                "under the current declaration"
            ),
        ),
        GateId::AppWebTypecheck => (
            "serve-web-typecheck",
            "0.31.0 checked removed serve JavaScript with an unpinned npm TypeScript package",
            "rerun app-web-typecheck against app/serve with cargo 1.97.1 and trunk 0.21.14",
        ),
        GateId::PythonTypecheck => (
            "python-typecheck",
            "0.31.0 selected pyright or uvx dynamically and admitted a version range",
            "install pyright 1.1.411 and rerun python-typecheck through its sole current program",
        ),
        _ => {
            let PredecessorConfiguration::Override { source_field } = configuration else {
                return None;
            };
            return Some(scanner_override_refusal(gate, source_field));
        }
    };
    let source_field = match configuration {
        PredecessorConfiguration::PackagedDefaults => {
            format!("policy.pre_commit_builtins.{name}")
        }
        PredecessorConfiguration::Override { source_field } => source_field.clone(),
    };
    Some(MigrationRefusal {
        release: PREDECESSOR_GATE_RELEASE,
        gate_id: gate,
        source_field,
        reason,
        recovery,
    })
}

fn scanner_override_refusal(gate: GateId, source_field: &str) -> MigrationRefusal {
    let (reason, recovery) = match gate {
        GateId::LocalPaths => (
            "0.31.0 applied layered policy.exclude paths outside the packaged scanner profile",
            concat!(
                "remove the predecessor policy.exclude override or encode the exclusion in ",
                "current selected-tree policy, then rerun local-paths"
            ),
        ),
        GateId::Taste => (
            "0.31.0 merged layered taste words or exclusions into the packaged scanner profile",
            concat!(
                "materialize the intended words in current compiled taste policy, remove the ",
                "predecessor override, and rerun taste"
            ),
        ),
        GateId::JavascriptUnused => (
            concat!(
                "0.31.0 resolved configured test paths or exclusions before JavaScript ",
                "reference classification"
            ),
            concat!(
                "materialize the intended test roots and exclusions in current compiled policy, ",
                "remove the predecessor override, and rerun javascript-unused"
            ),
        ),
        GateId::EnvPolicy => (
            concat!(
                "0.31.0 resolved layered environment suffix, matcher, waiver, baseline, or ",
                "exclusion policy"
            ),
            concat!(
                "materialize the environment policy in the current compiled profile, remove the ",
                "predecessor override, and rerun env-policy"
            ),
        ),
        GateId::EnvNameLedger => (
            "0.31.0 resolved layered environment names, suffixes, matchers, or exclusions",
            concat!(
                "materialize the environment ledger policy in the current compiled profile, ",
                "remove the predecessor override, and rerun env-name-ledger"
            ),
        ),
        _ => unreachable!("external executor refusals return before scanner overrides"),
    };
    MigrationRefusal {
        release: PREDECESSOR_GATE_RELEASE,
        gate_id: gate,
        source_field: source_field.to_owned(),
        reason,
        recovery,
    }
}
