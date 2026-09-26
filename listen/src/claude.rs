use tape::Locus;

use crate::{
    Compaction, Event, Events, Failure, Object, Prose, ProseRole, Reasoning, ReasoningContent,
    ResidueKind, SessionDeclaration, Tool, ToolMoment, ToolOutcome, ToolResult, Usage, array,
    media::{self, OwnedContent},
    object, optional_boolean, optional_string, optional_unsigned, required, string,
};

pub(crate) fn decode(record: &Object, locus: Locus) -> Events {
    match string(record, "type")? {
        "assistant" => assistant(record, locus),
        "stream_event" => stream_event(record, locus),
        "user" => user(record, locus),
        "system" => system(record, locus),
        "result" => result(record, locus),
        "summary" => Ok(vec![Event::Compaction(Compaction {
            summary: Some(string(record, "summary")?.to_owned()),
            boundary: None,
            locus,
        })]),
        "rate_limit_event" => rate_limit(record, locus),
        "error" => Ok(vec![Event::Failure(Failure {
            message: record
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("vendor reported an error")
                .to_owned(),
            locus,
        })]),
        "attachment" | "last-prompt" | "ai-title" | "mode" | "queue-operation" | "atis-latch" => {
            Ok(Vec::new())
        }
        other => Err(ResidueKind::Unknown {
            discriminator: other.to_owned(),
        }),
    }
}

/// A rate limit report is a failure only when the vendor rejected the request.
///
/// Claude Code reports its rate limit standing beside ordinary responses, and
/// an allowed standing, with or without a warning, says the request went
/// through. Only a rejection is the vendor's word that the turn did not, so
/// only a rejection decodes to the failure an exit can be held on.
fn rate_limit(record: &Object, locus: Locus) -> Events {
    let Some(info) = record
        .get("rate_limit_info")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(vec![Event::Failure(Failure {
            message: optional_string(record, "message")?
                .unwrap_or("vendor reported an error")
                .to_owned(),
            locus,
        })]);
    };
    let status = string(info, "status")?;
    if matches!(status, "allowed" | "allowed_warning") {
        return Ok(Vec::new());
    }
    let window = optional_string(info, "rateLimitType")?.unwrap_or("request");
    Ok(vec![Event::Failure(Failure {
        message: format!("rate limit {status} for the {window} window"),
        locus,
    })])
}

