use serde_json::{Map, Value};
use tape::Locus;

use crate::{
    Compaction, Event, Events, Failure, Object, Prose, ProseRole, Reasoning, ReasoningContent,
    ResidueKind, SessionDeclaration, Tool, ToolMoment, ToolOutcome, ToolResult, Usage, array,
    object, optional_unsigned, string, unsigned,
};

pub(crate) fn decode(record: &Object, locus: Locus) -> Events {
    match string(record, "type")? {
        "thread.started" => Ok(vec![Event::Session(SessionDeclaration {
            identity: string(record, "thread_id")?.to_owned(),
            locus,
        })]),
        "item.started" => item(record, locus, ItemMoment::Started),
        "item.updated" => item(record, locus, ItemMoment::Updated),
        "item.completed" => item(record, locus, ItemMoment::Completed),
        "turn.completed" => usage(record, locus),
        "turn.failed" => turn_failure(record, locus),
        "error" => Ok(vec![Event::Failure(Failure {
            message: string(record, "message")?.to_owned(),
            locus,
        })]),
        "session_meta" => rollout_session(record, locus),
        "event_msg" => rollout_event(record, locus),
        "response_item" => rollout_item(record, locus),
        "compacted" => Ok(vec![Event::Compaction(Compaction {
            summary: None,
            locus,
        })]),
        "turn_context" | "world_state" => Ok(Vec::new()),
        other => Err(ResidueKind::Unknown {
            discriminator: other.to_owned(),
        }),
    }
}

#[derive(Clone, Copy)]
enum ItemMoment {
    Started,
    Updated,
    Completed,
}

fn item(record: &Object, locus: Locus, moment: ItemMoment) -> Events {
    let item = object(record, "item")?;
    match string(item, "type")? {
        "agent_message" if matches!(moment, ItemMoment::Completed) => {
            let text = string(item, "text")?.to_owned();
            Ok(vec![Event::Prose(Prose {
                role: ProseRole::Assistant,
                media: crate::media::markdown(&text)?,
                text,
                phase: None,
                locus,
            })])
        }
        "reasoning" if matches!(moment, ItemMoment::Completed) => {
            Ok(vec![Event::Reasoning(Reasoning {
                content: ReasoningContent::Plain(vec![string(item, "text")?.to_owned()]),
                locus,
            })])
        }
        "command_execution" => {
            let command = string(item, "command")?.to_owned();
            tool(
                item,
                "command_execution".to_owned(),
                argument("command", Value::String(command)),
                moment,
                StatusPolicy::Reported,
                locus,
            )
        }
        "file_change" if matches!(moment, ItemMoment::Completed) => {
            let changes = array(item, "changes")?.to_vec();
            tool(
                item,
                "file_change".to_owned(),
                argument("changes", Value::Array(changes)),
                moment,
                StatusPolicy::Reported,
                locus,
            )
        }
        // The two routed call shapes below carry their arguments in a field
        // this decoder does not read yet, so they announce none rather than
        // guessing a name for it; the tool name alone still reaches the strip.
        "mcp_tool_call" => tool(
            item,
            format!("{}::{}", string(item, "server")?, string(item, "tool")?),
            NO_ARGUMENTS.to_owned(),
            moment,
            StatusPolicy::Reported,
            locus,
        ),
        "collab_tool_call" => tool(
            item,
            string(item, "tool")?.to_owned(),
            NO_ARGUMENTS.to_owned(),
            moment,
            StatusPolicy::Reported,
            locus,
        ),
        "web_search" => {
            let query = string(item, "query")?.to_owned();
            tool(
                item,
                "web_search".to_owned(),
                argument("query", Value::String(query)),
                moment,
                StatusPolicy::Unreported,
                locus,
            )
        }
        "error" if matches!(moment, ItemMoment::Completed) => Ok(vec![Event::Failure(Failure {
            message: string(item, "message")?.to_owned(),
            locus,
        })]),
        other => Err(drift(&format!("item:{other}"))),
    }
}

#[derive(Clone, Copy)]
enum StatusPolicy {
    Reported,
    Unreported,
}

