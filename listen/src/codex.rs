use tape::Locus;

use crate::{
    Event, Events, Failure, Object, Prose, Reasoning, ReasoningContent, ResidueKind,
    SessionDeclaration, Tool, ToolMoment, ToolResult, Usage, array, object, string, unsigned,
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
            let _ = string(item, "command")?;
            tool(
                item,
                "command_execution".to_owned(),
                moment,
                StatusPolicy::Reported,
                locus,
            )
        }
        "file_change" if matches!(moment, ItemMoment::Completed) => {
            let _ = array(item, "changes")?;
            tool(
                item,
                "file_change".to_owned(),
                moment,
                StatusPolicy::Reported,
                locus,
            )
        }
        "mcp_tool_call" => tool(
            item,
            format!("{}::{}", string(item, "server")?, string(item, "tool")?),
            moment,
            StatusPolicy::Reported,
            locus,
        ),
        "collab_tool_call" => tool(
            item,
            string(item, "tool")?.to_owned(),
            moment,
            StatusPolicy::Reported,
            locus,
        ),
        "web_search" => {
            let _ = string(item, "query")?;
            tool(
                item,
                "web_search".to_owned(),
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

fn tool(
    item: &Object,
    name: String,
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
            ToolMoment::Call { name }
        }
        ItemMoment::Updated => {
            if matches!(status_policy, StatusPolicy::Reported) {
                require_in_progress(item)?;
            }
            ToolMoment::Update { name }
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
        input_tokens: unsigned(usage, "input_tokens")?,
        cached_input_tokens: unsigned(usage, "cached_input_tokens")?,
        cache_write_input_tokens,
        output_tokens: unsigned(usage, "output_tokens")?,
        reasoning_output_tokens: Some(unsigned(usage, "reasoning_output_tokens")?),
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
