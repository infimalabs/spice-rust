//! Operator-facing preview text for one tool moment.
//!
//! The lane status strip is read by a person, so a tool moment renders as the
//! work being done rather than as vendor bookkeeping: a call shows its name and
//! enough of its arguments to recognize the command, and a result shows the
//! call it answers instead of the `tool_use` id that joins them. The rules here
//! follow the predecessor at `spice-z spice/serve/messagepresentation.py`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use wire::CanonicalUuid;

#[cfg(test)]
mod tests;

/// Longest preview one strip line carries before it is elided.
const MAX_CHARS: usize = 120;

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
            let preview = call_preview(name, arguments);
            previews.by_call.insert(key, preview.clone());
            preview
        }
        listen::ToolMoment::Update { name, arguments } => {
            let preview = call_preview(name, arguments);
            previews.by_call.insert(key, preview.clone());
            flatten(&format!("{preview} update"))
        }
        listen::ToolMoment::Result(result) => {
            let outcome = match result.result {
                listen::ToolResult::Succeeded => "succeeded",
                listen::ToolResult::Failed => "failed",
                listen::ToolResult::Unreported => "completed",
            };
            // An unmatched result still must not paint `toolu_01...`: the id is
            // vendor bookkeeping an operator cannot act on. A call whose record
            // was never selected, or one outstanding across a resident restart,
            // reads as the anonymous tool it is.
            previews.by_call.remove(&key).map_or_else(
                || format!("tool {outcome}"),
                |preview| flatten(&format!("{preview} {outcome}")),
            )
        }
    }
}

pub(super) fn call_preview(name: &str, arguments: &str) -> String {
    let name = name.trim().replace('_', " ");
    let arguments = argument_preview(arguments);
    if name.is_empty() {
        return flatten(&arguments);
    }
    if arguments.is_empty() {
        return flatten(&name);
    }
    flatten(&format!("{name}: {arguments}"))
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
        _ => String::new(),
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
fn flatten(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        return flat;
    }
    let kept = flat.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
}
