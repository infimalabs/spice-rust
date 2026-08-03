use serde::{Deserialize, Serialize};
use wire::Digest;

use super::{ArmFamily, BatteryError, valid_text};
use crate::core::{CoreArmReport, CoreStanding};
use crate::hygiene::{AnalysisEvidence, HygieneMeasurement, HygieneOutcome, HygieneProvenance};
use crate::manifest::GateId;
use crate::metrics::{EvidenceState as MetricState, StructuralArmEvidence};
use crate::toolchain::{ArmMeasurement, ArmOutcome};

/// Exact common battery result vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BatteryState {
    Passed,
    Failed,
    Unavailable,
    Unsupported,
    Unmeasured,
    TimedOut,
    ExplicitSkip,
    NotApplicable,
    Excluded,
}

impl BatteryState {
    /// Only pass and contract-owned absence/exclusion admit publication.
    #[must_use]
    pub const fn admits(self) -> bool {
        matches!(self, Self::Passed | Self::NotApplicable | Self::Excluded)
    }
}

/// Exact provenance required for an admitted non-execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResultProvenance {
    release: String,
    source: String,
    field: Vec<String>,
}

impl ResultProvenance {
    /// Bind an applicability/exclusion decision to its exact source field.
    pub fn new(
        release: impl Into<String>,
        source: impl Into<String>,
        field: Vec<String>,
    ) -> Result<Self, BatteryError> {
        let provenance = Self {
            release: release.into(),
            source: source.into(),
            field,
        };
        provenance.validate()?;
        Ok(provenance)
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if !valid_text(&self.release)
            || !valid_text(&self.source)
            || self.field.is_empty()
            || self.field.iter().any(|part| !valid_text(part))
        {
            return Err(BatteryError::new(
                "result provenance requires release, source, and exact field path",
            ));
        }
        Ok(())
    }

    /// Release that owns the decision grammar.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// Exact policy or declaration source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Exact structurally segmented field.
    #[must_use]
    pub fn field(&self) -> &[String] {
        &self.field
    }
}

/// Existing family-owned typed evidence retained by one battery position.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "family")]
pub enum ArmEvidence {
    Core {
        report: CoreArmReport,
    },
    Toolchain {
        report: ArmMeasurement,
    },
    Metrics {
        report: StructuralArmEvidence,
    },
    Hygiene {
        report: HygieneMeasurement,
    },
    ExecutorFailure {
        gate_id: GateId,
        arm_family: ArmFamily,
        declaration: String,
        reason: String,
    },
}

impl ArmEvidence {
    /// Family represented by compiled evidence.
    #[must_use]
    pub const fn family(&self) -> ArmFamily {
        match self {
            Self::Core { .. } => ArmFamily::Core,
            Self::Toolchain { .. } => ArmFamily::Toolchain,
            Self::Metrics { .. } => ArmFamily::Metrics,
            Self::Hygiene { .. } => ArmFamily::Hygiene,
            Self::ExecutorFailure { arm_family, .. } => *arm_family,
        }
    }

