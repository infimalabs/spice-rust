//! The canonical compiled 26-arm gate battery.
//!
//! Family modules own acquisition and decisions. This module owns only their
//! ordered orchestration: one checked-in disposition manifest, one immutable
//! run context, one call per compiled arm, and one total typed result vector.

#[cfg(not(target_arch = "wasm32"))]
mod authorities;
mod context;
mod execution;
mod family_evidence;
mod manifest_contract;
mod receipt;
#[cfg(not(target_arch = "wasm32"))]
mod repository_executor;

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
pub use authorities::BatteryCapture;
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
pub use receipt::{
    BatteryReceipt, GATE_BATTERY_RECEIPT_VERSION, ReceiptRefusal, UnsupportedReceipt,
    supported_pairs,
};
#[cfg(not(target_arch = "wasm32"))]
pub use repository_executor::RepositoryExecutionEvent;

/// One declaration family behind the common runner boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArmFamily {
    Core,
    Toolchain,
    Metrics,
    Hygiene,
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