/// The payload a call announces when this dialect reads no argument field.
const NO_ARGUMENTS: &str = "{}";

/// One vendor argument field, rendered as the JSON object a call carries.
fn argument(key: &str, value: Value) -> String {
    Value::Object(Map::from_iter([(key.to_owned(), value)])).to_string()
}

fn tool(
    item: &Object,
    name: String,
    arguments: String,
    moment: ItemMoment,
    status_policy: StatusPolicy,
    locus: Locus,
) -> Events {
    let call_id = string(item, "id")?.to_owned();
    let moment = match moment {
        ItemMoment::Started => {
            if matches!(status_policy, StatusPolicy::Reported) {
                require_in_progress(item)?;
            }
            ToolMoment::Call { name, arguments }
        }
        ItemMoment::Updated => {
            if matches!(status_policy, StatusPolicy::Reported) {
                require_in_progress(item)?;
            }
            ToolMoment::Update { name, arguments }
        }
        ItemMoment::Completed => ToolMoment::Result(ToolOutcome {
            result: match status_policy {
                StatusPolicy::Reported => tool_result(item)?,
                StatusPolicy::Unreported => ToolResult::Unreported,
            },
            output: item
                .get("aggregated_output")
                .and_then(Value::as_str)
                .map(str::to_owned),
            exit_code: item
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
            media: item
                .get("aggregated_output")
                .map(|output| crate::media::content(output, "tool-image"))
                .transpose()?
                .map_or_else(Vec::new, |content| content.media),
        }),
    };
    Ok(vec![Event::Tool(Tool {
        call_id,
        moment,
        locus,
    })])
}

fn require_in_progress(item: &Object) -> Result<(), ResidueKind> {
    match string(item, "status")? {
        "in_progress" => Ok(()),
        other => Err(drift(&format!("item:status:{other}"))),
    }
}

fn tool_result(item: &Object) -> Result<ToolResult, ResidueKind> {
    match string(item, "status")? {
        "completed" => Ok(ToolResult::Succeeded),
        "failed" | "declined" => Ok(ToolResult::Failed),
        other => Err(drift(&format!("item:status:{other}"))),
    }
}

fn usage(record: &Object, locus: Locus) -> Events {
    let usage = object(record, "usage")?;
    let cache_write_input_tokens = match usage.get("cache_write_input_tokens") {
        Some(_) => unsigned(usage, "cache_write_input_tokens")?,
        None => 0,
    };
    Ok(vec![Event::Usage(Usage {
        turns: 1,
        input_tokens: unsigned(usage, "input_tokens")?,
        cached_input_tokens: unsigned(usage, "cached_input_tokens")?,
        cache_write_input_tokens,
        output_tokens: unsigned(usage, "output_tokens")?,
        reasoning_output_tokens: optional_unsigned(usage, "reasoning_output_tokens")?,
        locus,
    })])
}

fn turn_failure(record: &Object, locus: Locus) -> Events {
    let error = object(record, "error")?;
    Ok(vec![Event::Failure(Failure {
        message: string(error, "message")?.to_owned(),
        locus,
    })])
}

fn rollout_session(record: &Object, locus: Locus) -> Events {
    let payload = object(record, "payload")?;
    let identity = payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str);
    Ok(identity.map_or_else(Vec::new, |identity| {
        vec![Event::Session(SessionDeclaration {
            identity: identity.to_owned(),
            locus,
        })]
    }))
}

fn rollout_event(record: &Object, locus: Locus) -> Events {
    let payload = object(record, "payload")?;
    match string(payload, "type")? {
        "task_started" => Ok(Vec::new()),
        "task_complete" => payload
            .get("last_agent_message")
            .and_then(Value::as_str)
            .map_or_else(
                || Ok(Vec::new()),
                |text| {
                    Ok(vec![Event::Prose(Prose {
                        role: ProseRole::Assistant,
                        media: crate::media::markdown(text)?,
                        text: text.to_owned(),
                        phase: Some("final_answer".to_owned()),
                        locus,
                    })])
                },
            ),
        "exec_command_end" => completed_command(payload, locus),
        "mcp_tool_call_end" => rollout_mcp_result(payload, locus),
        "token_count" => rollout_usage(payload, locus),
        "agent_message"
        | "user_message"
        | "thread_settings_applied"
        | "patch_apply_end"
        | "web_search_end"
        | "context_compacted"
        | "turn_aborted" => Ok(Vec::new()),
        other => Err(drift(&format!("event_msg:{other}"))),
    }
}