    /// Gate identity carried by the family report.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        match self {
            Self::Core { report } => report.id(),
            Self::Toolchain { report } => report.gate_id,
            Self::Metrics { report } => report.gate_id(),
            Self::Hygiene { report } => report.gate_id,
            Self::ExecutorFailure { gate_id, .. } => *gate_id,
        }
    }

    /// Typed terminal state derived from the retained family authority.
    pub fn state(&self) -> Result<BatteryState, BatteryError> {
        match self {
            Self::Core { report } => Ok(core_state(report.standing())),
            Self::Toolchain { report } => Ok(toolchain_state(report.outcome)),
            Self::Metrics { report } => metric_state(report),
            Self::Hygiene { report } => Ok(hygiene_state(report.outcome)),
            Self::ExecutorFailure { .. } => Ok(BatteryState::Failed),
        }
    }

    /// Exact result reason derived from the retained family authority.
    pub fn reason(&self) -> Result<String, BatteryError> {
        match self {
            Self::Core { report } => Ok(report.diagnostic().to_owned()),
            Self::Toolchain { report } => Ok(report.reason.clone()),
            Self::Metrics { report } => metric_reason(report),
            Self::Hygiene { report } => hygiene_reason(report),
            Self::ExecutorFailure { reason, .. } => Ok(reason.clone()),
        }
    }

    /// Declaration-owned provenance for applicability or exclusion.
    pub fn provenance(&self) -> Result<Option<ResultProvenance>, BatteryError> {
        match self {
            Self::Toolchain { report } if report.outcome.is_skip() => {
                Ok(Some(ResultProvenance::new(
                    "current",
                    report.configuration_source.clone(),
                    vec![report.configuration_field.clone()],
                )?))
            }
            Self::Metrics { report } if metric_state(report)? == BatteryState::NotApplicable => {
                Ok(Some(ResultProvenance::new(
                    "current",
                    format!("gate::metrics::{}", report.gate_id()),
                    vec!["compiled-applicability".to_owned()],
                )?))
            }
            Self::Hygiene { report } if report.outcome.is_skip() => {
                Ok(Some(hygiene_provenance(report)?))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn validate_execution(
        &self,
        state: BatteryState,
        reason: &str,
        provenance: Option<&ResultProvenance>,
    ) -> Result<(), BatteryError> {
        self.validate()?;
        if self.state()? != state || self.reason()? != reason {
            return Err(BatteryError::new(
                "arm state or reason disagrees with its retained family evidence",
            ));
        }
        let expected = self.provenance()?;
        if provenance != expected.as_ref() {
            return Err(BatteryError::new(
                "arm provenance disagrees with its declaration-owned skip evidence",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_expected(
        &self,
        family: ArmFamily,
        gate_id: GateId,
        declaration: &str,
        source: &Digest,
    ) -> Result<(), BatteryError> {
        if self.family() != family || self.gate_id() != gate_id {
            return Err(BatteryError::new(
                "arm evidence belongs to a different compiled gate or family",
            ));
        }
        match self {
            Self::Core { report } if report.tree() != source => Err(BatteryError::new(
                "core arm evidence belongs to a different immutable tree",
            )),
            Self::ExecutorFailure {
                declaration: observed,
                ..
            } if observed != declaration => Err(BatteryError::new(
                "executor failure names a different compiled declaration",
            )),
            _ => Ok(()),
        }
    }

    fn validate(&self) -> Result<(), BatteryError> {
        match self {
            Self::Core { report } => report
                .validate()
                .map_err(|error| BatteryError::new(format!("invalid core evidence: {error}"))),
            Self::Toolchain { report } => report
                .validate()
                .map_err(|error| BatteryError::new(format!("invalid toolchain evidence: {error}"))),
            Self::Metrics { report } => report
                .validate()
                .map_err(|error| BatteryError::new(format!("invalid metric evidence: {error}"))),
            Self::Hygiene { report } => report
                .validate()
                .map_err(|error| BatteryError::new(format!("invalid hygiene evidence: {error}"))),
            Self::ExecutorFailure {
                declaration,
                reason,
                ..
            } if valid_text(declaration) && valid_text(reason) => Ok(()),
            Self::ExecutorFailure { .. } => Err(BatteryError::new(
                "executor failure requires declaration and one-line reason",
            )),
        }
    }
}

const fn core_state(standing: CoreStanding) -> BatteryState {
    match standing {
        CoreStanding::Passed => BatteryState::Passed,
        CoreStanding::Failed => BatteryState::Failed,
        CoreStanding::Unavailable => BatteryState::Unavailable,
        CoreStanding::Unsupported => BatteryState::Unsupported,
        CoreStanding::Unmeasured => BatteryState::Unmeasured,
        CoreStanding::TimedOut => BatteryState::TimedOut,
        CoreStanding::Skipped => BatteryState::ExplicitSkip,
    }
}

const fn toolchain_state(outcome: ArmOutcome) -> BatteryState {
    match outcome {
        ArmOutcome::Passed => BatteryState::Passed,
        ArmOutcome::ToolUnavailable => BatteryState::Unavailable,
        ArmOutcome::UnsupportedVersion => BatteryState::Unsupported,
        ArmOutcome::NotApplicable => BatteryState::NotApplicable,
        ArmOutcome::Excluded => BatteryState::Excluded,
        ArmOutcome::Timeout => BatteryState::TimedOut,
        ArmOutcome::Findings | ArmOutcome::ParseFailure | ArmOutcome::NonzeroAnalysis => {
            BatteryState::Failed
        }
    }
}

const fn hygiene_state(outcome: HygieneOutcome) -> BatteryState {
    match outcome {
        HygieneOutcome::Passed => BatteryState::Passed,
        HygieneOutcome::NotApplicable => BatteryState::NotApplicable,
        HygieneOutcome::Excluded => BatteryState::Excluded,
        HygieneOutcome::Unavailable => BatteryState::Unavailable,
        HygieneOutcome::Unsupported => BatteryState::Unsupported,
        HygieneOutcome::Unmeasured => BatteryState::Unmeasured,
        HygieneOutcome::Timeout => BatteryState::TimedOut,
        HygieneOutcome::Findings | HygieneOutcome::ParseFailure | HygieneOutcome::Failed => {
            BatteryState::Failed
        }
    }
}

fn metric_state(report: &StructuralArmEvidence) -> Result<BatteryState, BatteryError> {
    let mut observed = None;
    for metric in report.metrics() {
        let state = metric_terminal(metric.state());
        observed = Some(match (observed, state) {
            (None, state) => state,
            (Some(BatteryState::Passed | BatteryState::Failed), BatteryState::Passed) => {
                observed.expect("measured state exists")
            }
            (Some(BatteryState::Passed | BatteryState::Failed), BatteryState::Failed) => {
                BatteryState::Failed
            }
            (Some(previous), current) if previous == current => previous,
            _ => {
                return Err(BatteryError::new(
                    "metric arm mixes incompatible terminal evidence states",
                ));
            }
        });
    }
    observed.ok_or_else(|| BatteryError::new("metric arm evidence must not be empty"))
}

fn metric_terminal(state: &MetricState) -> BatteryState {
    match state {
        MetricState::Measured { measurement }
            if measurement.latch().is_some_and(|latch| latch.over_limit())
                || measurement
                    .threshold()
                    .is_some_and(|threshold| measurement.value() > threshold) =>
        {
            BatteryState::Failed
        }
        MetricState::Measured { .. } => BatteryState::Passed,
        MetricState::Unavailable { .. } => BatteryState::Unavailable,
        MetricState::Unsupported { .. } => BatteryState::Unsupported,
        MetricState::Unmeasured { .. } => BatteryState::Unmeasured,
        MetricState::Timeout { .. } => BatteryState::TimedOut,
        MetricState::Failed { .. } => BatteryState::Failed,
        MetricState::NotApplicable { .. } => BatteryState::NotApplicable,
    }
}

fn metric_reason(report: &StructuralArmEvidence) -> Result<String, BatteryError> {
    let state = metric_state(report)?;
    if state == BatteryState::Passed {
        return Ok("all declared structural measurements completed within threshold".to_owned());
    }
    if report
        .metrics()
        .iter()
        .any(|metric| matches!(metric.state(), MetricState::Measured { .. }))
    {
        return Ok("one or more structural measurements exceeded threshold".to_owned());
    }
    let first = report
        .metrics()
        .first()
        .ok_or_else(|| BatteryError::new("metric arm evidence must not be empty"))?;
    Ok(match first.state() {
        MetricState::Unavailable { reason }
        | MetricState::Unmeasured { reason }
        | MetricState::Timeout { reason, .. }
        | MetricState::Failed { reason, .. }
        | MetricState::NotApplicable { reason }
        | MetricState::Unsupported { reason, .. } => reason.clone(),
        MetricState::Measured { .. } => unreachable!("measured metric reason handled above"),
    })
}

fn hygiene_reason(report: &HygieneMeasurement) -> Result<String, BatteryError> {
    if report.outcome == HygieneOutcome::Excluded {
        let exclusion = report.evidence.iter().find_map(|evidence| match evidence {
            AnalysisEvidence::Exclusion(exclusion) => Some(exclusion),
            _ => None,
        });
        return exclusion
            .map(|evidence| evidence.reason.clone())
            .ok_or_else(|| BatteryError::new("excluded hygiene result lacks exclusion evidence"));
    }
    Ok(report.reason.clone())
}

fn hygiene_provenance(report: &HygieneMeasurement) -> Result<ResultProvenance, BatteryError> {
    let provenance = report.evidence.iter().find_map(|evidence| match evidence {
        AnalysisEvidence::LanguageApplicability {
            applicable: false,
            provenance,
            ..
        } => Some(provenance),
        AnalysisEvidence::Exclusion(exclusion) => Some(&exclusion.provenance),
        _ => None,
    });
    let provenance = provenance
        .ok_or_else(|| BatteryError::new("hygiene skip lacks declaration-owned provenance"))?;
    provenance_from_hygiene(provenance)
}

fn provenance_from_hygiene(
    provenance: &HygieneProvenance,
) -> Result<ResultProvenance, BatteryError> {
    let evidence = ResultProvenance {
        release: provenance.release.clone(),
        source: provenance.document.clone(),
        field: vec![provenance.layer.clone(), provenance.field.clone()],
    };
    evidence.validate()?;
    Ok(evidence)
}
