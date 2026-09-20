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
mod media;
mod omp;
mod residue;
mod stream;
mod surface;

pub use canary::{Alarm, Canary};
pub use config::{Config, ConfigError};
pub use event::{
    Compaction, Event, EventKind, Failure, Prose, ProseRole, Reasoning, ReasoningContent,
    SessionDeclaration, Tool, ToolMoment, ToolOutcome, ToolResult, Usage,
};
pub use residue::{
    Decoded, Dialog, LocusLengthError, RecordContext, Residue, ResidueKind, TurnMoment,
};
pub use stream::{
    BOUNDARY_CHUNK_BYTES, Checkpoint, Position, Record, RecordPayload, Scan, ScanError,
    SourcedEvent, SourcedResidue, Span, TailBudget, TailBudgetError, TailScan, TailWork, scan,
    scan_before, scan_tail,
};
pub use surface::{
    Mode, Observation, RetroSelector, StreamOutcome, SurfaceError, SurfaceEvent, SurfaceReasoning,
    SurfaceResidue, SurfaceToolMoment, SurfaceToolResult, Watch, retro,
};
pub use tape::{Locus, SourceId};

use std::error::Error;
use std::fmt;

use serde_json::{Map, Value};

/// The installed vendor stream grammar to apply.
#[derive(
    Clone, Copy, Debug, serde::Deserialize, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    /// Claude Code JSONL records.
    Claude,
    /// Codex rollout JSONL records.
    Codex,
    /// oh-my-pi RPC frames and session JSONL records.
    Omp,
}

impl Dialect {
    /// Stable surface spelling of this decoder.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Omp => "omp",
        }
    }
}

/// One record decoded under its sole recognized vendor grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Detected {
    /// Grammar that owned the record.
    pub dialect: Dialect,
    /// Typed or exactly accounted record.
    pub decoded: Decoded,
}

/// Why automatic record ownership could not be established.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordDecodeError {
    /// The supplied locus did not cover the submitted bytes.
    Locus(LocusLengthError),
    /// The record could not be parsed far enough to establish ownership.
    Invalid {
        /// Exact structural failure observed while parsing.
        kind: ResidueKind,
    },
    /// No explicit or structurally detectable dialect owned the record.
    Unowned,
    /// The record structurally names a different dialect from the caller's hint.
    Disagreement {
        /// Dialect selected by the caller.
        expected: Dialect,
        /// Dialect named by the record envelope.
        observed: Dialect,
        /// Top-level discriminator that proved the disagreement.
        discriminator: String,
    },
}

impl fmt::Display for RecordDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locus(error) => error.fmt(formatter),
            Self::Invalid { kind } => write!(formatter, "invalid unowned record: {kind:?}"),
            Self::Unowned => formatter
                .write_str("record has neither an explicit dialect nor a uniquely owned envelope"),
            Self::Disagreement {
                expected,
                observed,
                discriminator,
            } => write!(
                formatter,
                "{} record {discriminator:?} was supplied to the {} decoder",
                observed.as_str(),
                expected.as_str()
            ),
        }
    }
}

impl Error for RecordDecodeError {}

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
    validate_locus(bytes, locus)?;
    Ok(decode_value(dialect, parse(bytes), locus))
}

/// Whether one record is a partial the same stream restates whole later.
///
/// A capture that keeps every record may leave such a record out: nothing is
/// decoded from the partial itself, and what it carries arrives again,
/// complete, in the frame that ends it. A record that does not parse is
/// never restated.
#[must_use]
pub fn restated(dialect: Dialect, bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Kind {
        #[serde(rename = "type")]
        kind: String,
    }
    dialect == Dialect::Omp
        && serde_json::from_slice::<Kind>(bytes).is_ok_and(|record| omp::restated(&record.kind))
}

