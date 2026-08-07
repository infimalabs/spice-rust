use serde_json::{Map, Value};
use tape::Locus;

use crate::{
    Event, Events, Failure, Object, Prose, Reasoning, ReasoningContent, ResidueKind,
    SessionDeclaration, Tool, ToolMoment, ToolResult, Usage, array, object, optional_unsigned,
    string, unsigned,
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
            Ok(vec![Event::Prose(Prose {
                text: string(item, "text")?.to_owned(),
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
        ItemMoment::Completed => ToolMoment::Result(match status_policy {
            StatusPolicy::Reported => tool_result(item)?,
            StatusPolicy::Unreported => ToolResult::Unreported,
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

fn drift(discriminator: &str) -> ResidueKind {
    ResidueKind::VersionDrift {
        discriminator: discriminator.to_owned(),
    }
}
