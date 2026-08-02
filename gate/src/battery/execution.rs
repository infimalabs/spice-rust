use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::manifest_contract::{
    ArmDisposition, BatteryManifest, BatteryManifestArm, CANONICAL_BATTERY_MANIFEST,
    GATE_BATTERY_VERSION,
};
use super::{
    ArmFamily, BatteryContext, BatteryError, BatteryInputs, MigrationRefusal, inline_evidence,
    valid_text,
};
use crate::manifest::GateId;

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

    pub(super) const fn requires_provenance(self) -> bool {
        matches!(self, Self::NotApplicable | Self::Excluded)
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

/// One family-owned named decision retained without flattening it to text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvidence {
    name: String,
    decision: bool,
    source: String,
}

impl DecisionEvidence {
    /// Retain one named boolean decision and its owning component.
    pub fn new(
        name: impl Into<String>,
        decision: bool,
        source: impl Into<String>,
    ) -> Result<Self, BatteryError> {
        let evidence = Self {
            name: name.into(),
            decision,
            source: source.into(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Stable decision name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Family-owned boolean result.
    #[must_use]
    pub const fn decision(&self) -> bool {
        self.decision
    }

    /// Component that made this decision.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if !valid_text(&self.name) || !valid_text(&self.source) {
            return Err(BatteryError::new(
                "decision name and source must be non-empty single-line text",
            ));
        }
        Ok(())
    }
}

/// One family-owned measured quantity retained with unit and source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementEvidence {
    name: String,
    value: u64,
    unit: String,
    source: String,
}

impl MeasurementEvidence {
    /// Retain one measured value.
    pub fn new(
        name: impl Into<String>,
        value: u64,
        unit: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<Self, BatteryError> {
        let evidence = Self {
            name: name.into(),
            value,
            unit: unit.into(),
            source: source.into(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Stable measured quantity.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact unsigned measured value.
    #[must_use]
    pub const fn value(&self) -> u64 {
        self.value
    }

    /// Canonical measurement unit.
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// Parser or component that produced the value.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if !valid_text(&self.name) || !valid_text(&self.unit) || !valid_text(&self.source) {
            return Err(BatteryError::new(
                "measurement name, unit, and source must be non-empty single-line text",
            ));
        }
        Ok(())
    }
}

/// Family-qualified typed evidence returned by one executor call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "family")]
pub enum ArmEvidence {
    Core {
        decisions: Vec<DecisionEvidence>,
        measurements: Vec<MeasurementEvidence>,
    },
    Toolchain {
        decisions: Vec<DecisionEvidence>,
        measurements: Vec<MeasurementEvidence>,
    },
    Metrics {
        decisions: Vec<DecisionEvidence>,
        measurements: Vec<MeasurementEvidence>,
    },
    Hygiene {
        decisions: Vec<DecisionEvidence>,
        measurements: Vec<MeasurementEvidence>,
    },
    MigrationRefusal {
        refusal: MigrationRefusal,
    },
}

impl ArmEvidence {
    /// Family represented by compiled evidence, absent for a migration refusal.
    #[must_use]
    pub const fn family(&self) -> Option<ArmFamily> {
        match self {
            Self::Core { .. } => Some(ArmFamily::Core),
            Self::Toolchain { .. } => Some(ArmFamily::Toolchain),
            Self::Metrics { .. } => Some(ArmFamily::Metrics),
            Self::Hygiene { .. } => Some(ArmFamily::Hygiene),
            Self::MigrationRefusal { .. } => None,
        }
    }

    /// Decisions in family order, empty for a migration refusal.
    #[must_use]
    pub fn decisions(&self) -> &[DecisionEvidence] {
        match self {
            Self::Core { decisions, .. }
            | Self::Toolchain { decisions, .. }
            | Self::Metrics { decisions, .. }
            | Self::Hygiene { decisions, .. } => decisions,
            Self::MigrationRefusal { .. } => &[],
        }
    }

    /// Measurements in family order, empty for a migration refusal.
    #[must_use]
    pub fn measurements(&self) -> &[MeasurementEvidence] {
        match self {
            Self::Core { measurements, .. }
            | Self::Toolchain { measurements, .. }
            | Self::Metrics { measurements, .. }
            | Self::Hygiene { measurements, .. } => measurements,
            Self::MigrationRefusal { .. } => &[],
        }
    }

    fn validate(&self) -> Result<(), BatteryError> {
        match self {
            Self::Core {
                decisions,
                measurements,
            }
            | Self::Toolchain {
                decisions,
                measurements,
            }
            | Self::Metrics {
                decisions,
                measurements,
            }
            | Self::Hygiene {
                decisions,
                measurements,
            } => {
                for decision in decisions {
                    decision.validate()?;
                }
                for measurement in measurements {
                    measurement.validate()?;
                }
            }
            Self::MigrationRefusal { refusal } => refusal.validate(0)?,
        }
        Ok(())
    }

    fn empty(family: ArmFamily, source: &str) -> Self {
        let decision = DecisionEvidence {
            name: "executor-contract".to_owned(),
            decision: false,
            source: source.to_owned(),
        };
        match family {
            ArmFamily::Core => Self::Core {
                decisions: vec![decision],
                measurements: vec![],
            },
            ArmFamily::Toolchain => Self::Toolchain {
                decisions: vec![decision],
                measurements: vec![],
            },
            ArmFamily::Metrics => Self::Metrics {
                decisions: vec![decision],
                measurements: vec![],
            },
            ArmFamily::Hygiene => Self::Hygiene {
                decisions: vec![decision],
                measurements: vec![],
            },
        }
    }
}

/// Complete executor output for one compiled arm.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArmExecution {
    state: BatteryState,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provenance: Option<ResultProvenance>,
    evidence: ArmEvidence,
}

impl ArmExecution {
    /// Validate one explicit terminal state and its retained family evidence.
    pub fn new(
        state: BatteryState,
        reason: impl Into<String>,
        provenance: Option<ResultProvenance>,
        evidence: ArmEvidence,
    ) -> Result<Self, BatteryError> {
        let execution = Self {
            state,
            reason: reason.into(),
            provenance,
            evidence,
        };
        execution.validate()?;
        Ok(execution)
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if !valid_text(&self.reason) {
            return Err(BatteryError::new(
                "arm result reason must be non-empty single-line text",
            ));
        }
        if self.state.requires_provenance() && self.provenance.is_none() {
            return Err(BatteryError::new(format!(
                "{:?} requires exact reason provenance",
                self.state
            )));
        }
        if let Some(provenance) = &self.provenance {
            provenance.validate()?;
        }
        self.evidence.validate()?;
        Ok(())
    }
}

/// One compiled row passed to the executor in canonical order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledArm {
    position: usize,
    predecessor: String,
    gate_id: GateId,
    family: ArmFamily,
    declaration: String,
}