/// Decode one record using an explicit dialect or its uniquely owned envelope.
///
/// The JSON value is parsed exactly once. When `expected` is present, an
/// envelope owned by the other dialect is a classification disagreement rather
/// than a fallback opportunity. Without an explicit dialect, records whose
/// envelope is not uniquely owned are refused.
///
/// # Errors
///
/// Returns [`RecordDecodeError`] when the locus is invalid, no dialect owns the
/// envelope, or the envelope disagrees with the explicit dialect.
pub fn decode_record(
    expected: Option<Dialect>,
    bytes: &[u8],
    locus: Locus,
) -> Result<Detected, RecordDecodeError> {
    validate_locus(bytes, locus).map_err(RecordDecodeError::Locus)?;
    let parsed = parse(bytes);
    if expected.is_none()
        && let Err(kind) = &parsed
    {
        return Err(RecordDecodeError::Invalid { kind: kind.clone() });
    }
    let detected = parsed
        .as_ref()
        .ok()
        .and_then(Value::as_object)
        .and_then(detect_dialect);
    let dialect = match (expected, detected) {
        (Some(expected), Some(observed)) if expected != observed => {
            let discriminator = parsed
                .as_ref()
                .ok()
                .and_then(Value::as_object)
                .and_then(|object| object.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
                .to_owned();
            return Err(RecordDecodeError::Disagreement {
                expected,
                observed,
                discriminator,
            });
        }
        (Some(expected), _) => expected,
        (None, Some(observed)) => observed,
        (None, None) => return Err(RecordDecodeError::Unowned),
    };
    Ok(Detected {
        dialect,
        decoded: decode_value(dialect, parsed, locus),
    })
}

fn validate_locus(bytes: &[u8], locus: Locus) -> Result<(), LocusLengthError> {
    let actual = u64::try_from(bytes.len()).expect("usize always fits in u64");
    if locus.length == actual {
        Ok(())
    } else {
        Err(LocusLengthError {
            locus,
            actual_length: actual,
        })
    }
}

fn parse(bytes: &[u8]) -> Result<Value, ResidueKind> {
    match wire::decode::<Value>(bytes) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err(ResidueKind::Malformed {
            field: "$",
            expected: "object",
        }),
        Err(error) if error.is_eof() => Err(ResidueKind::Truncated),
        Err(_) => Err(ResidueKind::Malformed {
            field: "$",
            expected: "JSON record",
        }),
    }
}

fn decode_value(dialect: Dialect, parsed: Result<Value, ResidueKind>, locus: Locus) -> Decoded {
    let object = match parsed {
        Ok(value) => value
            .as_object()
            .expect("parse admits only JSON objects")
            .clone(),
        Err(kind) => return Decoded::Residue(Residue::new(dialect, locus, kind)),
    };
    let context = match record_context(dialect, &object) {
        Ok(context) => context,
        Err(kind) => return Decoded::Residue(Residue::new(dialect, locus, kind)),
    };
    let decoded = match dialect {
        Dialect::Claude => claude::decode(&object, locus),
        Dialect::Codex => codex::decode(&object, locus),
        Dialect::Omp => omp::decode(&object, locus),
    };
    match decoded {
        Ok(events) => Decoded::Events { context, events },
        Err(kind) => Decoded::Residue(Residue::new(dialect, locus, kind)),
    }
}

fn detect_dialect(object: &Object) -> Option<Dialect> {
    match object.get("type").and_then(Value::as_str)? {
        "assistant" | "stream_event" | "user" | "system" | "result" | "summary"
        | "rate_limit_event" | "attachment" | "last-prompt" | "ai-title" | "mode"
        | "queue-operation" | "atis-latch" => Some(Dialect::Claude),
        "thread.started" | "item.started" | "item.updated" | "item.completed"
        | "turn.completed" | "turn.failed" | "session_meta" | "event_msg" | "response_item"
        | "compacted" | "turn_context" | "world_state" => Some(Dialect::Codex),
        "ready"
        | "agent_start"
        | "agent_end"
        | "turn_start"
        | "turn_end"
        | "message_start"
        | "message_update"
        | "message_end"
        | "tool_execution_start"
        | "tool_execution_end"
        | "auto_compaction_start"
        | "auto_compaction_end"
        | "extension_ui_request"
        | "available_commands_update"
        | "rpc_frame_error"
        | "rpc_chunk"
        | "session"
        | "model_change"
        | "thinking_level_change"
        | "notice"
        | "title"
        | "compaction"
        | "credential_pin"
        | "custom"
        | "custom_message"
        | "branch_summary"
        | "label"
        | "session_init"
        | "mode_change"
        | "model_usage"
        | "reset_boundary"
        | "service_tier_change"
        | "ttsr_injection" => Some(Dialect::Omp),
        _ => None,
    }
}

type Object = Map<String, Value>;
type Events = Result<Vec<Event>, ResidueKind>;

