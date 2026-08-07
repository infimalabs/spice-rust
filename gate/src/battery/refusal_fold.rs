//! Bounded, non-authoritative diagnostics derived from a canonical battery.
//!
//! The canonical [`BatteryReport`] remains the complete machine authority. This
//! module projects only refusal facts for a human at a terminal: every refusing
//! arm, the exact selected-arm command that recovers its complete evidence, and
//! only those structural rows that contributed to refusal.

use std::fmt;

use super::family_evidence::metric_terminal;
use super::{ArmEvidence, BatteryArmResult, BatteryReport};
use crate::metrics::{EvidenceState as MetricState, MetricEvidence, MetricKey};

/// Hard byte limit for one human-facing refusal fold.
pub const BATTERY_REFUSAL_FOLD_MAX_BYTES: usize = 65_536;

/// Space retained whenever truncation is necessary for exact omission counts.
const TRUNCATION_FOOTER_RESERVE: usize = 256;

/// One bounded, non-authoritative rendering of a canonical report refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatteryRefusalFold {
    text: String,
    omitted_arms: usize,
    omitted_findings: usize,
}

impl BatteryRefusalFold {
    /// UTF-8 fold text, including its trailing newline.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Fold bytes suitable for a diagnostic stream.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    /// Number of arm reasons replaced by bounded fallback summaries.
    #[must_use]
    pub const fn omitted_arms(&self) -> usize {
        self.omitted_arms
    }

    /// Number of concrete metric findings omitted by the byte limit.
    #[must_use]
    pub const fn omitted_findings(&self) -> usize {
        self.omitted_findings
    }

    /// Consume the fold as diagnostic bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.text.into_bytes()
    }
}