impl CompiledArm {
    /// Zero-based canonical report position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Exact predecessor standard-arm name.
    #[must_use]
    pub fn predecessor(&self) -> &str {
        &self.predecessor
    }

    /// Stable compiled identity.
    #[must_use]
    pub const fn gate_id(&self) -> GateId {
        self.gate_id
    }

    /// Owning family.
    #[must_use]
    pub const fn family(&self) -> ArmFamily {
        self.family
    }

    /// Exact compiled declaration identity.
    #[must_use]
    pub fn declaration(&self) -> &str {
        &self.declaration
    }
}

/// Rust execution boundary used exactly once for every compiled row.
pub trait BatteryExecutor {
    /// Execute one arm from the already captured immutable authorities.
    fn execute(
        &mut self,
        arm: &CompiledArm,
        inputs: &BatteryInputs<'_>,
    ) -> Result<ArmExecution, BatteryError>;
}

/// One visible result at one canonical report position.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryArmResult {
    position: usize,
    predecessor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gate_id: Option<GateId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    declaration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    family: Option<ArmFamily>,
    state: BatteryState,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provenance: Option<ResultProvenance>,
    evidence: ArmEvidence,
}

impl BatteryArmResult {
    /// Zero-based canonical position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Exact predecessor standard-arm name.
    #[must_use]
    pub fn predecessor(&self) -> &str {
        &self.predecessor
    }

    /// Compiled gate identity, absent only for a migration refusal.
    #[must_use]
    pub const fn gate_id(&self) -> Option<GateId> {
        self.gate_id
    }

    /// Exact compiled declaration identity.
    #[must_use]
    pub fn declaration(&self) -> Option<&str> {
        self.declaration.as_deref()
    }

    /// Owning compiled family, absent only for a migration refusal.
    #[must_use]
    pub const fn family(&self) -> Option<ArmFamily> {
        self.family
    }

    /// Typed terminal state.
    #[must_use]
    pub const fn state(&self) -> BatteryState {
        self.state
    }

    /// Exact retained reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Required applicability/exclusion provenance when relevant.
    #[must_use]
    pub const fn provenance(&self) -> Option<&ResultProvenance> {
        self.provenance.as_ref()
    }