fn record_context(dialect: Dialect, record: &Object) -> Result<RecordContext, ResidueKind> {
    let timestamp = optional_string(record, "timestamp")?.map(str::to_owned);
    let payload = record.get("payload").and_then(Value::as_object);
    let message = record.get("message").and_then(Value::as_object);
    let event = record.get("event").and_then(Value::as_object);
    let turn_id = match dialect {
        Dialect::Codex => payload
            .and_then(|payload| payload.get("turn_id").and_then(Value::as_str))
            .or_else(|| {
                payload
                    .and_then(|payload| payload.get("internal_chat_message_metadata_passthrough"))
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("turn_id"))
                    .and_then(Value::as_str)
            }),
        Dialect::Claude => record.get("promptId").and_then(Value::as_str),
        Dialect::Omp => None,
    }
    .map(str::to_owned);
    let top = record.get("type").and_then(Value::as_str).unwrap_or("");
    let nested = payload
        .and_then(|payload| payload.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let turn = match dialect {
        Dialect::Codex => match (top, nested) {
            ("event_msg", "task_started") => Some(TurnMoment::Started),
            ("event_msg", "task_complete" | "turn_aborted")
            | ("turn.completed" | "turn.failed", _) => Some(TurnMoment::Completed),
            _ if turn_id.is_some() => Some(TurnMoment::Observed),
            _ => None,
        },
        Dialect::Claude => match top {
            "user" if turn_id.is_some() && claude_operator_prompt(message) => {
                Some(TurnMoment::Started)
            }
            "user" if turn_id.is_some() => Some(TurnMoment::Observed),
            "assistant"
                if message
                    .and_then(|message| message.get("stop_reason"))
                    .and_then(Value::as_str)
                    .is_some_and(claude_turn_completed) =>
            {
                Some(TurnMoment::Completed)
            }
            "stream_event"
                if event
                    .and_then(|event| event.get("delta"))
                    .and_then(Value::as_object)
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                    .is_some_and(claude_turn_completed) =>
            {
                Some(TurnMoment::Completed)
            }
            _ => None,
        },
        Dialect::Omp => match top {
            "agent_start" => Some(TurnMoment::Started),
            "agent_end" => Some(TurnMoment::Completed),
            "message" => omp::session_turn(message),
            _ => None,
        },
    };
    let model_context_window = match dialect {
        Dialect::Codex => payload
            .and_then(|payload| payload.get("model_context_window"))
            .or_else(|| {
                payload
                    .and_then(|payload| payload.get("info"))
                    .and_then(Value::as_object)
                    .and_then(|info| info.get("model_context_window"))
            }),
        Dialect::Claude => message
            .and_then(|message| message.get("model_context_window"))
            .or_else(|| record.get("model_context_window")),
        Dialect::Omp => record
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("model"))
            .and_then(Value::as_object)
            .and_then(|model| model.get("contextWindow")),
    }
    .map(|value| {
        value.as_u64().ok_or(ResidueKind::Malformed {
            field: "model_context_window",
            expected: "unsigned integer",
        })
    })
    .transpose()?;
    Ok(RecordContext {
        timestamp,
        turn_id,
        turn,
        model_context_window,
        dialog: (dialect == Dialect::Omp)
            .then(|| omp::dialog(record))
            .flatten(),
        withdrawn: (dialect == Dialect::Omp)
            .then(|| omp::withdrawn(record))
            .flatten(),
    })
}

fn claude_operator_prompt(message: Option<&Object>) -> bool {
    let Some(content) = message.and_then(|message| message.get("content")) else {
        return false;
    };
    if content.is_string() {
        return true;
    }
    let Some(blocks) = content.as_array() else {
        return false;
    };
    let mut prompt = false;
    for content_type in blocks
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|item| item.get("type"))
        .filter_map(Value::as_str)
    {
        if content_type == "tool_result" {
            return false;
        }
        prompt |= matches!(content_type, "text" | "image");
    }
    prompt
}

fn claude_turn_completed(stop_reason: &str) -> bool {
    matches!(
        stop_reason,
        "end_turn" | "stop_sequence" | "max_tokens" | "refusal"
    )
}

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

fn optional_unsigned(object: &Object, key: &'static str) -> Result<Option<u64>, ResidueKind> {
    object
        .get(key)
        .map(|value| {
            value.as_u64().ok_or(ResidueKind::Malformed {
                field: key,
                expected: "unsigned integer",
            })
        })
        .transpose()
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

fn optional_string<'a>(
    object: &'a Object,
    key: &'static str,
) -> Result<Option<&'a str>, ResidueKind> {
    object
        .get(key)
        .map(|value| {
            value.as_str().ok_or(ResidueKind::Malformed {
                field: key,
                expected: "string",
            })
        })
        .transpose()
}
