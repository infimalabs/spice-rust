//! Operator-facing preview text for one tool moment.
//!
//! The lane status strip is read by a person, so a tool moment renders as the
//! work being done rather than as vendor bookkeeping: generic command-execution
//! calls suppress their taxonomy only when the command is already informative,
//! named shells retain their family, and results show the call they answer
//! instead of the `tool_use` id that joins them. The rules here follow the
//! predecessor at `spice-z spice/serve/messagepresentation.py`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use wire::CanonicalUuid;

#[cfg(test)]
mod tests;

/// Longest preview one strip line carries before it is elided.
pub(in crate::resident) const MAX_CHARS: usize = 120;
const MIN_INFORMATIVE_TOKENS: usize = 2;
const MAX_EVENT_IDENTITY_CHARS: usize = 18;

/// Argument fields worth showing, most recognizable first.
///
/// `command` leads the list because a shell call is the moment this strip
/// exists for. The predecessor reaches the same field through its trailing
/// insertion-order scan, which a sorted JSON object cannot reproduce, so the
/// field it depends on is named here instead of being arrived at by luck.
const ARGUMENT_KEYS: [&str; 8] = [
    "command", "cmd", "path", "query", "url", "input", "prompt", "text",
];

/// Call previews held only while their calls are still outstanding.
///
/// A result record names no tool: `listen::ToolMoment::Result` carries the
/// vendor correlation id alone, because no dialect repeats the name on the
/// record that answers a call. Taking the entry on a result is what bounds this
/// map -- it holds one entry per call still in flight rather than one per call
/// ever observed.
///
/// The lane is part of the key because one resident watches every lane at once.
/// A vendor id is unique within a transcript, not across the fleet, and a strip
/// showing another lane's command would be worse than showing none.
#[derive(Debug, Default)]
pub(in crate::resident) struct CallPreviews {
    by_call: BTreeMap<(CanonicalUuid, String), String>,
}

impl CallPreviews {
    pub(super) fn clear_lane(&mut self, lane: CanonicalUuid) {
        self.by_call.retain(|(candidate, _), _| *candidate != lane);
    }
}

/// Render one tool moment as the line an operator reads.
pub(super) fn activity(
    lane: CanonicalUuid,
    tool: &listen::Tool,
    previews: &mut CallPreviews,
) -> String {
    let key = (lane, tool.call_id.clone());
    match &tool.moment {
        listen::ToolMoment::Call { name, arguments } => {
            let preview = identified_preview(name, arguments, &tool.call_id);
            previews.by_call.insert(key, preview.clone());
            preview
        }
        listen::ToolMoment::Update { name, arguments } => {
            let preview = identified_preview(name, arguments, &tool.call_id);
            previews.by_call.insert(key, preview.clone());
            flatten(&format!("{preview} update"))
        }
        listen::ToolMoment::Result(result) => {
            let outcome = match result.result {
                listen::ToolResult::Succeeded => "succeeded",
                listen::ToolResult::Failed => "failed",
                listen::ToolResult::Unreported => "completed",
            };
            previews.by_call.remove(&key).map_or_else(
                || flatten(&format!("{} {outcome}", event_identity(&tool.call_id))),
                |preview| flatten(&format!("{preview} {outcome}")),
            )
        }
    }
}

fn identified_preview(name: &str, arguments: &str, call_id: &str) -> String {
    let normalized = normalized_name(name);
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("tool") {
        return named_preview(
            &event_identity(call_id),
            &flatten(&argument_preview(arguments)),
            "activity",
        );
    }
    ordinary_call_preview(name, arguments)
}

fn event_identity(call_id: &str) -> String {
    let identity = flatten(call_id);
    let count = identity.chars().count();
    if count == 0 {
        return "activity".to_owned();
    }
    if count <= MAX_EVENT_IDENTITY_CHARS {
        return identity;
    }
    let leading = MAX_EVENT_IDENTITY_CHARS / 2;
    let trailing = MAX_EVENT_IDENTITY_CHARS - leading - 1;
    format!(
        "{}…{}",
        identity.chars().take(leading).collect::<String>(),
        identity.chars().skip(count - trailing).collect::<String>(),
    )
}