    /// Family-qualified decisions and measurements.
    #[must_use]
    pub const fn evidence(&self) -> &ArmEvidence {
        &self.evidence
    }
}

/// Complete canonical battery report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryReport {
    version: u64,
    gate_set: Digest,
    context: BatteryContext,
    results: Vec<BatteryArmResult>,
}

impl BatteryReport {
    /// Current report version.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Manifest-and-declaration identity computed before execution.
    #[must_use]
    pub const fn gate_set(&self) -> &Digest {
        &self.gate_set
    }

    /// Complete immutable run inputs retained once for every result.
    #[must_use]
    pub const fn context(&self) -> &BatteryContext {
        &self.context
    }

    /// Exactly 25 visible entries in canonical order.
    #[must_use]
    pub fn results(&self) -> &[BatteryArmResult] {
        &self.results
    }

    /// Whether every result passed or has contract-owned non-execution.
    #[must_use]
    pub fn admits(&self) -> bool {
        self.results.iter().all(|result| result.state.admits())
    }

    /// Canonical JSON bytes for the complete report.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only a canonical, complete, correctly ordered current report.
    pub fn decode(bytes: &[u8]) -> Result<Self, BatteryError> {
        let report: Self = wire::decode(bytes)
            .map_err(|error| BatteryError::new(format!("invalid battery report: {error}")))?;
        report.validate()?;
        let canonical = report
            .encode()
            .map_err(|error| BatteryError::new(format!("cannot encode battery report: {error}")))?;
        if canonical != bytes {
            return Err(BatteryError::new("battery report bytes are not canonical"));
        }
        Ok(report)
    }

    fn validate(&self) -> Result<(), BatteryError> {
        if self.version != GATE_BATTERY_VERSION {
            return Err(BatteryError::new(format!(
                "unsupported battery report version {}; expected {GATE_BATTERY_VERSION}",
                self.version
            )));
        }
        self.context.validate()?;
        let manifest = BatteryManifest::canonical()?;
        if self.gate_set != gate_set_digest(&manifest) {
            return Err(BatteryError::new(
                "battery report gate-set identity disagrees with the canonical manifest",
            ));
        }
        if self.results.len() != manifest.arms.len() {
            return Err(BatteryError::new(format!(
                "complete battery report requires {} results, received {}",
                manifest.arms.len(),
                self.results.len()
            )));
        }
        for (position, (result, arm)) in self.results.iter().zip(&manifest.arms).enumerate() {
            validate_result(position, result, arm)?;
        }
        Ok(())
    }
}

/// Execute the one canonical battery and retain a total result vector.
pub fn run_battery(
    inputs: &BatteryInputs<'_>,
    executor: &mut impl BatteryExecutor,
) -> Result<BatteryReport, BatteryError> {
    let manifest = BatteryManifest::canonical()?;
    let gate_set = gate_set_digest(&manifest);
    let context = inputs.snapshot()?;
    let mut results = Vec::with_capacity(manifest.arms.len());
    for (position, arm) in manifest.arms.iter().enumerate() {
        results.push(execute_arm(position, arm, inputs, executor));
    }
    let report = BatteryReport {
        version: GATE_BATTERY_VERSION,
        gate_set,
        context,
        results,
    };
    report.validate()?;
    Ok(report)
}

