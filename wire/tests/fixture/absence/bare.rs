//! Optional fields that omit the spelling of absence, at file top level and
//! inside a module.
//!
//! Not a compiled target. `wire/tests/absence.rs` walks this tree and asserts
//! the exact findings, so gutting the walk turns the suite red.

use serde::{Deserialize, Serialize};

/// Carries an optional field with no serde attributes at all.
#[derive(Serialize, Deserialize)]
pub struct Draft {
    pub name: String,
    pub note: Option<String>,
}

/// Carries an optional field spelled the one admitted way.
#[derive(Serialize, Deserialize)]
pub struct Settled {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub note: Option<String>,
}

pub mod inner {
    use serde::{Deserialize, Serialize};

    /// A struct variant reached only by descending into this module.
    #[derive(Serialize, Deserialize)]
    pub enum Step {
        Begun { after: Option<u32> },
    }
}
