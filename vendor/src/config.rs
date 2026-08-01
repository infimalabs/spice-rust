//! Pure admission of tracked vendor executable selections.

use std::collections::BTreeMap;

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use wire::Name;

/// A configured executable spelling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binary(String);

impl Binary {
    /// Exact executable spelling supplied by tracked policy.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() {
            return Err(D::Error::custom("vendor binary must not be empty"));
        }
        if value.contains('\0') {
            return Err(D::Error::custom("vendor binary must not contain NUL"));
        }
        Ok(Self(value))
    }
}

impl Serialize for Binary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// One `[vendor.NAME]` override row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    binary: Binary,
}

impl Entry {
    /// Executable selected for this vendor.
    #[must_use]
    pub const fn binary(&self) -> &Binary {
        &self.binary
    }
}

/// Operator-authored vendor rows, keyed by one-word vendor identity.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Config(BTreeMap<Name, Entry>);

impl Config {
    /// Every configured vendor row in stable name order.
    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<Name, Entry> {
        &self.0
    }
}