fn execute_arm(
    position: usize,
    arm: &BatteryManifestArm,
    inputs: &BatteryInputs<'_>,
    executor: &mut impl BatteryExecutor,
) -> BatteryArmResult {
    match &arm.disposition {
        ArmDisposition::Compiled {
            gate_id,
            family,
            declaration,
        } => execute_compiled(
            position,
            arm,
            *gate_id,
            *family,
            declaration,
            inputs,
            executor,
        ),
        ArmDisposition::MigrationRefusal {
            source_field,
            owning_release,
            reason,
            recovery,
        } => {
            let refusal = MigrationRefusal {
                source_field: source_field.clone(),
                owning_release: owning_release.clone(),
                reason: reason.clone(),
                recovery: recovery.clone(),
            };
            BatteryArmResult {
                position,
                predecessor: arm.predecessor.clone(),
                gate_id: None,
                declaration: None,
                family: None,
                state: BatteryState::Unsupported,
                reason: refusal.reason.clone(),
                provenance: None,
                evidence: ArmEvidence::MigrationRefusal { refusal },
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_compiled(
    position: usize,
    arm: &BatteryManifestArm,
    gate_id: GateId,
    family: ArmFamily,
    declaration: &str,
    inputs: &BatteryInputs<'_>,
    executor: &mut impl BatteryExecutor,
) -> BatteryArmResult {
    let compiled = CompiledArm {
        position,
        predecessor: arm.predecessor.clone(),
        gate_id,
        family,
        declaration: declaration.to_owned(),
    };
    let execution = match executor.execute(&compiled, inputs) {
        Ok(execution) => admit_execution(execution, family),
        Err(error) => failed_execution(
            family,
            &format!("executor refused arm: {}", inline_evidence(error.detail())),
        ),
    };
    BatteryArmResult {
        position,
        predecessor: arm.predecessor.clone(),
        gate_id: Some(gate_id),
        declaration: Some(declaration.to_owned()),
        family: Some(family),
        state: execution.state,
        reason: execution.reason,
        provenance: execution.provenance,
        evidence: execution.evidence,
    }
}

fn admit_execution(execution: ArmExecution, family: ArmFamily) -> ArmExecution {
    match execution.validate() {
        Ok(()) if execution.evidence.family() == Some(family) => execution,
        Ok(()) => failed_execution(
            family,
            "executor returned evidence for a different declaration family",
        ),
        Err(error) => failed_execution(
            family,
            &format!(
                "executor returned invalid evidence: {}",
                inline_evidence(error.detail())
            ),
        ),
    }
}

fn failed_execution(family: ArmFamily, reason: &str) -> ArmExecution {
    ArmExecution {
        state: BatteryState::Failed,
        reason: reason.to_owned(),
        provenance: None,
        evidence: ArmEvidence::empty(family, "gate::battery::run_battery"),
    }
}

fn validate_result(
    position: usize,
    result: &BatteryArmResult,
    arm: &BatteryManifestArm,
) -> Result<(), BatteryError> {
    if result.position != position || result.predecessor != arm.predecessor {
        return Err(BatteryError::new(format!(
            "battery result {position} is missing or reordered"
        )));
    }
    ArmExecution {
        state: result.state,
        reason: result.reason.clone(),
        provenance: result.provenance.clone(),
        evidence: result.evidence.clone(),
    }
    .validate()?;
    match &arm.disposition {
        ArmDisposition::Compiled {
            gate_id,
            family,
            declaration,
        } => validate_compiled_result(position, result, *gate_id, *family, declaration),
        ArmDisposition::MigrationRefusal {
            source_field,
            owning_release,
            reason,
            recovery,
        } => validate_refusal_result(
            position,
            result,
            MigrationRefusal {
                source_field: source_field.clone(),
                owning_release: owning_release.clone(),
                reason: reason.clone(),
                recovery: recovery.clone(),
            },
        ),
    }
}

fn validate_compiled_result(
    position: usize,
    result: &BatteryArmResult,
    gate_id: GateId,
    family: ArmFamily,
    declaration: &str,
) -> Result<(), BatteryError> {
    if result.gate_id != Some(gate_id)
        || result.family != Some(family)
        || result.declaration.as_deref() != Some(declaration)
        || result.evidence.family() != Some(family)
    {
        return Err(BatteryError::new(format!(
            "battery result {position} disagrees with its compiled declaration"
        )));
    }
    Ok(())
}

fn validate_refusal_result(
    position: usize,
    result: &BatteryArmResult,
    refusal: MigrationRefusal,
) -> Result<(), BatteryError> {
    if result.gate_id.is_some()
        || result.family.is_some()
        || result.declaration.is_some()
        || result.state != BatteryState::Unsupported
        || result.reason != refusal.reason
        || result.provenance.is_some()
        || result.evidence != (ArmEvidence::MigrationRefusal { refusal })
    {
        return Err(BatteryError::new(format!(
            "battery result {position} does not preserve its migration refusal"
        )));
    }
    Ok(())
}

fn gate_set_digest(manifest: &BatteryManifest) -> Digest {
    let mut bytes = CANONICAL_BATTERY_MANIFEST.as_bytes().to_vec();
    bytes.extend_from_slice(b"\0compiled-declaration-identities\0");
    for (position, arm) in manifest.arms.iter().enumerate() {
        bytes.extend_from_slice(position.to_string().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(arm.predecessor.as_bytes());
        bytes.push(0);
        match &arm.disposition {
            ArmDisposition::Compiled { declaration, .. } => {
                bytes.extend_from_slice(declaration.as_bytes());
            }
            ArmDisposition::MigrationRefusal {
                source_field,
                owning_release,
                reason,
                recovery,
            } => {
                bytes.extend_from_slice(owning_release.as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(source_field.join(".").as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(reason.as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(recovery.as_bytes());
            }
        }
        bytes.push(b'\n');
    }
    Digest::compute(Domain::GateSet, &bytes)
}