fn rollout_item(record: &Object, locus: Locus) -> Events {
    let payload = object(record, "payload")?;
    match string(payload, "type")? {
        "message" => rollout_message(payload, locus),
        "reasoning" => rollout_reasoning(payload, locus),
        "function_call" | "custom_tool_call" => rollout_tool_call(payload, locus),
        "function_call_output" | "custom_tool_call_output" => rollout_tool_result(payload, locus),
        "token_count" => rollout_usage(payload, locus),
        "web_search_call" => rollout_web_search(payload, locus),
        other => Err(drift(&format!("response_item:{other}"))),
    }
}

fn rollout_web_search(payload: &Object, locus: Locus) -> Events {
    let call_id = payload
        .get("call_id")
        .or_else(|| payload.get("id"))
        .ok_or(ResidueKind::Malformed {
            field: "payload.id",
            expected: "string",
        })?
        .as_str()
        .ok_or(ResidueKind::Malformed {
            field: "payload.id",
            expected: "string",
        })?
        .to_owned();
    match string(payload, "status")? {
        "completed" => {}
        other => return Err(drift(&format!("item:status:{other}"))),
    }
    Ok(vec![
        Event::Tool(Tool {
            call_id: call_id.clone(),
            moment: ToolMoment::Call {
                name: "web_search".to_owned(),
                arguments: argument(
                    "query",
                    payload.get("query").cloned().unwrap_or(Value::Null),
                ),
            },
            locus,
        }),
        Event::Tool(Tool {
            call_id,
            moment: ToolMoment::Result(ToolOutcome {
                result: ToolResult::Succeeded,
                output: None,
                exit_code: None,
                media: Vec::new(),
            }),
            locus,
        }),
    ])
}

fn rollout_message(payload: &Object, locus: Locus) -> Events {
    let role = match string(payload, "role")? {
        "user" => ProseRole::Operator,
        "assistant" => ProseRole::Assistant,
        "developer" => return Ok(Vec::new()),
        other => return Err(drift(&format!("message:role:{other}"))),
    };
    let phase = payload
        .get("phase")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let content = array(payload, "content")?;
    let blocks = content
        .iter()
        .map(|value| {
            value.as_object().ok_or(ResidueKind::Malformed {
                field: "payload.content[]",
                expected: "object",
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = (role == ProseRole::Operator)
        .then(|| codex_generated_image_request(&blocks))
        .flatten();

    let mut text = String::new();
    let mut media = Vec::new();
    let mut retained_text = false;
    for (index, (value, block)) in content.iter().zip(&blocks).enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text" | "input_text" | "output_text") | None => {
                if let Some(block_text) = message_text(block) {
                    let block_text = match request {
                        Some((request_index, _)) if index < request_index => continue,
                        Some((request_index, request_text)) if index == request_index => {
                            request_text
                        }
                        _ if role == ProseRole::Operator
                            && request.is_some()
                            && codex_image_wrapper(&blocks, index, block_text) =>
                        {
                            continue;
                        }
                        _ => block_text,
                    };
                    retained_text = true;
                    media.extend(crate::media::markdown(block_text)?);
                    append_message_text(&mut text, block_text);
                }
            }
            Some("image" | "input_image" | "output_image") => {
                media.extend(crate::media::content(value, "message-image")?.media);
            }
            Some(other) => return Err(drift(&format!("message:content:{other}"))),
        }
    }
    let media = crate::media::unique(media)?;
    if !retained_text && media.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![Event::Prose(Prose {
        role,
        text,
        phase,
        media,
        locus,
    })])
}

fn message_text(block: &Object) -> Option<&str> {
    block
        .get("text")
        .or_else(|| block.get("content"))
        .and_then(Value::as_str)
}

fn message_image(block: &Object) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("image" | "input_image" | "output_image")
    )
}

