use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::family_evidence::metric_terminal;
use super::{ArmEvidence, BatteryArmResult, BatteryReport, BatteryState, FocusedArmReport};
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricKey, MetricMeasurement};

/// One stable work item projected from a non-admitting battery result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateFinding {
    identity: Digest,
    gate: String,
    authority: String,
    state: String,
    severity: String,
    locus: String,
    title: String,
    description: String,
    acceptance: Vec<String>,
}

impl GateFinding {
    /// Stable digest identity shared with board filing.
    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }

    /// Gate that produced the finding.
    #[must_use]
    pub fn gate(&self) -> &str {
        &self.gate
    }

    /// Canonical declaration whose authority produced the evidence.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Terminal state that made the finding non-admitting.
    #[must_use]
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Scheduling severity for filed work.
    #[must_use]
    pub fn severity(&self) -> &str {
        &self.severity
    }

    /// Evidence locus used for matching and display.
    #[must_use]
    pub fn locus(&self) -> &str {
        &self.locus
    }

    /// Board task title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Board task description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Board task acceptance criteria.
    #[must_use]
    pub fn acceptance(&self) -> &[String] {
        &self.acceptance
    }
}

impl BatteryReport {
    /// Project every non-admitting typed result into stable finding work.
    #[must_use]
    pub fn findings(&self) -> Vec<GateFinding> {
        self.results()
            .iter()
            .flat_map(project_result)
            .collect::<Vec<_>>()
    }
}

impl BatteryArmResult {
    /// Project every non-admitting typed result from this arm into stable work.
    #[must_use]
    pub fn findings(&self) -> Vec<GateFinding> {
        project_result(self)
    }
}

