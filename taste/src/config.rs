//! The tracked configuration document as the sole active-taste authority.
//!
//! `TASTES.md` owns the curation law and the rationale for each installed
//! seed; the tracked TOML owns which tastes are active. No taste name, trigger
//! word, or message appears in Rust, here or anywhere else in the crate,
//! because a named copy would be a second register that an operator commit
//! could not move.
//!
//! Acquisition stays outside. This decoder takes document text a caller
//! already holds and returns a [`Bag`]; nothing here opens a file, consults an
//! environment variable, or knows where the document lives. That is what lets
//! the same function decode the tracked root document, a documented seed
//! block, and a synthetic fixture without one of them being a special case.

use std::error::Error;
use std::fmt;

use serde::Deserialize;
use wire::{Name, NameError};

use crate::bag::{Bag, BagError, Taste};

/// The one root section this decoder reads.
///
/// Every other root section is ignored rather than refused. The tracked
/// document is shared configuration carrying sections this crate has no
/// opinion about, so refusing them would make taste the gatekeeper of a
/// vocabulary it does not own. The section is optional here only so that its
/// absence can be reported as [`ConfigError::Absent`] rather than read as an
/// empty set.
#[derive(Debug, Deserialize)]
struct Document {
    taste: Option<toml::Table>,
}

/// One `[taste.NAME]` row exactly as tracked.
///
/// Unknown keys are refused. A misspelled `word` would otherwise leave a taste
/// with no selectors under a name that still reads as active, which is the one
/// failure a curated set cannot afford to take quietly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    words: Vec<String>,
    message: String,
}

/// Build the active set from one complete tracked configuration document.
///
/// Rows arrive in tracked order and each one is converted through the same
/// [`Name`], [`crate::Trigger`], [`Taste`], and [`Bag`] validation every other
/// caller uses, so a tracked document cannot express a taste that a
/// hand-built one could not.
///
/// # Errors
///
/// Returns [`ConfigError`] when the document is not valid TOML, declares no
/// `[taste]` section or an empty one, or carries a row whose name, shape,
/// words, or message the taste vocabulary refuses. Every refusal names either
/// the parse position or the offending taste.
pub fn configured(document: &str) -> Result<Bag, ConfigError> {
    let parsed: Document = toml::from_str(document).map_err(ConfigError::Parse)?;
    let rows = parsed.taste.ok_or(ConfigError::Absent)?;
    if rows.is_empty() {
        return Err(ConfigError::Empty);
    }

    let mut tastes = Vec::with_capacity(rows.len());
    for (name, row) in rows {
        let identity = name.parse::<Name>().map_err(|error| ConfigError::Name {
            name: name.clone(),
            error,
        })?;
        let Row { words, message } = Row::deserialize(row).map_err(|error| ConfigError::Row {
            name: name.clone(),
            error,
        })?;
        let taste = Taste::new(identity, words, message)
            .map_err(|error| ConfigError::Taste { name, error })?;
        tastes.push(taste);
    }

    Bag::new(tastes).map_err(ConfigError::Set)
}

/// Why a tracked configuration document did not yield an active set.
#[derive(Debug)]
pub enum ConfigError {
    /// The document is not well-formed TOML.
    ///
    /// This also covers a repeated `[taste.NAME]` table and a repeated key
    /// inside one row: TOML refuses both outright, and its message names the
    /// key and position, which is more context than this decoder could add
    /// after the fact.
    Parse(toml::de::Error),
    /// The document declares no `[taste]` section.
    Absent,
    /// The `[taste]` section is present and declares nothing.
    ///
    /// Distinct from [`ConfigError::Absent`], because an emptied section is an
    /// edit that removed every taste while a missing one is a document that
    /// never carried the vocabulary at all.
    Empty,
    /// One `[taste.NAME]` row is not shaped like a taste.
    Row {
        /// Taste the offending row was tracked under.
        name: String,
        /// What the row decoder refused.
        error: toml::de::Error,
    },
    /// One `[taste.NAME]` table name is not a taste name.
    Name {
        /// Table name as tracked.
        name: String,
        /// What the name vocabulary refused.
        error: NameError,
    },
    /// One row's words or message were refused.
    Taste {
        /// Taste the offending row was tracked under.
        name: String,
        /// What the taste vocabulary refused.
        error: BagError,
    },
    /// The rows did not form an active set.
    ///
    /// TOML already refuses two tables under one name, so this reports a
    /// duplicate only if [`Name`] ever stops being a one-to-one reading of the
    /// tracked text. It is a guard on that assumption rather than the gate
    /// that enforces uniqueness today.
    Set(BagError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => {
                write!(formatter, "taste configuration is not valid TOML: {error}")
            }
            Self::Absent => formatter.write_str("taste configuration declares no [taste] section"),
            Self::Empty => {
                formatter.write_str("taste configuration declares an empty [taste] section")
            }
            Self::Row { name, error } => {
                write!(formatter, "[taste.{name}] is not a taste row: {error}")
            }
            Self::Name { name, error } => {
                write!(formatter, "[taste.{name}] does not name a taste: {error}")
            }
            Self::Taste { name, error } => write!(formatter, "[taste.{name}] is refused: {error}"),
            Self::Set(error) => write!(formatter, "the active taste set is refused: {error}"),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Parse(error) | Self::Row { error, .. } => Some(error),
            Self::Name { error, .. } => Some(error),
            Self::Taste { error, .. } | Self::Set(error) => Some(error),
            Self::Absent | Self::Empty => None,
        }
    }
}
