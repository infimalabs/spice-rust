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
pub struct BuildPins {
    pub rust: String,
    pub wasm_target: String,
    pub trunk: String,
    pub cargo_mutants: String,
    pub proof_container: String,
}

/// One staged file bound into a target payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryEntry {
    pub path: String,
    pub mode: u32,
    pub bytes: u64,
    pub digest: Digest,
}

/// The receipt-free object submitted to proof for one release target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
