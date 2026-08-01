//! Optional fields one directory deeper, spelled partly and not at all.
//!
//! Not a compiled target. `wire/tests/absence.rs` walks this tree and asserts
//! the exact findings, so a walk that stops at the top directory turns the
//! suite red.

use serde::{Deserialize, Serialize};

/// Serializes absence correctly and decodes it two ways.
#[derive(Serialize, Deserialize)]
pub struct Partial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<u64>,
}

/// Never crosses, so its optional field answers to nothing.
pub struct Untyped {
    pub note: Option<String>,
}