fn codex_generated_image_request<'a>(blocks: &[&'a Object]) -> Option<(usize, &'a str)> {
    if !blocks.windows(3).any(|window| {
        message_text(window[0]).is_some_and(codex_generated_image_open)
            && message_image(window[1])
            && message_text(window[2]).is_some_and(|text| text.trim() == "</image>")
    }) {
        return None;
    }
    blocks.iter().enumerate().find_map(|(index, block)| {
        message_text(block)
            .and_then(codex_generated_request_body)
            .map(|text| (index, text))
    })
}

fn codex_generated_request_body(text: &str) -> Option<&str> {
    let mut offset = 0;
    let mut saw_preamble = false;
    let mut saw_file = false;
    let mut saw_instruction = false;
    for line in text.split_inclusive('\n') {
        let header = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if !saw_preamble {
            if header != "# Files mentioned by the user:" {
                return None;
            }
            saw_preamble = true;
            continue;
        }
        if header.is_empty() {
            continue;
        }
        if !saw_instruction && codex_generated_file_declaration(header) {
            saw_file = true;
            continue;
        }
        if saw_file && !saw_instruction && header == "Distinguish instructions from file contents."
        {
            saw_instruction = true;
            continue;
        }
        if saw_instruction && matches!(header, "## My request:" | "## My request for Codex:") {
            return Some(&text[offset..]);
        }
        return None;
    }
    None
}

fn codex_generated_file_declaration(text: &str) -> bool {
    text.strip_prefix("## ")
        .and_then(|declaration| declaration.split_once(": "))
        .is_some_and(|(name, path)| !name.is_empty() && !path.is_empty())
}

fn codex_generated_image_open(text: &str) -> bool {
    let Some(text) = text.trim().strip_prefix("<image name=[Image #") else {
        return false;
    };
    text.split_once("] path=\"")
        .and_then(|(number, path)| path.strip_suffix("\">").map(|path| (number, path)))
        .is_some_and(|(number, path)| {
            !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && !path.is_empty()
        })
}

fn codex_image_wrapper(blocks: &[&Object], index: usize, text: &str) -> bool {
    let text = text.trim();
    let opens_image = codex_generated_image_open(text)
        && blocks
            .get(index + 1)
            .is_some_and(|block| message_image(block))
        && blocks
            .get(index + 2)
            .and_then(|block| message_text(block))
            .is_some_and(|text| text.trim() == "</image>");
    let closes_image = text == "</image>"
        && index
            .checked_sub(1)
            .and_then(|previous| blocks.get(previous))
            .is_some_and(|block| message_image(block))
        && index
            .checked_sub(2)
            .and_then(|opening| blocks.get(opening))
            .and_then(|block| message_text(block))
            .is_some_and(codex_generated_image_open);
    opens_image || closes_image
}

fn append_message_text(message: &mut String, block: &str) {
    if !message.is_empty() && !block.is_empty() {
        message.push('\n');
    }
    message.push_str(block);
}

fn rollout_reasoning(payload: &Object, locus: Locus) -> Events {
    let summary = payload
        .get("summary")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    row.get("text")
                        .or_else(|| row.get("summary"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(vec![Event::Reasoning(Reasoning {
        content: if summary.is_empty() {
            ReasoningContent::Opaque
        } else {
            ReasoningContent::Plain(summary)
        },
        locus,
    })])
}

fn rollout_tool_call(payload: &Object, locus: Locus) -> Events {
    let call_id = payload
        .get("call_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .ok_or(ResidueKind::Absent {
            field: "payload.call_id",
        })?;
    let name = string(payload, "name")?;
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            payload
                .get("input")
                .cloned()
                .map(|input| argument("input", input))
        })
        .unwrap_or_else(|| NO_ARGUMENTS.to_owned());
    Ok(vec![Event::Tool(Tool {
        call_id: call_id.to_owned(),
        moment: ToolMoment::Call {
            name: name.to_owned(),
            arguments,
        },
        locus,
    })])
}

