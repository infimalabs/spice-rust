//! Canonical identity spellings shared by every crossing type.
//!
//! WIRE fixes one spelling per durable identity. UUIDs are lower-case
//! `8-4-4-4-12`; instants are RFC3339 UTC with millisecond precision and a
//! trailing `Z`. The `uuid` and `jiff` crates own the underlying primitives,
//! so no leaf chooses its own; these newtypes add only the spelling law,
//! which those crates deliberately do not enforce.
//!
//! # Instants narrow at construction, they do not refuse
//!
//! An [`Instant`] holds exactly the resolution its spelling carries, because
//! [`Instant::new`] floors to milliseconds instead of storing the sharper
//! timestamp it was handed. Equal wire bytes therefore imply equal values, and
//! `Ord` and `Hash` agree with what `Display` renders.
//!
//! The alternative was refusing sharper input. It was rejected: `new` is how a
//! clock reading becomes wire-shaped, and every clock reading is sharper than a
//! millisecond, so refusal would make minting an instant fallible and push the
//! identical narrowing into every call site — the same repair written N times,
//! each free to floor, truncate, or round differently. One narrowing in one
//! place is the deterministic path. Refusal still governs the other direction:
//! [`FromStr`] rejects a sharper *spelling* outright rather than repairing it,
//! because there the extra digits are a violated assumption about the wire
//! rather than a deliberate narrowing of an internal value.
//!
//! The narrowing floors toward negative infinity, matching the printer.
//! Truncating toward zero — which is what `Timestamp::as_millisecond` and
//! `RoundMode::Trunc` both do — disagrees with the rendered spelling before
//! 1970: `1969-12-31T23:59:59.999999Z` renders `.999Z` but truncates up to
//! `1970-01-01T00:00:00Z`, reintroducing this very defect in a narrower range.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// A spelling that WIRE refuses.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentError {
    /// The text is not the canonical lower-case `8-4-4-4-12` UUID spelling.
    #[error(
        "uuid must be canonical lower-case 8-4-4-4-12; \
         braces, upper case, compact text, and urn forms are refused: {0:?}"
    )]
    Uuid(String),
    /// The text is not RFC3339 UTC with millisecond precision and a `Z` suffix.
    #[error("instant must be RFC3339 UTC with millisecond precision and a trailing Z: {0:?}")]
    Instant(String),
}

/// A UUID with exactly one admitted spelling.
///
/// [`uuid::Uuid::parse_str`] accepts braces, upper case, compact text, and
/// urn prefixes. WIRE admits none of those, so parsing validates the literal
/// shape before delegating the hex decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalUuid(uuid::Uuid);

impl CanonicalUuid {
    /// Byte length of the one admitted spelling.
    const SPELLING_LEN: usize = 36;
    /// Indices carrying the four separators.
    const HYPHENS: [usize; 4] = [8, 13, 18, 23];

    /// Wrap an already-decoded UUID; rendering always uses the canonical form.
    #[must_use]
    pub const fn new(inner: uuid::Uuid) -> Self {
        Self(inner)
    }

    /// The underlying identity.
    #[must_use]
    pub const fn get(self) -> uuid::Uuid {
        self.0
    }

    /// True when `text` is the canonical lower-case `8-4-4-4-12` spelling.
    ///
    /// The check is deliberately literal: length, separator positions, and
    /// lower-case hex everywhere else. Anything a looser parser would repair
    /// is refused here instead.
    fn is_canonical(text: &str) -> bool {
        if text.len() != Self::SPELLING_LEN {
            return false;
        }
        text.bytes().enumerate().all(|(index, byte)| {
            if Self::HYPHENS.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
            }
        })
    }
}

impl FromStr for CanonicalUuid {
    type Err = IdentError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !Self::is_canonical(text) {
            return Err(IdentError::Uuid(text.to_owned()));
        }
        uuid::Uuid::parse_str(text)
            .map(Self)
            .map_err(|_| IdentError::Uuid(text.to_owned()))
    }
}

impl fmt::Display for CanonicalUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.as_hyphenated())
    }
}

impl Serialize for CanonicalUuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CanonicalUuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// An instant with exactly one admitted spelling.
///
/// Deadlines persist as these instants; the waits they arm run on monotonic
/// clocks elsewhere. Identifiers never double as instants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(jiff::Timestamp);

impl Instant {
    /// Fractional digits the one admitted spelling carries.
    const MILLIS: u8 = 3;
    /// Byte length of the one admitted spelling, `MILLIS` digits included.
    const SPELLING_LEN: usize = 24;
    /// Every fixed byte the spelling carries, by index.
    const SEPARATORS: [(usize, u8); 7] = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ];

    /// Wrap a timestamp, flooring it to the millisecond the spelling carries.
    ///
    /// A timestamp built elsewhere in the workspace may carry more resolution
    /// than the wire spells; the surplus is dropped here rather than at render,
    /// so the stored value is the one the spelling denotes. Flooring is total
    /// over the whole `Timestamp` range — `MIN` is already whole-millisecond
    /// and `MAX` floors downward inside it — which `instant_narrowing_is_total_
    /// over_the_timestamp_range` pins.
    #[must_use]
    pub fn new(inner: jiff::Timestamp) -> Self {
        let floored = inner
            .round(
                jiff::TimestampRound::new()
                    .smallest(jiff::Unit::Millisecond)
                    .mode(jiff::RoundMode::Floor),
            )
            .expect("flooring to milliseconds stays inside the timestamp range");
        Self(floored)
    }

    /// The underlying timestamp.
    #[must_use]
    pub const fn get(self) -> jiff::Timestamp {
        self.0
    }

    /// True when `text` is the canonical `YYYY-MM-DDTHH:MM:SS.mmmZ` spelling.
    ///
    /// The check is deliberately literal, like the UUID one: fixed bytes at
    /// fixed indices and digits everywhere else. `jiff` accepts offsets, a
    /// lower-case `z`, a space in place of the `T`, and any fractional
    /// precision; WIRE admits none of those, so the shape is settled before
    /// the parse rather than repaired by it.
    fn is_canonical(text: &str) -> bool {
        if text.len() != Self::SPELLING_LEN {
            return false;
        }
        text.bytes().enumerate().all(|(index, byte)| {
            match Self::SEPARATORS.iter().find(|(at, _)| *at == index) {
                Some((_, fixed)) => byte == *fixed,
                None => byte.is_ascii_digit(),
            }
        })
    }
}

impl FromStr for Instant {
    type Err = IdentError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !Self::is_canonical(text) {
            return Err(IdentError::Instant(text.to_owned()));
        }
        text.parse::<jiff::Timestamp>()
            .map(Self::new)
            .map_err(|_| IdentError::Instant(text.to_owned()))
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let printer = jiff::fmt::temporal::DateTimePrinter::new().precision(Some(Self::MILLIS));
        let mut rendered = String::new();
        printer
            .print_timestamp(&self.0, &mut rendered)
            .map_err(|_| fmt::Error)?;
        formatter.write_str(&rendered)
    }
}

impl Serialize for Instant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Instant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}