fn assistant(record: &Object, locus: Locus) -> Events {
    let message = object(record, "message")?;
    if string(message, "type")? != "message" || string(message, "role")? != "assistant" {
        return Err(drift("assistant:message"));
    }

    let phase = message
        .get("stop_reason")
        .and_then(serde_json::Value::as_str)
        .is_some_and(terminal_stop_reason)
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
            "redacted_thinking" => {
                let _ = string(block, "data")?;
                events.push(Event::Reasoning(Reasoning {
                    content: ReasoningContent::Opaque,
                    locus,
                }));
            }
            "tool_use" => events.push(Event::Tool(Tool {
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

    if record.contains_key("timestamp")
        && let Some(usage) = message.get("usage").and_then(serde_json::Value::as_object)
    {
        events.push(Event::Usage(usage_event(usage, locus)?));
    }

    Ok(events)
}

fn stream_event(record: &Object, locus: Locus) -> Events {
    let event = object(record, "event")?;
    match string(event, "type")? {
        "message_delta" => message_delta(event, locus),
        other => Err(ResidueKind::Unknown {
            discriminator: format!("stream_event:{other}"),
        }),
    }
}

/// Claude streaming spend reported by a terminal message delta.
///
/// Persisted transcript records carry usage on timestamped assistant messages
/// instead; this shape exists only when the vendor launches with partial
/// messages.
fn message_delta(event: &Object, locus: Locus) -> Events {
    let usage = object(event, "usage")?;
    let reasoning_output_tokens = match usage.get("output_tokens_details") {
        Some(_) => optional_unsigned(object(usage, "output_tokens_details")?, "thinking_tokens")?,
        None => None,
    };
    let mut normalized = usage_event(usage, locus)?;
    normalized.reasoning_output_tokens = reasoning_output_tokens;
    Ok(vec![Event::Usage(normalized)])
}

fn user(record: &Object, locus: Locus) -> Events {
    let message = object(record, "message")?;
    if string(message, "role")? != "user" {
        return Err(drift("user:role"));
    }

    let content = required(message, "content")?;
    if let Some(text) = content.as_str() {
        return Ok(vec![Event::Prose(Prose {
            role: ProseRole::Operator,
            media: media::decompose(OwnedContent::AuthoredText(text))?.media,
            text: text.to_owned(),
            phase: Some("prompt".to_owned()),
            locus,
        })]);
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
            "text" => {
                let text = string(block, "text")?.to_owned();
                events.push(Event::Prose(Prose {
                    role: ProseRole::Operator,
                    media: media::decompose(OwnedContent::AuthoredText(&text))?.media,
                    text,
                    phase: Some("prompt".to_owned()),
                    locus,
                }));
            }
            "image" => {
                let media = media::decompose(OwnedContent::ProviderValue {
                    value,
                    default_name: "message-image",
                })?
                .media;
                if let Some(Event::Prose(prose)) = events.last_mut() {
                    prose.media.extend(media);
                    prose.media.truncate(wire::media::MAX_MEDIA_ITEMS);
                    wire::media::validate_media(&prose.media).map_err(|_| {
                        ResidueKind::Malformed {
                            field: "message.content[].media",
                            expected: "at most eight admitted images",
                        }
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
            "tool_result" => {
                let content = block
                    .get("content")
                    .map(|content| {
                        media::decompose(OwnedContent::ProviderValue {
                            value: content,
                            default_name: "tool-image",
                        })
                    })
                    .transpose()?;
                let result = match optional_boolean(block, "is_error")? {
                    Some(true) => ToolResult::Failed,
                    Some(false) => ToolResult::Succeeded,
                    None => ToolResult::Unreported,
                };
                events.push(Event::Tool(Tool {
                    call_id: string(block, "tool_use_id")?.to_owned(),
                    moment: ToolMoment::Result(ToolOutcome {
                        result,
                        output: content.as_ref().map(|content| content.text.clone()),
                        exit_code: content
                            .as_ref()
                            .and_then(|content| output_exit_code(&content.text)),
                        media: content.map_or_else(Vec::new, |content| content.media),
                    }),
                    locus,
                }));
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

fn system(record: &Object, locus: Locus) -> Events {
    match string(record, "subtype")? {
        "init" => Ok(vec![Event::Session(SessionDeclaration {
            identity: string(record, "session_id")?.to_owned(),
            locus,
        })]),
        "compact_boundary" => Ok(vec![Event::Compaction(Compaction {
            summary: None,
            boundary: None,
            locus,
        })]),
        "api_error" => {
            let error = object(record, "error")?;
            Ok(vec![Event::Failure(Failure {
                message: error
                    .get("formatted")
                    .or_else(|| error.get("message"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ResidueKind::Absent {
                        field: "error.message",
                    })?
                    .to_owned(),
                locus,
            })])
        }
        other => Err(drift(&format!("system:{other}"))),
    }
}

fn usage_event(usage: &Object, locus: Locus) -> Result<Usage, ResidueKind> {
    Ok(Usage {
        turns: 1,
        input_tokens: optional_unsigned(usage, "input_tokens")?.unwrap_or(0),
        cached_input_tokens: optional_unsigned(usage, "cache_read_input_tokens")?.unwrap_or(0),
        cache_write_input_tokens: optional_unsigned(usage, "cache_creation_input_tokens")?
            .unwrap_or(0),
        output_tokens: optional_unsigned(usage, "output_tokens")?.unwrap_or(0),
        reasoning_output_tokens: optional_unsigned(usage, "reasoning_output_tokens")?,
        locus,
    })
}

fn output_exit_code(output: &str) -> Option<i32> {
    [
        "Process exited with code ",
        "exit_code\":",
        "\"exit_code\":",
    ]
    .into_iter()
    .find_map(|prefix| {
        let rest = output.split(prefix).nth(1)?;
        rest.trim_start()
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '-')
            .collect::<String>()
            .parse()
            .ok()
    })
}

fn terminal_stop_reason(stop_reason: &str) -> bool {
    matches!(
        stop_reason,
        "end_turn" | "stop_sequence" | "max_tokens" | "refusal"
    )
}

fn result(record: &Object, locus: Locus) -> Events {
    let subtype = string(record, "subtype")?;
    let is_error = required(record, "is_error")?
        .as_bool()
        .ok_or(ResidueKind::Malformed {
            field: "is_error",
            expected: "boolean",
        })?;

    match (subtype, is_error) {
        ("success", false) => {
            return Err(ResidueKind::Unknown {
                discriminator: "result:success".to_owned(),
            });
        }
        (
            "error_during_execution"
            | "error_max_turns"
            | "error_max_budget_usd"
            | "error_max_structured_output_retries",
            true,
        ) => {}
        _ => return Err(drift(&format!("result:{subtype}"))),
    }

    let mut messages = Vec::new();
    for value in array(record, "errors")? {
        messages.push(
            value
                .as_str()
                .ok_or(ResidueKind::Malformed {
                    field: "errors[]",
                    expected: "string",
                })?
                .to_owned(),
        );
    }
    if messages.is_empty() {
        return Err(ResidueKind::Absent { field: "errors[]" });
    }

    Ok(vec![Event::Failure(Failure {
        message: messages.join("\n"),
        locus,
    })])
}

/// The vendor call payload as JSON, or an empty object when it carries none.
///
/// A `tool_use` block whose `input` is absent, or is a bare scalar, is still a
/// call an operator can read by name, so a missing payload is not version
/// drift; it is an empty argument list, which is what the predecessor records
/// for the same block.
fn arguments(block: &Object) -> String {
    block
        .get("input")
        .filter(|input| input.is_object())
        .map_or_else(|| "{}".to_owned(), ToString::to_string)
}

fn drift(discriminator: &str) -> ResidueKind {
    ResidueKind::VersionDrift {
        discriminator: discriminator.to_owned(),
    }
}