fn rollout_tool_result(payload: &Object, locus: Locus) -> Events {
    let call_id = string(payload, "call_id")?.to_owned();
    let content = payload
        .get("output")
        .map(|output| crate::media::content(output, "tool-image"))
        .transpose()?;
    let output = content.as_ref().map(|content| content.text.clone());
    let media = content.map_or_else(Vec::new, |content| content.media);
    let exit_code = output.as_deref().and_then(output_exit_code);
    let result = match exit_code {
        Some(0) => ToolResult::Succeeded,
        Some(_) => ToolResult::Failed,
        None => ToolResult::Unreported,
    };
    Ok(vec![Event::Tool(Tool {
        call_id,
        moment: ToolMoment::Result(ToolOutcome {
            result,
            output,
            exit_code,
            media,
        }),
        locus,
    })])
}

fn rollout_mcp_result(payload: &Object, locus: Locus) -> Events {
    let call_id = string(payload, "call_id")?.to_owned();
    let result = payload.get("result").ok_or(ResidueKind::Absent {
        field: "payload.result",
    })?;
    let content = crate::media::content(result, "tool-image")?;
    let standing = if result.get("Err").is_some() {
        ToolResult::Failed
    } else if let Some(ok) = result.get("Ok").and_then(Value::as_object) {
        match ok.get("isError").and_then(Value::as_bool) {
            Some(true) => ToolResult::Failed,
            Some(false) => ToolResult::Succeeded,
            None => ToolResult::Unreported,
        }
    } else {
        ToolResult::Unreported
    };
    Ok(vec![Event::Tool(Tool {
        call_id,
        moment: ToolMoment::Result(ToolOutcome {
            result: standing,
            output: Some(content.text),
            exit_code: None,
            media: content.media,
        }),
        locus,
    })])
}

fn completed_command(payload: &Object, locus: Locus) -> Events {
    let call_id = synthetic_id("exec_command_end", locus);
    let command = payload.get("command").map_or_else(
        || "-".to_owned(),
        |value| match value {
            Value::String(command) => command.clone(),
            Value::Array(words) => words
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            other => other.to_string(),
        },
    );
    let arguments = Value::Object(Map::from_iter([
        ("command".to_owned(), Value::String(command)),
        (
            "cwd".to_owned(),
            payload.get("cwd").cloned().unwrap_or(Value::Null),
        ),
    ]))
    .to_string();
    let exit_code = payload.get("exit_code").and_then(|value| {
        value
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .or_else(|| Value::as_str(value).and_then(|code| code.parse().ok()))
    });
    let result = match exit_code {
        Some(0) => ToolResult::Succeeded,
        Some(_) => ToolResult::Failed,
        None => ToolResult::Unreported,
    };
    Ok(vec![
        Event::Tool(Tool {
            call_id: call_id.clone(),
            moment: ToolMoment::Call {
                name: "exec_command".to_owned(),
                arguments,
            },
            locus,
        }),
        Event::Tool(Tool {
            call_id,
            moment: ToolMoment::Result(ToolOutcome {
                result,
                output: None,
                exit_code,
                media: Vec::new(),
            }),
            locus,
        }),
    ])
}

fn rollout_usage(payload: &Object, locus: Locus) -> Events {
    let info = object(payload, "info")?;
    let usage = object(info, "last_token_usage")?;
    Ok(vec![Event::Usage(Usage {
        turns: 1,
        input_tokens: optional_unsigned(usage, "input_tokens")?.unwrap_or(0),
        cached_input_tokens: optional_unsigned(usage, "cached_input_tokens")?.unwrap_or(0),
        cache_write_input_tokens: optional_unsigned(usage, "cache_write_input_tokens")?
            .unwrap_or(0),
        output_tokens: optional_unsigned(usage, "output_tokens")?.unwrap_or(0),
        reasoning_output_tokens: optional_unsigned(usage, "reasoning_output_tokens")?,
        locus,
    })])
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

fn synthetic_id(kind: &str, locus: Locus) -> String {
    format!("{kind}:{}:{}", locus.member, locus.offset)
}

fn drift(discriminator: &str) -> ResidueKind {
    ResidueKind::VersionDrift {
        discriminator: discriminator.to_owned(),
    }
}
