//! Receipt-free distribution payloads shared by dist and proof.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Digest;

/// A release tuple with a distinct artifact identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ReleaseTarget {
    #[serde(rename = "aarch64-apple-darwin")]
    Aarch64AppleDarwin,
    #[serde(rename = "x86_64-unknown-linux-musl")]
    X86_64UnknownLinuxMusl,
}

impl ReleaseTarget {
    /// The canonical Rust target-triple spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aarch64AppleDarwin => "aarch64-apple-darwin",
            Self::X86_64UnknownLinuxMusl => "x86_64-unknown-linux-musl",
        }
    }
}

impl fmt::Display for ReleaseTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Exact identities required before distribution may build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPins {
    pub rust: String,
    pub wasm_target: String,
    pub trunk: String,
    pub cargo_mutants: String,
    pub proof_container: String,
}

/// One staged file bound into a target payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryEntry {
    pub path: String,
    pub mode: u32,
    pub bytes: u64,
    pub digest: Digest,
}

/// The receipt-free object submitted to proof for one release target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPayload {
    pub version: u16,
    pub target: ReleaseTarget,
    pub source: Digest,
    pub app: Digest,
    pub embedded_app: Digest,
    pub native: Digest,
    pub pins: BuildPins,
    pub pins_digest: Digest,
    pub inventory: Vec<InventoryEntry>,
    pub inventory_digest: Digest,
}

/// Whether the complete sealed proof battery passed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProofStanding {
    /// Every declared battery member completed and its evidence was whole.
    Clean,
    /// At least one battery member failed, timed out, or produced incomplete evidence.
    Refused,
}

/// Detached evidence binding one sealed replay to one receipt-free target payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofReceipt {
    /// Current detached-receipt schema.
    pub version: u16,
    /// Release tuple the payload and replay both cover.
    pub target: ReleaseTarget,
    /// Canonical identity of the receipt-free target payload.
    pub payload: Digest,
    /// Canonical identity of the ordered battery command inventory.
    pub battery: Digest,
    /// Canonical identity of the complete typed replay result.
    pub replay: Digest,
    /// Digest-pinned container or native runner identity that executed the replay.
    pub environment: String,
    /// One standing for the complete battery.
    pub standing: ProofStanding,
}
