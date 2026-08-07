use tape::Locus;

use crate::{
    Compaction, Event, Events, Failure, Object, Prose, Reasoning, ReasoningContent, ResidueKind,
    SessionDeclaration, Tool, ToolMoment, ToolResult, Usage, array, object, optional_boolean,
    optional_unsigned, required, string,
};

pub(crate) fn decode(record: &Object, locus: Locus) -> Events {
    match string(record, "type")? {
        "assistant" => assistant(record, locus),
        "stream_event" => stream_event(record, locus),
        "user" => user(record, locus),
        "system" => system(record, locus),
        "result" => result(record, locus),
        other => Err(ResidueKind::Unknown {
            discriminator: other.to_owned(),
        }),
    }
}

fn assistant(record: &Object, locus: Locus) -> Events {
    let message = object(record, "message")?;
    if string(message, "type")? != "message" || string(message, "role")? != "assistant" {
        return Err(drift("assistant:message"));
    }

    let mut events = Vec::new();
    for value in array(message, "content")? {
        let block = value.as_object().ok_or(ResidueKind::Malformed {
            field: "message.content[]",
            expected: "object",
        })?;
        match string(block, "type")? {
            "text" => events.push(Event::Prose(Prose {
                text: string(block, "text")?.to_owned(),
                phase: None,
                locus,
            })),
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

fn message_delta(event: &Object, locus: Locus) -> Events {
    let usage = object(event, "usage")?;
    let reasoning_output_tokens = match usage.get("output_tokens_details") {
        Some(_) => optional_unsigned(object(usage, "output_tokens_details")?, "thinking_tokens")?,
        None => None,
    };
    Ok(vec![Event::Usage(Usage {
        turns: 1,
        input_tokens: optional_unsigned(usage, "input_tokens")?.unwrap_or(0),
        cached_input_tokens: optional_unsigned(usage, "cache_read_input_tokens")?.unwrap_or(0),
        cache_write_input_tokens: optional_unsigned(usage, "cache_creation_input_tokens")?
            .unwrap_or(0),
        output_tokens: optional_unsigned(usage, "output_tokens")?.unwrap_or(0),
        reasoning_output_tokens,
        locus,
    })])
}

fn user(record: &Object, locus: Locus) -> Events {
    let message = object(record, "message")?;
    if string(message, "role")? != "user" {
        return Err(drift("user:role"));
    }

    let content = required(message, "content")?;
    let Some(blocks) = content.as_array() else {
        return Err(ResidueKind::Unknown {
            discriminator: "user".to_owned(),
        });
    };

    let mut events = Vec::new();
    for value in blocks {
        let block = value.as_object().ok_or(ResidueKind::Malformed {
            field: "message.content[]",
            expected: "object",
        })?;
        if string(block, "type")? != "tool_result" {
            return Err(drift("user:content"));
        }

        let result = match optional_boolean(block, "is_error")? {
            Some(true) => ToolResult::Failed,
            Some(false) => ToolResult::Succeeded,
            None => ToolResult::Unreported,
        };
        events.push(Event::Tool(Tool {
            call_id: string(block, "tool_use_id")?.to_owned(),
            moment: ToolMoment::Result(result),
            locus,
        }));
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
        "compact_boundary" => Ok(vec![Event::Compaction(Compaction { locus })]),
        other => Err(drift(&format!("system:{other}"))),
    }
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
