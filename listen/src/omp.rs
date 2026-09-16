//! oh-my-pi's RPC frame grammar and the session file it persists beside it.
//!
//! A lane drives omp over `--mode rpc`, so the selected stream is one JSON
//! frame per line. The frames that carry content are `message_end` and the
//! `get_state` response; the rest are lifecycle and transport that the record
//! context and the supervisor read. omp's own session file persists the same
//! message shape under a `message` envelope, so one decoder reads both.

use serde_json::Value;
use tape::Locus;

use crate::{
    Compaction, Event, Events, Failure, Object, Prose, ProseRole, Reasoning, ReasoningContent,
    ResidueKind, SessionDeclaration, Tool, ToolMoment, ToolOutcome, ToolResult, Usage, array,
    media::{self, OwnedContent},
    object, optional_boolean, optional_string, optional_unsigned, required, string,
};

/// RPC frames and session entries that carry no listening moment of their own.
///
/// Every partial, lifecycle, and transport frame is named here rather than
/// left to the unknown arm, so a frame omp adds later is drift the residue
/// accounts for instead of silence.
const SILENT: &[&str] = &[
    // Transport and lifecycle frames.
    "ready",
    "agent_start",
    "agent_end",
    "turn_start",
    "turn_end",
    "message_start",
    "message_update",
    "tool_execution_start",
    "tool_execution_update",
    "tool_execution_end",
    "auto_compaction_start",
    "compaction_summary",
    "extension_ui_request",
    "extension_error",
    "available_commands_update",
    "advisor_cost_changed",
    "session_info_update",
    "session_shutdown",
    "prompt_result",
    "subagent_event",
    "subagent_lifecycle",
    "subagent_progress",
    "host_tool_call",
    "host_tool_cancel",
    "host_uri_request",
    "host_uri_cancel",
    "config_update",
    "rpc_chunk",
    // Session file entries.
    "title",
    "model_change",
    "thinking_level_change",
    "custom",
    "custom_message",
    "branch_summary",
    "label",
    "credential_pin",
    "session_init",
    "mode_change",
    "model_usage",
    "reset_boundary",
    "service_tier_change",
    "ttsr_injection",
];

pub(crate) fn decode(record: &Object, locus: Locus) -> Events {
    match string(record, "type")? {
        "message_end" | "message" => message(object(record, "message")?, locus),
        "response" => response(record, locus),
        "session" => Ok(vec![Event::Session(SessionDeclaration {
            identity: string(record, "id")?.to_owned(),
            locus,
        })]),
        "auto_compaction_end" | "compaction" => compaction(record, locus),
        "rpc_frame_error" => Ok(vec![Event::Failure(Failure {
            message: string(record, "error")?.to_owned(),
            locus,
        })]),
        "notice" => notice(record, locus),
        silent if SILENT.contains(&silent) => Ok(Vec::new()),
        other => Err(ResidueKind::Unknown {
            discriminator: other.to_owned(),
        }),
    }
}

fn message(message: &Object, locus: Locus) -> Events {
    match string(message, "role")? {
        "user" => user(message, locus),
        "assistant" => assistant(message, locus),
        "toolResult" => tool_result(message, locus),
        // Vendor notices to the model: background-job deliveries, `!command`
        // executions, and extension-authored custom messages.
        "custom" | "bashExecution" => Ok(Vec::new()),
        other => Err(drift(&format!("message:{other}"))),
    }
}

fn user(message: &Object, locus: Locus) -> Events {
    let content = required(message, "content")?;
    if let Some(text) = content.as_str() {
        return Ok(vec![operator_prose(text, locus)?]);
    }
    let blocks = content.as_array().ok_or(ResidueKind::Malformed {
        field: "message.content",
        expected: "string or array",
    })?;
    let mut events = Vec::new();
    for value in blocks {
        let block = value.as_object().ok_or(ResidueKind::Malformed {
            field: "message.content[]",
            expected: "object",
        })?;
        match string(block, "type")? {
            "text" => events.push(operator_prose(string(block, "text")?, locus)?),
            "image" => {
                let media = media::decompose(OwnedContent::ProviderValue {
                    value,
                    default_name: "message-image",
                })?
                .media;
                if let Some(Event::Prose(prose)) = events.last_mut() {
                    prose.media.extend(media);
                    wire::media::admit_media(&prose.media).map_err(|_| ResidueKind::Malformed {
                        field: "message.content[].media",
                        expected: "at most eight admitted images",
                    })?;
                } else if !media.is_empty() {
                    events.push(Event::Prose(Prose {
                        role: ProseRole::Operator,
                        text: String::new(),
                        phase: Some("prompt".to_owned()),
                        media,
                        locus,
                    }));
                }
            }
            other => return Err(drift(&format!("user:{other}"))),
        }
    }
    if events.is_empty() {
        return Err(ResidueKind::Absent {
            field: "message.content[]",
        });
    }
    Ok(events)
}

fn operator_prose(text: &str, locus: Locus) -> Result<Event, ResidueKind> {
    Ok(Event::Prose(Prose {
        role: ProseRole::Operator,
        media: media::decompose(OwnedContent::AuthoredText(text))?.media,
        text: text.to_owned(),
        phase: Some("prompt".to_owned()),
        locus,
    }))
}

