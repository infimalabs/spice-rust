//! Pure decoding from preserved vendor records into listening events.
//!
//! Callers commit bytes to tape first, then pass the returned [`Locus`] and
//! those exact bytes to [`decode`]. A record is interpreted by one current
//! dialect or retained whole as [`Residue`]; decoding never performs I/O and
//! never turns vendor telemetry into a wire fact.

#![forbid(unsafe_code)]

mod canary;
mod claude;
mod codex;
pub mod config;
mod event;
mod residue;
mod surface;

pub use canary::{Alarm, Canary};
pub use config::{Config, ConfigError};
pub use event::{
    Compaction, Event, EventKind, Failure, Prose, Reasoning, ReasoningContent, Tool, ToolMoment,
    ToolResult, Usage,
};
pub use residue::{Decoded, LocusLengthError, Residue, ResidueKind};
pub use surface::{
    Mode, Observation, RetroSelector, Span, StreamOutcome, SurfaceError, Watch, retro,
};
pub use tape::Locus;

use serde_json::{Map, Value};

/// The installed vendor stream grammar to apply.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Dialect {
    /// Claude Code JSONL records.
    Claude,
    /// Codex rollout JSONL records.
    Codex,
}

impl Dialect {
    /// Stable surface spelling of this decoder.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// Decode one complete vendor record already committed to tape.
///
/// The supplied locus must cover exactly `bytes`. Syntactically or
/// structurally unrecognized bytes return accounted [`Residue`] rather than an
/// error; a mismatched locus is a caller contract violation and fails.
///
/// # Errors
///
/// Returns [`LocusLengthError`] when the locus length differs from the byte
/// slice length.
pub fn decode(dialect: Dialect, bytes: &[u8], locus: Locus) -> Result<Decoded, LocusLengthError> {
    let actual = u64::try_from(bytes.len()).expect("usize always fits in u64");
    if locus.length != actual {
        return Err(LocusLengthError {
            locus,
            actual_length: actual,
        });
    }

    let value = match wire::decode::<Value>(bytes) {
        Ok(value) => value,
        Err(error) if error.is_eof() => {
            return Ok(Decoded::Residue(Residue::new(
                dialect,
                locus,
                ResidueKind::Truncated,
            )));
        }
        Err(_) => {
            return Ok(Decoded::Residue(Residue::new(
                dialect,
                locus,
                ResidueKind::Malformed {
                    field: "$",
                    expected: "JSON record",
                },
            )));
        }
    };

    let object = match value.as_object() {
        Some(object) => object,
        None => {
            return Ok(Decoded::Residue(Residue::new(
                dialect,
                locus,
                ResidueKind::Malformed {
                    field: "$",
                    expected: "object",
                },
            )));
        }
    };

    let decoded = match dialect {
        Dialect::Claude => claude::decode(object, locus),
        Dialect::Codex => codex::decode(object, locus),
    };

    Ok(match decoded {
        Ok(events) => Decoded::Events(events),
        Err(kind) => Decoded::Residue(Residue::new(dialect, locus, kind)),
    })
}

type Object = Map<String, Value>;
type Events = Result<Vec<Event>, ResidueKind>;

fn required<'a>(object: &'a Object, key: &'static str) -> Result<&'a Value, ResidueKind> {
    object.get(key).ok_or(ResidueKind::Absent { field: key })
}

fn object<'a>(object: &'a Object, key: &'static str) -> Result<&'a Object, ResidueKind> {
    required(object, key)?
        .as_object()
        .ok_or(ResidueKind::Malformed {
            field: key,
            expected: "object",
        })
}

fn array<'a>(object: &'a Object, key: &'static str) -> Result<&'a [Value], ResidueKind> {
    required(object, key)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or(ResidueKind::Malformed {
            field: key,
            expected: "array",
        })
}

fn string<'a>(object: &'a Object, key: &'static str) -> Result<&'a str, ResidueKind> {
    required(object, key)?
        .as_str()
        .ok_or(ResidueKind::Malformed {
            field: key,
            expected: "string",
        })
}

fn unsigned(object: &Object, key: &'static str) -> Result<u64, ResidueKind> {
    required(object, key)?
        .as_u64()
        .ok_or(ResidueKind::Malformed {
            field: key,
            expected: "unsigned integer",
        })
}

fn optional_boolean(object: &Object, key: &'static str) -> Result<Option<bool>, ResidueKind> {
    object
        .get(key)
        .map(|value| {
            value.as_bool().ok_or(ResidueKind::Malformed {
                field: key,
                expected: "boolean",
            })
        })
        .transpose()
}