pub(super) fn message_text(tool: &listen::Tool, activity: String, media: &[wire::Media]) -> String {
    if media.is_empty()
        || matches!(
            &tool.moment,
            listen::ToolMoment::Result(outcome)
                if outcome.output.as_deref().is_some_and(|text| !text.trim().is_empty())
        )
    {
        activity
    } else {
        String::new()
    }
}

pub(super) fn call_preview(name: &str, arguments: &str) -> String {
    ordinary_call_preview(name, arguments)
}

fn ordinary_call_preview(name: &str, arguments: &str) -> String {
    let name = normalized_name(name);
    let arguments = argument_preview(arguments);
    let fallback = match name.to_ascii_lowercase().as_str() {
        "bash" | "zsh" => "shell",
        "shell" | "local shell" => "command",
        _ => "tool",
    };
    named_preview(&name, &arguments, fallback)
}

fn normalized_name(name: &str) -> String {
    flatten(name.trim())
}

fn named_preview(name: &str, arguments: &str, fallback: &str) -> String {
    let preview = match (name.is_empty(), arguments.is_empty()) {
        (false, false) => flatten(&format!("{name}: {arguments}")),
        (false, true) => name.to_owned(),
        (true, false) => flatten(arguments),
        (true, true) => String::new(),
    };
    complete_preview(&preview, fallback)
}

fn complete_preview(preview: &str, fallback: &str) -> String {
    if informative_tokens(preview) >= MIN_INFORMATIVE_TOKENS {
        return preview.to_owned();
    }
    if preview.is_empty() {
        return format!("{fallback} activity");
    }
    let completed = if preview.eq_ignore_ascii_case(fallback) {
        format!("{preview} activity")
    } else {
        format!("{preview} {fallback}")
    };
    if completed.chars().count() <= MAX_CHARS {
        completed
    } else {
        flatten(&format!("{fallback}: {preview}"))
    }
}

fn informative_tokens(preview: &str) -> usize {
    preview.split_whitespace().count()
}

/// The most readable single argument the vendor payload carries.
fn argument_preview(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        // A payload this resident cannot parse is still the vendor's own text;
        // its first line beats showing nothing at all.
        return raw.lines().next().unwrap_or_default().to_owned();
    };
    match value {
        Value::Object(fields) => object_preview(&fields),
        Value::Array(items) => joined(&items),
        Value::String(text) => text,
        Value::Null => "null".to_owned(),
        scalar => scalar.to_string(),
    }
}

fn object_preview(fields: &Map<String, Value>) -> String {
    // An argv array is the whole command, so it wins over any single field.
    if let Some(Value::Array(command)) = fields.get("command")
        && !command.is_empty()
    {
        return joined(command);
    }
    for key in ARGUMENT_KEYS {
        if let Some(Value::String(text)) = fields.get(key)
            && !text.trim().is_empty()
        {
            return text.clone();
        }
    }
    for value in fields.values() {
        if let Value::String(text) = value
            && !text.trim().is_empty()
        {
            return text.clone();
        }
    }
    for (key, value) in fields {
        let summary = match value {
            Value::Number(number) => format!("{key}={number}"),
            Value::Bool(boolean) => format!("{key}={boolean}"),
            Value::Array(items) => format!("{key}[{}]", items.len()),
            Value::Object(nested) => format!("{key}{{{}}}", nested.len()),
            Value::Null => format!("{key}=null"),
            Value::String(_) => continue,
        };
        return summary;
    }
    String::new()
}

fn joined(items: &[Value]) -> String {
    items
        .iter()
        .filter(|item| !item.is_null())
        .map(|item| match item {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapse a preview onto one bounded line.
pub(super) fn flatten(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        return flat;
    }
    let kept = flat.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
}
