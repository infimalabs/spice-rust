//! The canonical compiled 25-arm gate battery.
//!
//! Family modules own acquisition and decisions. This module owns only their
//! ordered orchestration: one checked-in disposition manifest, one immutable
//! run context, one call per compiled arm, and one total typed result vector.

mod context;
mod execution;
mod family_evidence;
mod manifest_contract;

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::manifest::PREDECESSOR_GATE_RELEASE;

pub use context::{
    ActorEvidence, BatteryContext, BatteryInputs, BatteryScope, BatteryToolEvidence,
    BoundActorInputsEvidence, BoundEvidence, ConfigurationEvidence, LatchEvidence,
};
pub use execution::{
    ArmExecution, BatteryArmResult, BatteryExecutor, BatteryReport, CompiledArm, run_battery,
};
pub use family_evidence::{ArmEvidence, BatteryState, ResultProvenance};
#[cfg(test)]
use manifest_contract::declaration_identity;
pub use manifest_contract::{
    ArmDisposition, BatteryManifest, BatteryManifestArm, CANONICAL_BATTERY_MANIFEST,
    GATE_BATTERY_VERSION,
};

/// One declaration family behind the common runner boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArmFamily {
    Core,
    Toolchain,
    Metrics,
    Hygiene,
}

/// One release-owned refusal retained in place of executable behavior.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationRefusal {
    pub(super) source_field: Vec<String>,
    pub(super) owning_release: String,
    pub(super) reason: String,
    pub(super) recovery: String,
}

impl MigrationRefusal {
    /// Structurally segmented source field that could not be compiled.
    #[must_use]
    pub fn source_field(&self) -> &[String] {
        &self.source_field
    }

    /// Release that still owns interpretation of the field.
    #[must_use]
    pub fn owning_release(&self) -> &str {
        &self.owning_release
    }

    /// Exact semantic incompatibility.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Exact operator recovery.
    #[must_use]
    pub fn recovery(&self) -> &str {
        &self.recovery
    }

    pub(super) fn validate(&self, position: usize) -> Result<(), BatteryError> {
        if self.source_field.is_empty()
            || self.source_field.iter().any(|part| !valid_text(part))
            || self.owning_release != PREDECESSOR_GATE_RELEASE
            || !valid_text(&self.reason)
            || !valid_text(&self.recovery)
        {
            return Err(BatteryError::new(format!(
                "battery arm {position} has an incomplete migration refusal"
            )));
        }
        Ok(())
    }
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains(['\0', '\r', '\n'])
}

fn inline_evidence(value: &str) -> String {
    value
        .replace('\0', "\\0")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// A malformed manifest, input contract, execution, or report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatteryError {
    detail: String,
}

impl BatteryError {
    /// Build an explicit executor or integration refusal.
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// Exact retained refusal detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for BatteryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for BatteryError {}

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
