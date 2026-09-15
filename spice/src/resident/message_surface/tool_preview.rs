//! Operator-facing preview text for one tool moment.
//!
//! The lane status strip is read by a person, so a tool moment renders as the
//! work being done rather than as vendor bookkeeping: generic command-execution
//! calls suppress their taxonomy only when the command is already informative,
//! named shells retain their family, and results show the call they answer
//! instead of the `tool_use` id that joins them. The rules here follow the
//! predecessor at `spice-z spice/serve/messagepresentation.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde_json::{Map, Value};
use wire::CanonicalUuid;

#[cfg(test)]
mod tests;

/// Longest preview one strip line carries before it is elided.
pub(in crate::resident) const MAX_CHARS: usize = 120;
const MIN_INFORMATIVE_TOKENS: usize = 2;
const MAX_EVENT_IDENTITY_CHARS: usize = 18;
const MAX_FILE_CHANGE_KIND_CHARS: usize = 12;
const MAX_FILE_CHANGE_KINDS: usize = 2;
const TOOL_OUTCOME_RESERVE: usize = " succeeded".len();

/// Argument fields worth showing, most recognizable first.
///
/// `command` leads the list because a shell call is the moment this strip
/// exists for. The predecessor reaches the same field through its trailing
/// insertion-order scan, which a sorted JSON object cannot reproduce, so the
/// field it depends on is named here instead of being arrived at by luck.
const ARGUMENT_KEYS: [&str; 8] = [
    "command", "cmd", "path", "query", "url", "input", "prompt", "text",
];

const GENERIC_COMMAND_EXECUTION_TOOLS: [&str; 3] = ["exec", "exec_command", "command_execution"];
const STRUCTURED_COMMAND_EXECUTION_TOOLS: [&str; 2] = ["exec", "exec_command"];

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
#[cfg(test)]
pub(super) fn activity(
    lane: CanonicalUuid,
    tool: &listen::Tool,
    previews: &mut CallPreviews,
) -> String {
    activity_in(lane, tool, previews, None)
}

pub(super) fn activity_in(
    lane: CanonicalUuid,
    tool: &listen::Tool,
    previews: &mut CallPreviews,
    worktree: Option<&Path>,
) -> String {
    let key = (lane, tool.call_id.clone());
    match &tool.moment {
        listen::ToolMoment::Call { name, arguments } => {
            let preview = identified_preview(name, arguments, &tool.call_id, worktree);
            previews.by_call.insert(key, preview.clone());
            preview
        }
        listen::ToolMoment::Update { name, arguments } => {
            let preview = identified_preview(name, arguments, &tool.call_id, worktree);
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

fn identified_preview(
    name: &str,
    arguments: &str,
    call_id: &str,
    worktree: Option<&Path>,
) -> String {
    let normalized = normalized_name(name);
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("tool") {
        return named_preview(
            &event_identity(call_id),
            &flatten(&argument_preview(arguments)),
            "activity",
        );
    }
    activity_preview_in(name, arguments, worktree)
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

#[cfg(test)]
fn activity_preview(name: &str, arguments: &str) -> String {
    activity_preview_in(name, arguments, None)
}

fn activity_preview_in(name: &str, arguments: &str, worktree: Option<&Path>) -> String {
    let name = name.trim();
    if name == "file_change"
        && let Some(arguments) = file_change_preview(arguments, worktree)
    {
        return named_preview(&normalized_name(name), &arguments, "tool");
    }
    if GENERIC_COMMAND_EXECUTION_TOOLS.contains(&name) {
        if STRUCTURED_COMMAND_EXECUTION_TOOLS.contains(&name)
            && !arguments.is_empty()
            && serde_json::from_str::<Value>(arguments).is_err()
        {
            return complete_preview(&normalized_name(name), "command");
        }
        let arguments = flatten(&argument_preview(arguments));
        return if informative_tokens(&arguments) >= MIN_INFORMATIVE_TOKENS {
            arguments
        } else {
            named_preview(&normalized_name(name), &arguments, "command")
        };
    }
    ordinary_call_preview(name, arguments)
}

pub(super) fn call_preview(name: &str, arguments: &str) -> String {
    ordinary_call_preview(name, arguments)
}

fn file_change_preview(raw: &str, worktree: Option<&Path>) -> Option<String> {
    let Value::Object(arguments) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    let changes = arguments.get("changes")?.as_array()?;
    if changes.is_empty() {
        return None;
    }
    let mut paths = Vec::with_capacity(changes.len());
    let mut kinds = BTreeSet::<String>::new();
    for change in changes {
        let change = change.as_object()?;
        let path = relative_change_path(change.get("path")?.as_str()?, worktree)?;
        paths.push(path);
        if let Some(kind) = change
            .get("kind")
            .and_then(Value::as_str)
            .and_then(change_kind)
        {
            kinds.insert(kind);
        }
    }
    paths.sort();
    let suffix = if paths.len() == 1 {
        kinds
            .iter()
            .next()
            .map_or_else(String::new, |kind| format!(" [{kind}]"))
    } else {
        format!(
            " +{} paths{}",
            paths.len() - 1,
            file_change_kind_summary(&kinds)
        )
    };
    let prefix_chars = "file_change: ".chars().count();
    let suffix_chars = suffix.chars().count();
    let path_budget = MAX_CHARS
        .saturating_sub(TOOL_OUTCOME_RESERVE)
        .saturating_sub(prefix_chars)
        .saturating_sub(suffix_chars);
    Some(format!("{}{}", elide(&paths[0], path_budget), suffix))
}

fn relative_change_path(raw: &str, worktree: Option<&Path>) -> Option<String> {
    let path = Path::new(raw);
    let relative = if path.is_absolute() {
        path.strip_prefix(worktree?).ok()?
    } else {
        path
    };
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(component) => components.push(component.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

fn change_kind(kind: &str) -> Option<String> {
    let kind = kind.trim();
    (!kind.is_empty()
        && kind.len() <= MAX_FILE_CHANGE_KIND_CHARS
        && kind
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')))
    .then(|| kind.to_owned())
}

fn file_change_kind_summary(kinds: &BTreeSet<String>) -> String {
    if kinds.is_empty() {
        return String::new();
    }
    let shown = kinds
        .iter()
        .take(MAX_FILE_CHANGE_KINDS)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let remaining = kinds.len().saturating_sub(shown.len());
    let remaining = if remaining > 0 {
        format!(" +{remaining} kinds")
    } else {
        String::new()
    };
    format!(" [{}{}]", shown.join(","), remaining)
}

fn elide(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_owned();
    }
    if budget == 0 {
        return String::new();
    }
    let kept = text.chars().take(budget - 1).collect::<String>();
    format!("{}…", kept.trim_end())
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