impl fmt::Display for BatteryRefusalFold {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl BatteryReport {
    /// Derive a bounded human diagnostic without changing canonical report bytes.
    #[must_use]
    pub fn refusal_fold(&self) -> BatteryRefusalFold {
        render(self)
    }
}

struct ArmFold {
    full: String,
    fallback: String,
}

fn render(report: &BatteryReport) -> BatteryRefusalFold {
    let refused: Vec<_> = report
        .results()
        .iter()
        .filter(|result| !result.state().admits())
        .collect();
    let header = format!(
        "non-authoritative battery refusal fold: {} non-admitting arms\n",
        refused.len()
    );
    let arms: Vec<_> = refused.iter().map(|result| arm_fold(result)).collect();
    let findings: Vec<_> = refused
        .iter()
        .flat_map(|result| metric_findings(result))
        .collect();

    let arm_size = arms
        .iter()
        .fold(0usize, |size, arm| size.saturating_add(arm.full.len()));
    let finding_size = findings
        .iter()
        .fold(0usize, |size, finding| size.saturating_add(finding.len()));
    let complete_size = header
        .len()
        .saturating_add(arm_size)
        .saturating_add(finding_size);
    if complete_size <= BATTERY_REFUSAL_FOLD_MAX_BYTES {
        let mut text = String::with_capacity(complete_size);
        text.push_str(&header);
        for arm in arms {
            text.push_str(&arm.full);
        }
        for finding in findings {
            text.push_str(&finding);
        }
        return BatteryRefusalFold {
            text,
            omitted_arms: 0,
            omitted_findings: 0,
        };
    }

    let fallback_size = arms
        .iter()
        .fold(0usize, |size, arm| size.saturating_add(arm.fallback.len()));
    let fixed_size = header
        .len()
        .saturating_add(fallback_size)
        .saturating_add(TRUNCATION_FOOTER_RESERVE);
    assert!(
        fixed_size <= BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "compiled battery arm summaries exceed the refusal fold limit"
    );
    let mut budget = BATTERY_REFUSAL_FOLD_MAX_BYTES - fixed_size;
    let mut chosen_arms = Vec::with_capacity(arms.len());
    let mut omitted_arms = 0;
    for arm in arms {
        let extra = arm.full.len().saturating_sub(arm.fallback.len());
        if arm.full.len() <= arm.fallback.len() || extra <= budget {
            budget = budget.saturating_sub(extra);
            chosen_arms.push(arm.full);
        } else {
            omitted_arms += 1;
            chosen_arms.push(arm.fallback);
        }
    }

    let mut chosen_findings = Vec::new();
    let mut omitted_findings = 0;
    for finding in findings {
        if finding.len() <= budget {
            budget -= finding.len();
            chosen_findings.push(finding);
        } else {
            omitted_findings += 1;
        }
    }
    let footer = format!(
        "fold truncated at {BATTERY_REFUSAL_FOLD_MAX_BYTES} bytes: \
         omitted_arms={omitted_arms} omitted_findings={omitted_findings}\n"
    );
    assert!(
        footer.len() <= TRUNCATION_FOOTER_RESERVE,
        "refusal fold truncation footer exceeds its fixed reserve"
    );

    let mut text = String::with_capacity(BATTERY_REFUSAL_FOLD_MAX_BYTES);
    text.push_str(&header);
    for arm in chosen_arms {
        text.push_str(&arm);
    }
    for finding in chosen_findings {
        text.push_str(&finding);
    }
    text.push_str(&footer);
    assert!(
        text.len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "refusal fold bypassed its byte limit"
    );
    BatteryRefusalFold {
        text,
        omitted_arms,
        omitted_findings,
    }
}

fn arm_fold(result: &BatteryArmResult) -> ArmFold {
    let identity = format!(
        "arm position={} gate={} state={}\n",
        result.position(),
        result.gate_id(),
        result.state().as_str()
    );
    let command = format!("  full-evidence: cargo xtask gate {}\n", result.gate_id());
    ArmFold {
        full: format!("{identity}  reason={}\n{command}", quoted(result.reason())),
        fallback: format!("{identity}  reason=<omitted from bounded fold>\n{command}"),
    }
}

fn metric_findings(result: &BatteryArmResult) -> Vec<String> {
    let ArmEvidence::Metrics { report } = result.evidence() else {
        return Vec::new();
    };
    report
        .metrics()
        .iter()
        .filter_map(|metric| metric_finding(result, metric))
        .collect()
}

fn metric_finding(result: &BatteryArmResult, metric: &MetricEvidence) -> Option<String> {
    let terminal = metric_terminal(metric);
    if terminal.admits() {
        return None;
    }
    let mut finding = format!(
        "finding gate={} metric={} key={} state={} unit={}",
        result.gate_id(),
        metric.metric().as_str(),
        metric_key(metric.key()),
        terminal.as_str(),
        metric.unit().as_str()
    );
    match metric.state() {
        MetricState::Measured { measurement } => {
            finding.push_str(&format!(" value={}", measurement.value()));
            if let Some(threshold) = measurement.threshold() {
                finding.push_str(&format!(" threshold={threshold}"));
            }
            if let Some(latch) = measurement.latch() {
                finding.push_str(&format!(
                    " base_limit={} configured_ratchet_limit={} ratchet_limit={} \
                     active_limit={} latch_transition={}",
                    latch.base_limit(),
                    latch.configured_ratchet_limit(),
                    latch.ratchet_limit(),
                    latch.active_limit(),
                    latch.transition()
                ));
            }
        }
        MetricState::Unavailable { reason }
        | MetricState::Unmeasured { reason }
        | MetricState::NotApplicable { reason } => {
            finding.push_str(&format!(" reason={}", quoted(reason)));
        }
        MetricState::Unsupported {
            source_field,
            owning_release,
            reason,
            recovery,
        } => {
            finding.push_str(&format!(
                " source_field={} owning_release={} reason={} recovery={}",
                quoted(source_field),
                quoted(owning_release),
                quoted(reason),
                quoted(recovery)
            ));
        }
        MetricState::Timeout {
            milliseconds,
            reason,
        } => {
            finding.push_str(&format!(
                " milliseconds={milliseconds} reason={}",
                quoted(reason)
            ));
        }
        MetricState::Failed { reason, exit_code } => {
            finding.push_str(&format!(
                " exit_code={} reason={}",
                exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
                quoted(reason)
            ));
        }
    }
    finding.push('\n');
    Some(finding)
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

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("validated battery text always serializes as JSON text")
}
