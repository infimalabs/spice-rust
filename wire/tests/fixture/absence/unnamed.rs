//! Optional fields declared without a name, in a tuple struct and in a tuple
//! variant.
//!
//! Not a compiled target. `wire/tests/absence.rs` walks this tree and asserts
//! the exact findings, so a judgment that reads the ident and skips the fields
//! that have none turns the suite red.

use serde::{Deserialize, Serialize};

/// Carries an optional field past the first position with no serde attributes,
/// so the finding has to name the position rather than a constant.
#[derive(Serialize, Deserialize)]
pub struct Tagged(pub String, pub Option<u32>);

/// Spells absence the one admitted way on a field that has no name to hang the
/// attributes on but carries them anyway.
#[derive(Serialize, Deserialize)]
pub struct Sealed(
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub Option<u32>,
);

/// Tuple variants, one bare and one spelled partly.
#[derive(Serialize, Deserialize)]
pub enum Reach {
    Near(Option<String>),
    Far(#[serde(skip_serializing_if = "Option::is_none")] Option<String>),
}
