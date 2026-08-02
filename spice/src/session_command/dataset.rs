use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use serde_json::Value;
use wire::Refusal;

use super::model::{
    CommandRecord, CompactionRecord, Dataset, MessageRecord, Residue, SourceView, TokenRecord,
    TurnRecord, Vendor,
};
use super::source::ResolvedSource;

const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const UNPARSEABLE_INTENT: &str = "unparseable compaction summary intent";
const HYPHEN_BYTE: u8 = 45;
const UNDERSCORE_BYTE: u8 = 95;

#[derive(Debug)]
struct RawEvent {
    source: String,
    line: u64,
    ts: String,
    vendor: Vendor,
    value: Value,
}

#[derive(Debug)]
struct Marker {
    source: String,
    ts: String,
    summary: String,
}

pub(crate) fn load(sources: &[ResolvedSource]) -> Result<Dataset, Refusal> {
    let mut dataset = Dataset::default();
    let mut events = Vec::new();
    for source in sources {
        let source_name = source.path.display().to_string();
        let (mut rows, mut residues) = read_events(source)?;
        let vendor = detected_vendor(source.vendor, &rows);
        for row in &mut rows {
            if row.vendor == Vendor::Unknown {
                row.vendor = vendor;
            }
        }
        dataset.sources.push(SourceView {
            name: source.path.file_name().map_or_else(
                || source_name.clone(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: source_name,
            vendor,
            thread: source.thread.clone(),
            compressed: source.path.to_string_lossy().ends_with(".gz"),
        });
        events.append(&mut rows);
        dataset.residues.append(&mut residues);
    }

    let mut markers = Vec::new();
    let source_order = dataset
        .sources
        .iter()
        .map(|source| (source.path.clone(), source.vendor))
        .collect::<Vec<_>>();
    for (source_path, source_vendor) in source_order {
        let mut source_events = events
            .iter()
            .filter(|event| event.source == source_path)
            .collect::<Vec<_>>();
        source_events.sort_by_key(|event| event.line);
        match source_vendor {
            Vendor::Claude => reduce_claude(&source_events, &mut dataset, &mut markers),
            Vendor::Codex | Vendor::Unknown => {
                reduce_codex(&source_events, &mut dataset, &mut markers)
            }
        }
    }
    reconstruct_compactions(&markers, &mut dataset);
    account_compactions(&mut dataset);
    sort_dataset(&mut dataset);
    Ok(dataset)
}

fn read_events(source: &ResolvedSource) -> Result<(Vec<RawEvent>, Vec<Residue>), Refusal> {
    let file = File::open(&source.path).map_err(|error| {
        refusal(
            "session::transcript-unreadable",
            Some(format!("{}: {error}", source.path.display())),
            "restore read access to the transcript and retry",
        )
    })?;
    let compressed = source.path.to_string_lossy().ends_with(".gz");
    let mut reader: Box<dyn BufRead> = if compressed {
        Box::new(BufReader::new(GzDecoder::new(file)))
    } else {
        Box::new(BufReader::new(file))
    };
    let mut rows = Vec::new();
    let mut residues = Vec::new();
    let mut buffer = Vec::new();
    let mut line = 0_u64;
    loop {
        buffer.clear();
        let read = match reader.read_until(b'\n', &mut buffer) {
            Ok(read) => read,
            Err(error) => {
                residues.push(read_residue(&source.path, line.saturating_add(1), &error));
                break;
            }
        };
        if read == 0 {
            break;
        }
        line = line.saturating_add(1);
        let terminated = buffer.last() == Some(&b'\n');
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        if buffer.len() > MAX_LINE_BYTES {
            residues.push(Residue {
                source: source.path.display().to_string(),
                line,
                kind: "oversized-line".to_owned(),
                detail: format!("JSONL record exceeds {MAX_LINE_BYTES} bytes"),
                recovery: "restore a bounded transcript record or select an earlier intact copy"
                    .to_owned(),
            });
            continue;
        }
        let value = match serde_json::from_slice::<Value>(&buffer) {
            Ok(value) => value,
            Err(error) => {
                residues.push(Residue {
                    source: source.path.display().to_string(),
                    line,
                    kind: if terminated {
                        "malformed-json"
                    } else {
                        "truncated-json"
                    }
                    .to_owned(),
                    detail: error.to_string(),
                    recovery: "retain the readable prefix and recover this line from the \
                               owning transcript archive"
                        .to_owned(),
                });
                continue;
            }
        };
        let Some(raw_ts) = value.get("timestamp").and_then(Value::as_str) else {
            residues.push(Residue {
                source: source.path.display().to_string(),
                line,
                kind: "timestamp-missing".to_owned(),
                detail: "record has no string timestamp".to_owned(),
                recovery: "recover the original timestamped transcript record".to_owned(),
            });
            continue;
        };
        rows.push(RawEvent {
            source: source.path.display().to_string(),
            line,
            ts: normalize_timestamp(raw_ts),
            vendor: source.vendor,
            value,
        });
    }
    Ok((rows, residues))
}

fn read_residue(path: &Path, line: u64, error: &io::Error) -> Residue {
    Residue {
        source: path.display().to_string(),
        line,
        kind: "read-interrupted".to_owned(),
        detail: error.to_string(),
        recovery: "retain the readable prefix and restore an uninterrupted transcript copy"
            .to_owned(),
    }
}

fn detected_vendor(hint: Vendor, rows: &[RawEvent]) -> Vendor {
    if hint != Vendor::Unknown {
        return hint;
    }
    rows.iter()
        .find_map(
            |event| match event.value.get("type").and_then(Value::as_str) {
                Some("user" | "assistant" | "system" | "summary") => Some(Vendor::Claude),
                Some("event_msg" | "response_item" | "compacted" | "session_meta") => {
                    Some(Vendor::Codex)
                }
                _ if event.value.get("payload").is_some() => Some(Vendor::Codex),
                _ => None,
            },
        )
        .unwrap_or(Vendor::Unknown)
}

fn reduce_codex(events: &[&RawEvent], dataset: &mut Dataset, markers: &mut Vec<Marker>) {
    let mut current = None;
    let mut pending_commands = BTreeMap::new();
    for event in events {
        let kind = event
            .value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let payload = event.value.get("payload").unwrap_or(&Value::Null);
        let payload_kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match (kind, payload_kind) {
            ("event_msg", "task_started") => {
                if let Some(index) = current {
                    finish_turn(dataset, index, &event.ts, false);
                }
                current = Some(start_turn(
                    dataset,
                    event,
                    payload.get("turn_id").and_then(Value::as_str),
                ));
            }
            ("event_msg", "task_complete") => {
                let index = ensure_turn(
                    dataset,
                    event,
                    &mut current,
                    payload.get("turn_id").and_then(Value::as_str),
                );
                if let Some(text) = payload.get("last_agent_message").and_then(Value::as_str) {
                    push_message(dataset, index, event, "assistant", "final_answer", text);
                }
                finish_turn(dataset, index, &event.ts, true);
                current = None;
            }
            ("event_msg", "exec_command_end") => {
                let turn_id = payload.get("turn_id").and_then(Value::as_str);
                let index = ensure_turn(dataset, event, &mut current, turn_id);
                push_completed_command(dataset, index, event, payload);
            }
            ("response_item", "message") => {
                let index = ensure_turn(dataset, event, &mut current, None);
                let role = payload
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("assistant");
                let phase = if role == "user" {
                    "prompt"
                } else {
                    payload
                        .get("phase")
                        .and_then(Value::as_str)
                        .unwrap_or("commentary")
                };
                for text in content_texts(payload.get("content")) {
                    push_message(dataset, index, event, role, phase, &text);
                }
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                let index = ensure_turn(
                    dataset,
                    event,
                    &mut current,
                    payload.get("turn_id").and_then(Value::as_str),
                );
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str);
                register_tool_call(
                    dataset,
                    index,
                    event,
                    payload,
                    call_id,
                    &mut pending_commands,
                );
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let call_id = payload.get("call_id").and_then(Value::as_str);
                complete_tool_call(dataset, current, event, payload, call_id, &pending_commands);
            }
            ("response_item", "token_count") => collect_codex_usage(dataset, event, payload),
            ("compacted", _) => markers.push(Marker {
                source: event.source.clone(),
                ts: event.ts.clone(),
                summary: String::new(),
            }),
            ("event_msg", "web_search") | ("response_item", "web_search_call") => {
                let index = ensure_turn(dataset, event, &mut current, None);
                dataset.turns[index].web_search_count =
                    dataset.turns[index].web_search_count.saturating_add(1);
                touch_turn(dataset, index, &event.ts);
            }
            _ => {}
        }
    }
}

fn reduce_claude(events: &[&RawEvent], dataset: &mut Dataset, markers: &mut Vec<Marker>) {
    let mut current = None;
    let mut active_prompt = None;
    let mut pending_commands = BTreeMap::new();
    for event in events {
        let kind = event
            .value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        match kind {
            "user" => {
                let prompt = event.value.get("promptId").and_then(Value::as_str);
                if prompt != active_prompt.as_deref() {
                    if let Some(index) = current {
                        finish_turn(dataset, index, &event.ts, false);
                    }
                    current = Some(start_turn(dataset, event, None));
                    active_prompt = prompt.map(str::to_owned);
                }
                let index = ensure_turn(dataset, event, &mut current, None);
                let content = event.value.pointer("/message/content");
                for text in content_texts(content) {
                    push_message(dataset, index, event, "user", "prompt", &text);
                }
                complete_claude_tool_results(dataset, index, event, content, &pending_commands);
            }
            "assistant" => {
                let index = ensure_turn(dataset, event, &mut current, None);
                let final_answer = event
                    .value
                    .pointer("/message/stop_reason")
                    .and_then(Value::as_str)
                    .is_some();
                let phase = if final_answer {
                    "final_answer"
                } else {
                    "commentary"
                };
                if let Some(content) = event
                    .value
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                {
                    for block in content {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    push_message(dataset, index, event, "assistant", phase, text);
                                }
                            }
                            Some("tool_use") => {
                                let call_id = block.get("id").and_then(Value::as_str);
                                register_tool_call(
                                    dataset,
                                    index,
                                    event,
                                    block,
                                    call_id,
                                    &mut pending_commands,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                collect_claude_usage(dataset, event);
                if final_answer {
                    finish_turn(dataset, index, &event.ts, true);
                    current = None;
                    active_prompt = None;
                }
            }
            "summary" => markers.push(Marker {
                source: event.source.clone(),
                ts: event.ts.clone(),
                summary: event
                    .value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            }),
            "system"
                if event.value.get("subtype").and_then(Value::as_str)
                    == Some("compact_boundary") =>
            {
                markers.push(Marker {
                    source: event.source.clone(),
                    ts: event.ts.clone(),
                    summary: String::new(),
                });
            }
            "rate_limit_event" | "error" => {
                let index = ensure_turn(dataset, event, &mut current, None);
                dataset.turns[index].error_count =
                    dataset.turns[index].error_count.saturating_add(1);
                touch_turn(dataset, index, &event.ts);
            }
            _ => {}
        }
    }
    if let Some(index) = current {
        let end = dataset.turns[index].last_activity_ts.clone();
        finish_turn(dataset, index, &end, false);
    }
}

fn start_turn(dataset: &mut Dataset, event: &RawEvent, turn_id: Option<&str>) -> usize {
    let index = dataset.turns.len();
    dataset.turns.push(TurnRecord {
        source: event.source.clone(),
        vendor: event.vendor,
        turn_id: turn_id.map(str::to_owned),
        start_ts: event.ts.clone(),
        end_ts: None,
        last_activity_ts: event.ts.clone(),
        completed: false,
        messages: Vec::new(),
        tool_calls: BTreeMap::new(),
        touched_files: Default::default(),
        command_count: 0,
        patch_count: 0,
        error_count: 0,
        web_search_count: 0,
        compaction_count: 0,
    });
    index
}

fn ensure_turn(
    dataset: &mut Dataset,
    event: &RawEvent,
    current: &mut Option<usize>,
    turn_id: Option<&str>,
) -> usize {
    if let Some(turn_id) = turn_id
        && let Some(index) = dataset.turns.iter().rposition(|turn| {
            turn.source == event.source && turn.turn_id.as_deref() == Some(turn_id)
        })
    {
        *current = Some(index);
        return index;
    }
    if let Some(index) = *current {
        return index;
    }
    let index = start_turn(dataset, event, turn_id);
    *current = Some(index);
    index
}

fn finish_turn(dataset: &mut Dataset, index: usize, ts: &str, completed: bool) {
    let turn = &mut dataset.turns[index];
    turn.end_ts = Some(ts.to_owned());
    turn.last_activity_ts = ts.to_owned();
    turn.completed = completed;
}

fn touch_turn(dataset: &mut Dataset, index: usize, ts: &str) {
    dataset.turns[index].last_activity_ts = ts.to_owned();
}

fn push_message(
    dataset: &mut Dataset,
    index: usize,
    event: &RawEvent,
    side: &str,
    phase: &str,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    let (primary_flavor, flavor_tags, matched_cues, human) = classify_message(side, phase, text);
    let record = MessageRecord {
        source: event.source.clone(),
        ts: event.ts.clone(),
        turn_id: dataset.turns[index].turn_id.clone(),
        side: side.to_owned(),
        phase: phase.to_owned(),
        text: text.to_owned(),
        primary_flavor,
        flavor_tags,
        matched_cues,
        human,
    };
    if dataset.turns[index].messages.iter().any(|row| {
        row.ts == record.ts
            && row.side == record.side
            && row.phase == record.phase
            && row.text == record.text
    }) {
        return;
    }
    dataset.turns[index].messages.push(record.clone());
    dataset.messages.push(record);
    touch_turn(dataset, index, &event.ts);
}

fn classify_message(
    side: &str,
    phase: &str,
    text: &str,
) -> (String, Vec<String>, Vec<String>, bool) {
    let lower = text.to_lowercase();
    let stripped = lower.trim_start();
    let mut tags = Vec::new();
    let mut cues = Vec::new();
    if side == "assistant" {
        let flavor = if phase == "final_answer" {
            "final_answer"
        } else {
            "commentary"
        };
        tags.push(flavor.to_owned());
        for cue in question_cues(&lower, stripped) {
            if !tags.iter().any(|tag| tag == "question_like") {
                tags.push("question_like".to_owned());
            }
            cues.push(cue);
        }
        if text.trim_start().starts_with("ACK ") {
            tags.push("steering_ack".to_owned());
        } else if text.trim_start().starts_with("NACK ") {
            tags.push("steering_nack".to_owned());
        }
        return (flavor.to_owned(), dedupe(tags), dedupe(cues), false);
    }

    let question = question_cues(&lower, stripped);
    if !question.is_empty() {
        tags.push("question_like".to_owned());
        cues.extend(question);
    }
    let directive = pattern_cues(
        &lower,
        &[
            "need to",
            "i want",
            "please",
            "let's",
            "let us",
            "help me",
            "show me",
            "give me",
            "implement",
            "fix",
            "remove",
            "change",
            "add",
            "build",
            "make sure",
        ],
    );
    if !directive.is_empty() {
        tags.push("directive_like".to_owned());
        cues.extend(directive);
    }
    let constraints = pattern_cues(
        &lower,
        &[
            "do not",
            "don't",
            "must",
            "only",
            "without",
            "never",
            "important",
        ],
    );
    if !constraints.is_empty() {
        tags.push("constraint_like".to_owned());
        cues.extend(constraints);
    }
    if tags.is_empty() {
        tags.push("plain".to_owned());
    }
    let primary = [
        "constraint_like",
        "directive_like",
        "question_like",
        "plain",
    ]
    .into_iter()
    .find(|candidate| tags.iter().any(|tag| tag == candidate))
    .unwrap_or("plain")
    .to_owned();
    (
        primary,
        dedupe(tags),
        dedupe(cues),
        human_user_message(text),
    )
}

fn question_cues(lower: &str, stripped: &str) -> Vec<String> {
    let mut cues = Vec::new();
    if lower.contains('?') {
        cues.push("?".to_owned());
    }
    cues.extend(
        [
            "what", "why", "how", "can", "is", "are", "should", "could", "would",
        ]
        .into_iter()
        .filter(|word| stripped.starts_with(word))
        .map(str::to_owned),
    );
    cues
}

fn pattern_cues(lower: &str, patterns: &[&str]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| lower.contains(**pattern))
        .map(|pattern| (*pattern).to_owned())
        .collect()
}

fn human_user_message(text: &str) -> bool {
    let trimmed = text.trim_start();
    ![
        "The linked skill below carries the full authority",
        "[$spice](",
        "This session is being continued from a previous conversation",
        "<task-notification>",
        "<user_instructions>",
        "<environment_context>",
        "<ENVIRONMENT",
        "Your tool call was malformed",
        "[Your previous response had no visible output",
    ]
    .into_iter()
    .any(|prefix| trimmed.starts_with(prefix))
        && !complete_tag_envelope(trimmed)
}

fn complete_tag_envelope(text: &str) -> bool {
    if !text.starts_with('<') {
        return false;
    }
    let open_end = match text.find('>') {
        Some(index) => index,
        None => return false,
    };
    let opening = &text[1..open_end];
    let tag = match opening.split_whitespace().next() {
        Some(candidate) => candidate.trim_end_matches('/'),
        None => return false,
    };
    if tag.is_empty() {
        return false;
    }
    for byte in tag.bytes() {
        if !(byte.is_ascii_alphanumeric() || byte == UNDERSCORE_BYTE || byte == HYPHEN_BYTE) {
            return false;
        }
    }
    text.trim_end().ends_with(&format!("</{tag}>"))
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut rows = Vec::new();
    for value in values {
        if !rows.contains(&value) {
            rows.push(value);
        }
    }
    rows
}

fn content_texts(content: Option<&Value>) -> Vec<String> {
    match content {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| match value.get("type").and_then(Value::as_str) {
                Some("text" | "input_text" | "output_text") | None => value
                    .get("text")
                    .or_else(|| value.get("content").filter(|row| row.is_string()))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn register_tool_call(
    dataset: &mut Dataset,
    index: usize,
    event: &RawEvent,
    payload: &Value,
    call_id: Option<&str>,
    pending_commands: &mut BTreeMap<String, usize>,
) {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    *dataset.turns[index]
        .tool_calls
        .entry(name.to_owned())
        .or_default() += 1;
    if matches!(name, "apply_patch" | "Edit" | "Write" | "MultiEdit") {
        dataset.turns[index].patch_count = dataset.turns[index].patch_count.saturating_add(1);
        for path in tool_paths(payload) {
            dataset.turns[index].touched_files.insert(path);
        }
    }
    if matches!(name, "web_search" | "web.run" | "WebSearch") {
        dataset.turns[index].web_search_count =
            dataset.turns[index].web_search_count.saturating_add(1);
    }
    if matches!(
        name,
        "exec_command" | "Bash" | "shell" | "local_shell" | "container.exec"
    ) {
        let (command, cwd) = tool_command(payload);
        let command_index = dataset.commands.len();
        dataset.commands.push(CommandRecord {
            source: event.source.clone(),
            ts: event.ts.clone(),
            turn_id: dataset.turns[index].turn_id.clone(),
            cwd,
            command,
            exit_code: None,
            status: "called".to_owned(),
        });
        dataset.turns[index].command_count = dataset.turns[index].command_count.saturating_add(1);
        if let Some(call_id) = call_id {
            pending_commands.insert(call_id.to_owned(), command_index);
        }
    }
    touch_turn(dataset, index, &event.ts);
}

fn tool_command(payload: &Value) -> (String, Option<String>) {
    let arguments = payload.get("arguments").and_then(Value::as_str);
    let parsed = arguments
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .or_else(|| payload.get("input").cloned())
        .unwrap_or(Value::Null);
    let command = parsed
        .get("cmd")
        .or_else(|| parsed.get("command"))
        .map(render_command)
        .unwrap_or_else(|| "-".to_owned());
    let cwd = parsed
        .get("workdir")
        .or_else(|| parsed.get("cwd"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    (command, cwd)
}

fn render_command(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => value.to_string(),
    }
}

fn tool_paths(payload: &Value) -> Vec<String> {
    let arguments = payload
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .or_else(|| payload.get("input").cloned())
        .unwrap_or(Value::Null);
    let mut paths = ["file_path", "path"]
        .into_iter()
        .filter_map(|key| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    if let Some(patch) = arguments.get("input").and_then(Value::as_str) {
        for line in patch.lines() {
            if let Some(path) = line
                .strip_prefix("*** Update File: ")
                .or_else(|| line.strip_prefix("*** Add File: "))
                .or_else(|| line.strip_prefix("*** Delete File: "))
            {
                paths.push(path.to_owned());
            }
        }
    }
    dedupe(paths)
}

fn complete_tool_call(
    dataset: &mut Dataset,
    current: Option<usize>,
    event: &RawEvent,
    payload: &Value,
    call_id: Option<&str>,
    pending_commands: &BTreeMap<String, usize>,
) {
    let Some(command_index) = call_id.and_then(|id| pending_commands.get(id)).copied() else {
        return;
    };
    let output = output_text(payload.get("output"));
    let (status, exit_code) = output_status(&output);
    dataset.commands[command_index].status = status;
    dataset.commands[command_index].exit_code = exit_code;
    if let Some(index) = current {
        if exit_code.is_some_and(|code| code != 0) {
            dataset.turns[index].error_count = dataset.turns[index].error_count.saturating_add(1);
        }
        touch_turn(dataset, index, &event.ts);
    }
}

fn complete_claude_tool_results(
    dataset: &mut Dataset,
    index: usize,
    event: &RawEvent,
    content: Option<&Value>,
    pending_commands: &BTreeMap<String, usize>,
) {
    let Some(values) = content.and_then(Value::as_array) else {
        return;
    };
    for block in values {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let call_id = block.get("tool_use_id").and_then(Value::as_str);
        complete_tool_call(
            dataset,
            Some(index),
            event,
            block,
            call_id,
            pending_commands,
        );
    }
}

fn output_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| {
                value
                    .get("text")
                    .or_else(|| value.get("content"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

fn output_status(output: &str) -> (String, Option<i32>) {
    if output.contains("Process running with session ID")
        || output.contains("Script running with cell ID")
    {
        return ("running".to_owned(), None);
    }
    for prefix in [
        "Process exited with code ",
        "exit_code\":",
        "\"exit_code\":",
    ] {
        if let Some(rest) = output.split(prefix).nth(1) {
            let digits = rest
                .trim_start()
                .chars()
                .take_while(|character| character.is_ascii_digit() || *character == '-')
                .collect::<String>();
            if let Ok(code) = digits.parse() {
                return ("completed".to_owned(), Some(code));
            }
        }
    }
    ("called".to_owned(), None)
}

fn push_completed_command(dataset: &mut Dataset, index: usize, event: &RawEvent, payload: &Value) {
    let command = payload
        .get("command")
        .map(render_command)
        .unwrap_or_else(|| "-".to_owned());
    let exit_code = payload.get("exit_code").and_then(|value| {
        value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    });
    dataset.commands.push(CommandRecord {
        source: event.source.clone(),
        ts: event.ts.clone(),
        turn_id: payload
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| dataset.turns[index].turn_id.clone()),
        cwd: payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_owned),
        command,
        exit_code,
        status: payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_owned(),
    });
    dataset.turns[index].command_count = dataset.turns[index].command_count.saturating_add(1);
    if exit_code.is_some_and(|code| code != 0) {
        dataset.turns[index].error_count = dataset.turns[index].error_count.saturating_add(1);
    }
    touch_turn(dataset, index, &event.ts);
}

fn collect_codex_usage(dataset: &mut Dataset, event: &RawEvent, payload: &Value) {
    let usage = payload
        .pointer("/info/last_token_usage")
        .unwrap_or(&Value::Null);
    let record = token_record(&event.source, &event.ts, usage);
    if record.total > 0 || record.input > 0 || record.output > 0 {
        dataset.tokens.push(record);
    }
}

fn collect_claude_usage(dataset: &mut Dataset, event: &RawEvent) {
    let usage = event
        .value
        .pointer("/message/usage")
        .unwrap_or(&Value::Null);
    let record = token_record(&event.source, &event.ts, usage);
    if record.total > 0 || record.input > 0 || record.output > 0 {
        dataset.tokens.push(record);
    }
}

fn token_record(source: &str, ts: &str, usage: &Value) -> TokenRecord {
    let input = integer(usage, &["input_tokens"]);
    let cached = integer(usage, &["cached_input_tokens", "cache_read_input_tokens"]);
    let output = integer(usage, &["output_tokens"]);
    let reasoning = integer(usage, &["reasoning_output_tokens"]);
    let total = integer(usage, &["total_tokens"])
        .max(input.saturating_add(output).saturating_add(reasoning));
    TokenRecord {
        source: source.to_owned(),
        ts: ts.to_owned(),
        input,
        cached,
        output,
        reasoning,
        total,
    }
}

fn integer(value: &Value, names: &[&str]) -> u64 {
    names
        .iter()
        .find_map(|name| value.get(name).and_then(Value::as_u64))
        .unwrap_or(0)
}

fn reconstruct_compactions(markers: &[Marker], dataset: &mut Dataset) {
    let mut ordered = markers.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|marker| (&marker.source, &marker.ts));
    for (index, marker) in ordered.iter().enumerate() {
        let next = ordered
            .iter()
            .skip(index + 1)
            .find(|candidate| candidate.source == marker.source)
            .map(|candidate| candidate.ts.as_str());
        let before = dataset
            .messages
            .iter()
            .filter(|message| {
                message.source == marker.source
                    && message.side == "assistant"
                    && message.ts < marker.ts
            })
            .max_by_key(|message| &message.ts)
            .map_or_else(String::new, |message| message.text.clone());
        let after = |message: &&MessageRecord| {
            message.source == marker.source
                && message.ts > marker.ts
                && next.is_none_or(|end| message.ts.as_str() < end)
        };
        let summary = if marker.summary.is_empty() {
            dataset
                .messages
                .iter()
                .filter(after)
                .find(|message| {
                    message
                        .text
                        .trim_start()
                        .starts_with("This session is being continued from a previous conversation")
                })
                .map_or_else(String::new, |message| message.text.clone())
        } else {
            marker.summary.clone()
        };
        let user_after = dataset
            .messages
            .iter()
            .filter(after)
            .find(|message| message.side == "user" && message.human)
            .map_or_else(String::new, |message| message.text.clone());
        dataset.compactions.push(CompactionRecord {
            source: marker.source.clone(),
            ts: marker.ts.clone(),
            assistant_before: before,
            intent: compaction_intent(&summary),
            summary_after: summary,
            user_after,
        });
    }
}

fn compaction_intent(summary: &str) -> String {
    if summary.is_empty() {
        return String::new();
    }
    let lines = summary.lines().collect::<Vec<_>>();
    let Some(start) = lines
        .iter()
        .position(|line| line.contains("Primary Request and Intent"))
    else {
        return UNPARSEABLE_INTENT.to_owned();
    };
    let selected = lines
        .iter()
        .skip(start + 1)
        .take_while(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with(|character: char| character.is_ascii_digit())
                && trimmed.contains(". "))
        })
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if selected.is_empty() {
        UNPARSEABLE_INTENT.to_owned()
    } else {
        selected
    }
}

fn account_compactions(dataset: &mut Dataset) {
    for turn in &mut dataset.turns {
        let end = turn.end_ts.as_deref().unwrap_or(&turn.last_activity_ts);
        turn.compaction_count = dataset
            .compactions
            .iter()
            .filter(|row| {
                row.source == turn.source && row.ts >= turn.start_ts && row.ts.as_str() <= end
            })
            .count() as u64;
    }
}

fn sort_dataset(dataset: &mut Dataset) {
    dataset.turns.sort_by(|left, right| {
        (&left.start_ts, &left.source).cmp(&(&right.start_ts, &right.source))
    });
    dataset
        .compactions
        .sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    dataset.messages.sort_by(|left, right| {
        (&left.ts, &left.source, &left.side).cmp(&(&right.ts, &right.source, &right.side))
    });
    dataset
        .commands
        .sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    dataset
        .tokens
        .sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
}

fn normalize_timestamp(raw: &str) -> String {
    raw.parse::<jiff::Timestamp>()
        .ok()
        .and_then(|timestamp| wire::Instant::new(timestamp).ok())
        .map_or_else(|| raw.to_owned(), |timestamp| timestamp.to_string())
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Refusal {
    Refusal {
        reason: reason.to_owned(),
        witness: Some("session-transcript".to_owned()),
        expected: Some("readable bounded JSONL transcript evidence".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    }
}