impl FocusedArmReport {
    /// Project the exact findings carried by this focused report.
    #[must_use]
    pub fn findings(&self) -> Vec<GateFinding> {
        self.result().findings()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindingIdentity {
    version: u64,
    gate: String,
    declaration: String,
    family: String,
    scope: FindingScope,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
enum FindingScope {
    Arm,
    Metric { metric: String, key: MetricKey },
}

fn project_result(result: &BatteryArmResult) -> Vec<GateFinding> {
    if result.state().admits() {
        return Vec::new();
    }
    let ArmEvidence::Metrics { report } = result.evidence() else {
        return vec![arm_finding(result)];
    };
    let findings = report
        .metrics()
        .iter()
        .filter(|metric| !metric_terminal(metric).admits())
        .map(|metric| metric_finding(result, metric))
        .collect::<Vec<_>>();
    if findings.is_empty() {
        vec![arm_finding(result)]
    } else {
        findings
    }
}

fn arm_finding(result: &BatteryArmResult) -> GateFinding {
    let state = result.state().as_str().to_owned();
    let gate = result.gate_id().to_string();
    let authority = result.declaration().to_owned();
    let locus = format!("gate={gate}");
    let title = format!("Gate finding: {gate} {state}");
    let description = format!(
        concat!(
            "Gate finding identity: {identity}\n",
            "Gate: {gate}\n",
            "Affected authority: {authority}\n",
            "State: {state}\n",
            "Locus: {locus}\n",
            "Reason: {reason}"
        ),
        identity = finding_identity(result, FindingScope::Arm),
        gate = gate.as_str(),
        authority = authority.as_str(),
        state = state.as_str(),
        locus = locus.as_str(),
        reason = result.reason()
    );
    let identity = finding_identity(result, FindingScope::Arm);
    GateFinding {
        acceptance: acceptance(&gate, &state, &locus),
        severity: severity(result.state()).to_owned(),
        identity,
        gate,
        authority,
        state,
        locus,
        title,
        description,
    }
}

fn metric_finding(result: &BatteryArmResult, metric: &MetricEvidence) -> GateFinding {
    let terminal = metric_terminal(metric);
    let state = terminal.as_str().to_owned();
    let gate = result.gate_id().to_string();
    let authority = result.declaration().to_owned();
    let metric_name = metric.metric().as_str().to_owned();
    let key = metric_key(metric.key());
    let locus = format!("gate={gate} metric={metric_name} key={key}");
    let identity = finding_identity(
        result,
        FindingScope::Metric {
            metric: metric_name.clone(),
            key: metric.key().clone(),
        },
    );
    let title = format!("Gate finding: {gate} {metric_name}");
    let description = format!(
        concat!(
            "Gate finding identity: {identity}\n",
            "Gate: {gate}\n",
            "Affected authority: {authority}\n",
            "Metric: {metric_name}\n",
            "State: {state}\n",
            "Locus: {locus}\n",
            "Reason: {reason}{details}"
        ),
        identity = identity,
        gate = gate.as_str(),
        authority = authority.as_str(),
        metric_name = metric_name.as_str(),
        state = state.as_str(),
        locus = locus.as_str(),
        reason = result.reason(),
        details = metric_details(metric)
    );
    GateFinding {
        acceptance: acceptance(&gate, &state, &locus),
        severity: severity(terminal).to_owned(),
        identity,
        gate,
        authority,
        state,
        locus,
        title,
        description,
    }
}

fn finding_identity(result: &BatteryArmResult, scope: FindingScope) -> Digest {
    let identity = FindingIdentity {
        version: 1,
        gate: result.gate_id().to_string(),
        declaration: result.declaration().to_owned(),
        family: format!("{:?}", result.family()),
        scope,
    };
    let bytes = wire::encode(&identity).expect("gate finding identity is canonical");
    Digest::compute(Domain::GateFinding, &bytes)
}

fn metric_key(key: &MetricKey) -> String {
    match key {
        MetricKey::Arm => "arm".to_owned(),
        MetricKey::Path(path) => format!("path={}", quoted(path)),
        MetricKey::Function(function) => format!(
            "path={} function={}",
            quoted(function.path()),
            quoted(function.function())
        ),
    }
}

fn metric_details(metric: &MetricEvidence) -> String {
    match metric.state() {
        MetricState::Measured { measurement } => measured_details(measurement),
        MetricState::Unavailable { reason }
        | MetricState::Unmeasured { reason }
        | MetricState::NotApplicable { reason } => {
            format!("\nMetric reason: {reason}")
        }
        MetricState::Unsupported {
            source_field,
            owning_release,
            reason,
            recovery,
        } => format!(
            "\nSource field: {source_field}\nOwning release: {owning_release}\nMetric reason: {reason}\nRecovery: {recovery}"
        ),
        MetricState::Timeout {
            milliseconds,
            reason,
        } => format!("\nMilliseconds: {milliseconds}\nMetric reason: {reason}"),
        MetricState::Failed { reason, exit_code } => format!(
            "\nExit code: {}\nMetric reason: {reason}",
            exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string())
        ),
    }
}

fn measured_details(measurement: &MetricMeasurement) -> String {
    let mut details = format!("\nValue: {}", measurement.value());
    if let Some(threshold) = measurement.threshold() {
        details.push_str(&format!("\nThreshold: {threshold}"));
    }
    if let Some(latch) = measurement.latch() {
        details.push_str(&format!(
            concat!(
                "\nBase limit: {}",
                "\nConfigured ratchet limit: {}",
                "\nRatchet limit: {}",
                "\nActive limit: {}",
                "\nLatch transition: {}"
            ),
            latch.base_limit(),
            latch.configured_ratchet_limit(),
            latch.ratchet_limit(),
            latch.active_limit(),
            latch.transition()
        ));
    }
    for site in measurement.findings() {
        details.push_str(&format!(
            "\nSite: {}:{} target {}",
            site.path,
            site.line,
            quoted(&site.target)
        ));
        if let Some(expected) = site.expected.as_deref() {
            details.push_str(&format!(" expected {}", quoted(expected)));
        }
    }
    details
}

fn severity(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Failed | BatteryState::TimedOut => "high",
        BatteryState::Unavailable | BatteryState::Unsupported | BatteryState::Unmeasured => {
            "medium"
        }
        BatteryState::ExplicitSkip => "low",
        BatteryState::Passed | BatteryState::NotApplicable | BatteryState::Excluded => "low",
    }
}

fn acceptance(gate: &str, state: &str, locus: &str) -> Vec<String> {
    vec![format!(
        "The {gate} gate no longer reports {state} for {locus}."
    )]
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("gate finding text is valid JSON string content")
}