fn assistant(message: &Object, locus: Locus) -> Events {
    let stop_reason = optional_string(message, "stopReason")?;
    let phase = stop_reason
        .is_some_and(|reason| matches!(reason, "stop" | "length"))
        .then(|| "final_answer".to_owned());
    let mut events = Vec::new();
    for value in array(message, "content")? {
        let block = value.as_object().ok_or(ResidueKind::Malformed {
            field: "message.content[]",
            expected: "object",
        })?;
        match string(block, "type")? {
            "text" => {
                let text = string(block, "text")?.to_owned();
                events.push(Event::Prose(Prose {
                    role: ProseRole::Assistant,
                    media: media::decompose(OwnedContent::AuthoredText(&text))?.media,
                    text,
                    phase: phase.clone(),
                    locus,
                }));
            }
            "thinking" => events.push(Event::Reasoning(Reasoning {
                content: ReasoningContent::Plain(vec![string(block, "thinking")?.to_owned()]),
                locus,
            })),
            "toolCall" => events.push(Event::Tool(Tool {
                call_id: string(block, "id")?.to_owned(),
                moment: ToolMoment::Call {
                    name: string(block, "name")?.to_owned(),
                    arguments: arguments(block),
                },
                locus,
            })),
            other => return Err(drift(&format!("assistant:{other}"))),
        }
    }
    if let Some(usage) = message.get("usage").and_then(Value::as_object) {
        events.push(Event::Usage(usage_event(usage, locus)?));
    }
    if stop_reason == Some("error") {
        events.push(Event::Failure(Failure {
            message: optional_string(message, "errorMessage")?
                .unwrap_or("vendor reported an error")
                .to_owned(),
            locus,
        }));
    }
    Ok(events)
}

fn tool_result(message: &Object, locus: Locus) -> Events {
    let content = media::decompose(OwnedContent::ProviderValue {
        value: required(message, "content")?,
        default_name: "tool-image",
    })?;
    let result = match optional_boolean(message, "isError")? {
        Some(true) => ToolResult::Failed,
        Some(false) => ToolResult::Succeeded,
        None => ToolResult::Unreported,
    };
    let exit_code = message
        .get("details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("exitCode"))
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());
    Ok(vec![Event::Tool(Tool {
        call_id: string(message, "toolCallId")?.to_owned(),
        moment: ToolMoment::Result(ToolOutcome {
            result,
            output: Some(content.text),
            exit_code,
            media: content.media,
        }),
        locus,
    })])
}

/// A command reply. Only `get_state` carries content: the exact session
/// identity a resumed launch names again. A refused command is a failure the
/// lane must see, whatever the command was.
fn response(record: &Object, locus: Locus) -> Events {
    let success = required(record, "success")?
        .as_bool()
        .ok_or(ResidueKind::Malformed {
            field: "success",
            expected: "boolean",
        })?;
    if !success {
        return Ok(vec![Event::Failure(Failure {
            message: format!(
                "{} refused: {}",
                string(record, "command")?,
                optional_string(record, "error")?.unwrap_or("no reason given")
            ),
            locus,
        })]);
    }
    if string(record, "command")? != "get_state" {
        return Ok(Vec::new());
    }
    Ok(vec![Event::Session(SessionDeclaration {
        identity: string(object(record, "data")?, "sessionId")?.to_owned(),
        locus,
    })])
}

fn compaction(record: &Object, locus: Locus) -> Events {
    if optional_boolean(record, "aborted")? == Some(true)
        || optional_boolean(record, "skipped")? == Some(true)
    {
        return Ok(Vec::new());
    }
    let summary = record
        .get("result")
        .and_then(Value::as_object)
        .map_or_else(|| record.get("summary"), |result| result.get("summary"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(vec![Event::Compaction(Compaction { summary, locus })])
}

fn notice(record: &Object, locus: Locus) -> Events {
    if optional_string(record, "level")? != Some("error") {
        return Ok(Vec::new());
    }
    Ok(vec![Event::Failure(Failure {
        message: string(record, "message")?.to_owned(),
        locus,
    })])
}

fn usage_event(usage: &Object, locus: Locus) -> Result<Usage, ResidueKind> {
    Ok(Usage {
        turns: 1,
        input_tokens: optional_unsigned(usage, "input")?.unwrap_or(0),
        cached_input_tokens: optional_unsigned(usage, "cacheRead")?.unwrap_or(0),
        cache_write_input_tokens: optional_unsigned(usage, "cacheWrite")?.unwrap_or(0),
        output_tokens: optional_unsigned(usage, "output")?.unwrap_or(0),
        reasoning_output_tokens: None,
        locus,
    })
}

/// The vendor call payload as JSON, or an empty object when it carries none.
fn arguments(block: &Object) -> String {
    block
        .get("arguments")
        .filter(|arguments| arguments.is_object())
        .map_or_else(|| "{}".to_owned(), ToString::to_string)
}

fn drift(discriminator: &str) -> ResidueKind {
    ResidueKind::VersionDrift {
        discriminator: discriminator.to_owned(),
    }
}
