//! The command envelope: one mutating request crossing the socket.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::envelope::WireVersion;
use crate::ident::CanonicalUuid;
use crate::witness::Witness;

/// A command discriminant, spelled as a path of single lower-case words.
///
/// The closed set of commands and the witness law each one carries live in
/// the generated command registry. This type enforces only the spelling, so
/// no leaf invents a discriminant shape while that registry is authored.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandKind(String);

/// Text that does not spell a command discriminant.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("command kind must be lower-case single words joined by `::`: {0:?}")]
pub struct CommandKindError(String);

impl CommandKind {
    /// Separator joining the words of a kind path.
    const SEPARATOR: &'static str = "::";

    /// The kind as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommandKind {
    type Err = CommandKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut words = text.split(Self::SEPARATOR);
        let shaped = words.clone().count() >= 2
            && words.all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()));
        if shaped {
            Ok(Self(text.to_owned()))
        } else {
            Err(CommandKindError(text.to_owned()))
        }
    }
}

impl fmt::Display for CommandKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for CommandKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CommandKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A mutating request in one envelope.
///
/// The body contains no caller-authored actor, lane, epoch, journal position,
/// result, or fact envelope; the commit door assigns all of those.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command<B> {
    /// Current wire version.
    pub v: WireVersion,
    /// Client-minted canonical identity for this request.
    pub id: CanonicalUuid,
    /// Closed command discriminant.
    pub kind: CommandKind,
    /// Named predicates over only the subjects this command relies on.
    pub expected: Vec<Witness>,
    /// Kind-specific intent.
    pub body: B,
}

impl<B: Serialize> Command<B> {
    /// Render the canonical encoding this command's identity binds.
    ///
    /// Witness order carries no meaning, so witnesses are sorted; object keys
    /// are emitted in sorted order by the encoder. The same request therefore
    /// yields the same bytes regardless of how a client ordered its input.
    ///
    /// # Errors
    ///
    /// Returns the encoder error when the body cannot be represented as JSON.
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        let mut sorted = serde_json::to_value(self)?;
        if let Some(expected) = sorted.get_mut("expected").and_then(|e| e.as_array_mut()) {
            expected.sort_by_key(|witness| witness.to_string());
        }
        serde_json::to_string(&sorted)
    }
}
