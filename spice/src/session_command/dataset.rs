use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use listen::{
    Decoded, Dialect, Event, ProseRole, RecordContext, RecordDecodeError, ResidueKind,
    SourcedEvent, Span, ToolMoment, ToolOutcome, ToolResult, TurnMoment,
};
use serde_json::Value;
use spice::session_meter::ContextSnapshot;
use wire::Refusal;

use crate::message_shape::{
    MessageShape, UNRECOVERABLE_REHYDRATION_INTENT, classify, rehydration_intent,
};

use super::model::{
    CommandRecord, CompactionRecord, Dataset, MessageRecord, Residue, SourceView, TokenRecord,
    TurnRecord, Vendor,
};
use super::source::ResolvedSource;

mod horizon;

use horizon::read_records;
pub(crate) use horizon::recent_compaction_horizon;

const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
struct TypedRecord {
    source: String,
    line: u64,
    vendor: Vendor,
    context: RecordContext,
    events: Vec<SourcedEvent>,
}

#[derive(Debug)]
struct Marker {
    source: String,
    ts: String,
    summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionHorizon {
    pub(crate) start: Option<String>,
}

#[derive(Debug)]
struct PendingTool {
    turn: usize,
    command: Option<usize>,
}

enum DecodedRow {
    Typed(TypedRecord),
    Residue(Residue),
    Ignored,
}

pub(crate) fn load(sources: &[ResolvedSource]) -> Result<Dataset, Refusal> {
    load_since(sources, None)
}

pub(crate) fn load_since(
    sources: &[ResolvedSource],
    start: Option<&str>,
) -> Result<Dataset, Refusal> {
    let mut dataset = Dataset::default();
    let mut markers = Vec::new();
    for source in sources {
        let source_name = source.path.display().to_string();
        let (records, mut residues, vendor) = read_records(source, start)?;
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
        reduce(&records, &mut dataset, &mut markers)?;
        dataset.residues.append(&mut residues);
    }
    reconstruct_compactions(&markers, &mut dataset);
    account_compactions(&mut dataset);
    sort_dataset(&mut dataset);
    Ok(dataset)
}

fn decode_row(
    source: &ResolvedSource,
    line: u64,
    buffer: &[u8],
    dialect: Option<Dialect>,
) -> Result<(Dialect, DecodedRow), Refusal> {
    if buffer.len() > MAX_LINE_BYTES {
        return Err(refusal(
            "session::transcript-record-oversized",
            Some(format!(
                "{}:{line} exceeds {MAX_LINE_BYTES} bytes",
                source.path.display()
            )),
            "restore a bounded transcript record or select an earlier intact copy",
        ));
    }
    let locus = listen::Locus::new(line, 0, buffer.len() as u64);
    let detected = listen::decode_record(dialect, buffer, locus)
        .map_err(|error| record_refusal(&source.path, line, &error))?;
    let observed = detected.dialect;
    let row = match detected.decoded {
        Decoded::Residue(residue) => decode_residue(source, line, residue.kind)?,
        Decoded::Events { context, events } => {
            typed_row(source, line, buffer, observed, context, events)?
        }
    };
    Ok((observed, row))
}

fn decode_residue(
    source: &ResolvedSource,
    line: u64,
    kind: ResidueKind,
) -> Result<DecodedRow, Refusal> {
    match kind {
        ResidueKind::Unknown { discriminator } => Ok(DecodedRow::Residue(Residue {
            source: source.path.display().to_string(),
            line,
            kind: "unrecognized-record".to_owned(),
            detail: discriminator,
            recovery:
                "upgrade the owning transcript decoder or select records from its current release"
                    .to_owned(),
        })),
        kind => Err(refusal(
            "session::transcript-evidence-invalid",
            Some(format!("{}:{line}: {kind:?}", source.path.display())),
            "restore the current vendor record or use the decoder from its owning release",
        )),
    }
}

fn typed_row(
    source: &ResolvedSource,
    line: u64,
    buffer: &[u8],
    dialect: Dialect,
    mut context: RecordContext,
    events: Vec<Event>,
) -> Result<DecodedRow, Refusal> {
    if events.is_empty() && context.turn.is_none() && context.timestamp.is_none() {
        return Ok(DecodedRow::Ignored);
    }
    let raw_timestamp = context.timestamp.as_deref().ok_or_else(|| {
        refusal(
            "session::transcript-timestamp-missing",
            Some(format!("{}:{line}", source.path.display())),
            "recover the original timestamped transcript record",
        )
    })?;
    let timestamp = spice::session_meter::normalize_timestamp(raw_timestamp).ok_or_else(|| {
        refusal(
            "session::transcript-timestamp-invalid",
            Some(format!("{}:{line}: {raw_timestamp}", source.path.display())),
            "recover the original timestamp; no substitute ordering key is used",
        )
    })?;
    context.timestamp = Some(timestamp.to_string());
    let span = Span {
        member: line,
        offset: 0,
        length: buffer.len() as u64,
    };
    let sourced = events
        .into_iter()
        .enumerate()
        .map(|(ordinal, event)| SourcedEvent {
            ordinal: ordinal as u64,
            spans: vec![span],
            context: context.clone(),
            event,
        })
        .collect();
    Ok(DecodedRow::Typed(TypedRecord {
        source: source.path.display().to_string(),
        line,
        vendor: if source.vendor.is_unknown() {
            Vendor::inferred(dialect)
        } else {
            source.vendor
        },
        context,
        events: sourced,
    }))
}

fn record_refusal(path: &Path, line: u64, error: &RecordDecodeError) -> Refusal {
    let reason = match error {
        RecordDecodeError::Disagreement { .. } => "session::transcript-dialect-disagreement",
        RecordDecodeError::Invalid { .. } => "session::transcript-evidence-invalid",
        RecordDecodeError::Unowned => "session::transcript-dialect-unowned",
        RecordDecodeError::Locus(_) => "session::transcript-locus-invalid",
    };
    refusal(
        reason,
        Some(format!("{}:{line}: {error}", path.display())),
        "select the transcript's owning vendor release and retry without a fallback decoder",
    )
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

fn reduce(
    records: &[TypedRecord],
    dataset: &mut Dataset,
    markers: &mut Vec<Marker>,
) -> Result<(), Refusal> {
    let mut current = None;
    let mut pending = BTreeMap::new();
    for record in records {
        let timestamp = record_timestamp(record);
        match record.context.turn {
            Some(TurnMoment::Started) => {
                if let Some(index) = current {
                    finish_turn(dataset, index, timestamp, false);
                }
                current = Some(start_turn(
                    dataset,
                    record,
                    record.context.turn_id.as_deref(),
                ));
            }
            Some(TurnMoment::Observed) => {
                current = Some(observe_turn(dataset, record, current));
            }
            Some(TurnMoment::Completed) | None => {}
        }

        for sourced in &record.events {
            match &sourced.event {
                Event::Session(_) => {}
                Event::Prose(prose) => {
                    let index = ensure_turn(dataset, record, &mut current);
                    let side = match prose.role {
                        ProseRole::Operator => "user",
                        ProseRole::Assistant => "assistant",
                    };
                    let phase = if prose.role == ProseRole::Operator {
                        "prompt"
                    } else if prose.phase.as_deref() == Some("final_answer")
                        || record.context.turn == Some(TurnMoment::Completed)
                    {
                        "final_answer"
                    } else {
                        prose.phase.as_deref().unwrap_or("commentary")
                    };
                    if !(phase == "final_answer" && last_final_matches(dataset, index, &prose.text))
                    {
                        push_message(dataset, index, record, side, phase, &prose.text);
                    }
                }
                Event::Reasoning(_) => {
                    let index = ensure_turn(dataset, record, &mut current);
                    touch_turn(dataset, index, timestamp);
                }
                Event::Tool(tool) => match &tool.moment {
                    ToolMoment::Call { name, arguments } => {
                        let index = ensure_turn(dataset, record, &mut current);
                        register_tool_call(
                            dataset,
                            index,
                            record,
                            &tool.call_id,
                            name,
                            arguments,
                            &mut pending,
                        )?;
                    }
                    ToolMoment::Update { .. } => {
                        if let Some(tool) = pending.get(&tool.call_id) {
                            touch_turn(dataset, tool.turn, timestamp);
                        }
                    }
                    ToolMoment::Result(outcome) => {
                        complete_tool_call(dataset, record, &tool.call_id, outcome, &mut pending);
                    }
                },
                Event::Usage(usage) => push_usage(dataset, record, usage),
                Event::Compaction(compaction) => markers.push(Marker {
                    source: record.source.clone(),
                    ts: timestamp.to_owned(),
                    summary: compaction.summary.clone().unwrap_or_default(),
                }),
                Event::Failure(_) => {
                    let index = ensure_turn(dataset, record, &mut current);
                    dataset.turns[index].error_count =
                        dataset.turns[index].error_count.saturating_add(1);
                    touch_turn(dataset, index, timestamp);
                }
            }
        }

        if record.context.turn == Some(TurnMoment::Completed) {
            let index = ensure_turn(dataset, record, &mut current);
            finish_turn(dataset, index, timestamp, true);
            current = None;
        }
    }
    if let Some(index) = current {
        let end = dataset.turns[index].last_activity_ts.clone();
        finish_turn(dataset, index, &end, false);
    }
    Ok(())
}

fn record_timestamp(record: &TypedRecord) -> &str {
    record
        .context
        .timestamp
        .as_deref()
        .expect("typed session records require normalized timestamps")
}

fn observe_turn(dataset: &mut Dataset, record: &TypedRecord, current: Option<usize>) -> usize {
    if let Some(turn_id) = record.context.turn_id.as_deref() {
        if let Some(index) = current
            && dataset.turns[index].turn_id.as_deref() == Some(turn_id)
        {
            return index;
        }
        if let Some(index) = current {
            finish_turn(dataset, index, record_timestamp(record), false);
        }
        if let Some(index) = dataset.turns.iter().rposition(|turn| {
            turn.source == record.source
                && turn.turn_id.as_deref() == Some(turn_id)
                && !turn.completed
        }) {
            return index;
        }
        return start_turn(dataset, record, Some(turn_id));
    }
    current.unwrap_or_else(|| start_turn(dataset, record, None))
}

fn start_turn(dataset: &mut Dataset, record: &TypedRecord, turn_id: Option<&str>) -> usize {
    let timestamp = record_timestamp(record);
    let index = dataset.turns.len();
    dataset.turns.push(TurnRecord {
        source: record.source.clone(),
        vendor: record.vendor,
        turn_id: turn_id.map(str::to_owned),
        start_ts: timestamp.to_owned(),
        end_ts: None,
        last_activity_ts: timestamp.to_owned(),
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

fn ensure_turn(dataset: &mut Dataset, record: &TypedRecord, current: &mut Option<usize>) -> usize {
    if let Some(turn_id) = record.context.turn_id.as_deref()
        && let Some(index) = dataset.turns.iter().rposition(|turn| {
            turn.source == record.source
                && turn.turn_id.as_deref() == Some(turn_id)
                && !turn.completed
        })
    {
        *current = Some(index);
        return index;
    }
    if let Some(index) = *current {
        return index;
    }
    let index = start_turn(dataset, record, record.context.turn_id.as_deref());
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

fn last_final_matches(dataset: &Dataset, index: usize, text: &str) -> bool {
    dataset.turns[index]
        .messages
        .iter()
        .rev()
        .find(|message| message.side == "assistant" && message.phase == "final_answer")
        .is_some_and(|message| message.text == text)
}

fn push_message(
    dataset: &mut Dataset,
    index: usize,
    record: &TypedRecord,
    side: &str,
    phase: &str,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    let timestamp = record_timestamp(record);
    let (primary_flavor, flavor_tags, matched_cues) = classify_message(side, phase, text);
    let classified = classify(text);
    let message = MessageRecord {
        source: record.source.clone(),
        ts: timestamp.to_owned(),
        turn_id: dataset.turns[index].turn_id.clone(),
        side: side.to_owned(),
        phase: phase.to_owned(),
        text: text.to_owned(),
        primary_flavor,
        flavor_tags,
        matched_cues,
        shape: classified.shape,
        authored: classified.authored,
    };
    if dataset.turns[index].messages.iter().any(|row| {
        row.ts == message.ts
            && row.side == message.side
            && row.phase == message.phase
            && row.text == message.text
    }) {
        return;
    }
    dataset.turns[index].messages.push(message.clone());
    dataset.messages.push(message);
    touch_turn(dataset, index, timestamp);
}

fn classify_message(side: &str, phase: &str, text: &str) -> (String, Vec<String>, Vec<String>) {
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
        return (flavor.to_owned(), dedupe(tags), dedupe(cues));
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
    (primary, dedupe(tags), dedupe(cues))
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

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut rows = Vec::new();
    for value in values {
        if !rows.contains(&value) {
            rows.push(value);
        }
    }
    rows
}

fn register_tool_call(
    dataset: &mut Dataset,
    index: usize,
    record: &TypedRecord,
    call_id: &str,
    name: &str,
    arguments: &str,
    pending: &mut BTreeMap<String, PendingTool>,
) -> Result<(), Refusal> {
    if pending.contains_key(call_id) {
        return Err(refusal(
            "session::tool-invocation-duplicate",
            Some(call_id.to_owned()),
            "restore one invocation per vendor call identity",
        ));
    }
    *dataset.turns[index]
        .tool_calls
        .entry(name.to_owned())
        .or_default() += 1;
    if matches!(
        name,
        "apply_patch" | "Edit" | "Write" | "MultiEdit" | "file_change"
    ) {
        dataset.turns[index].patch_count = dataset.turns[index].patch_count.saturating_add(1);
        for path in tool_paths(arguments) {
            dataset.turns[index].touched_files.insert(path);
        }
    }
    if matches!(name, "web_search" | "web.run" | "WebSearch") {
        dataset.turns[index].web_search_count =
            dataset.turns[index].web_search_count.saturating_add(1);
    }
    let command = if matches!(
        name,
        "exec_command" | "command_execution" | "Bash" | "shell" | "local_shell" | "container.exec"
    ) {
        let (command, cwd) = tool_command(arguments);
        let command_index = dataset.commands.len();
        dataset.commands.push(CommandRecord {
            source: record.source.clone(),
            ts: record_timestamp(record).to_owned(),
            turn_id: dataset.turns[index].turn_id.clone(),
            cwd,
            command,
            exit_code: None,
            status: "called".to_owned(),
        });
        dataset.turns[index].command_count = dataset.turns[index].command_count.saturating_add(1);
        Some(command_index)
    } else {
        None
    };
    pending.insert(
        call_id.to_owned(),
        PendingTool {
            turn: index,
            command,
        },
    );
    touch_turn(dataset, index, record_timestamp(record));
    Ok(())
}

fn tool_command(arguments: &str) -> (String, Option<String>) {
    let parsed = serde_json::from_str::<Value>(arguments).unwrap_or(Value::Null);
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

fn tool_paths(arguments: &str) -> Vec<String> {
    let parsed = serde_json::from_str::<Value>(arguments).unwrap_or(Value::Null);
    let mut paths = ["file_path", "path"]
        .into_iter()
        .filter_map(|key| parsed.get(key).and_then(Value::as_str).map(str::to_owned))
        .collect::<Vec<_>>();
    if let Some(changes) = parsed.get("changes").and_then(Value::as_array) {
        paths.extend(changes.iter().filter_map(|change| {
            change
                .get("path")
                .or_else(|| change.get("file_path"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        }));
    }
    if let Some(patch) = parsed.get("input").and_then(Value::as_str) {
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
    record: &TypedRecord,
    call_id: &str,
    outcome: &ToolOutcome,
    pending: &mut BTreeMap<String, PendingTool>,
) {
    let Some(pending) = pending.remove(call_id) else {
        dataset.residues.push(Residue {
            source: record.source.clone(),
            line: record.line,
            kind: "tool-invocation-missing".to_owned(),
            detail: format!(
                "call_id={call_id} outcome was not attributed because no matching invocation preceded it"
            ),
            recovery: "restore the matching invocation to account this outcome; independent session evidence remains available"
                .to_owned(),
        });
        return;
    };
    let failed =
        outcome.result == ToolResult::Failed || outcome.exit_code.is_some_and(|code| code != 0);
    if failed {
        dataset.turns[pending.turn].error_count =
            dataset.turns[pending.turn].error_count.saturating_add(1);
    }
    if let Some(command) = pending.command {
        dataset.commands[command].exit_code = outcome.exit_code;
        dataset.commands[command].status = if outcome.output.as_deref().is_some_and(|output| {
            output.contains("Process running with session ID")
                || output.contains("Script running with cell ID")
        }) {
            "running"
        } else if outcome.exit_code.is_some() || outcome.result != ToolResult::Unreported {
            "completed"
        } else {
            "called"
        }
        .to_owned();
    }
    touch_turn(dataset, pending.turn, record_timestamp(record));
}

fn push_usage(dataset: &mut Dataset, record: &TypedRecord, usage: &listen::Usage) {
    let context = ContextSnapshot::admit(
        record_timestamp(record),
        record.context.model_context_window,
    )
    .expect("typed record timestamps were admitted before reduction");
    let reasoning = usage.reasoning_output_tokens.unwrap_or(0);
    let total = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(reasoning);
    if total == 0 {
        return;
    }
    dataset.tokens.push(TokenRecord {
        source: record.source.clone(),
        ts: context.at.to_string(),
        input: usage.input_tokens,
        cached: usage.cached_input_tokens,
        output: usage.output_tokens,
        reasoning,
        total,
        model_context_window: context.model_context_window,
    });
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
                .find(|message| message.shape == MessageShape::CompactionRehydration)
                .map_or_else(String::new, |message| message.text.clone())
        } else {
            marker.summary.clone()
        };
        let user_after = dataset
            .messages
            .iter()
            .filter(after)
            .find(|message| message.side == "user" && message.shape.is_authored())
            .and_then(|message| message.authored.clone())
            .unwrap_or_default();
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
        return UNRECOVERABLE_REHYDRATION_INTENT.to_owned();
    }
    rehydration_intent(summary)
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

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Refusal {
    Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("session-transcript".to_owned()),
            Some("one current typed transcript evidence stream".to_owned()),
            observed,
        )
        .with_detail(detail),
    )
}
