use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use clap::ArgMatches;
use wire::Refusal;

use super::model::{
    ActivityView, CommandAudit, CommandRecord, CommandView, CommitView, CompactionRecord,
    CompactionView, Dataset, DialogueLine, MessageRecord, MessageView, PhaseExample, PhaseView,
    PruningView, RecallWindow, Report, SliceView, SourceView, ThreadView, TimelineView,
    TokenRecord, TokenView, TurnRecord, TurnView,
};

const DEFAULT_RECALL_MAX_LINES: usize = 120;
const DEFAULT_RECALL_MAX_BYTES: usize = 20_000;
const DEFAULT_HORIZON_COMPACTIONS: usize = 3;
const DEFAULT_SUMMARY_RECENT: usize = 8;
const DEFAULT_TIMELINE_LIMIT: usize = 50;
const DEFAULT_TIMELINE_TEXT: usize = 180;
const DEFAULT_TURNS_LIMIT: usize = 20;
const DEFAULT_COMPACTIONS_LIMIT: usize = 25;
const DEFAULT_SLICES_LIMIT: usize = 25;
const DEFAULT_SLICE_TEXT: usize = 180;
const DEFAULT_PHASE_EXAMPLES: usize = 2;
const DEFAULT_PHASE_TEXT: usize = 180;
const DEFAULT_MESSAGES_LIMIT: usize = 80;
const DEFAULT_MESSAGE_TEXT: usize = 180;
const DEFAULT_COMMITS_LIMIT: usize = 25;
const DEFAULT_COMMANDS_LIMIT: usize = 80;
const DEFAULT_COMMAND_TEXT: usize = 220;

#[derive(Debug, Default)]
pub(crate) struct Filters {
    start: Option<String>,
    end: Option<String>,
    contains: Option<String>,
    turn_ids: BTreeSet<String>,
    tools: BTreeSet<String>,
}

impl Filters {
    pub(crate) fn from_matches(matches: &ArgMatches) -> Result<Self, Refusal> {
        let start = optional_bound(matches, "start")?;
        let end = optional_bound(matches, "end")?;
        if let (Some(start), Some(end)) = (&start, &end)
            && start > end
        {
            return Err(refusal(
                "session::bounds-reversed",
                Some(format!("{start} -> {end}")),
                "pass a start timestamp not later than the end timestamp",
            ));
        }
        Ok(Self {
            start,
            end,
            contains: one(matches, "contains").map(|value| value.to_lowercase()),
            turn_ids: values(matches, "turn-id").into_iter().collect(),
            tools: values(matches, "tool").into_iter().collect(),
        })
    }

    fn turn_matches(&self, turn: &TurnRecord) -> bool {
        if self
            .start
            .as_ref()
            .is_some_and(|start| turn_end(turn) < *start)
            || self.end.as_ref().is_some_and(|end| turn.start_ts > *end)
        {
            return false;
        }
        if !self.turn_ids.is_empty()
            && turn
                .turn_id
                .as_ref()
                .is_none_or(|turn_id| !self.turn_ids.contains(turn_id))
        {
            return false;
        }
        if !self.tools.is_empty() && !turn.tool_calls.keys().any(|tool| self.tools.contains(tool)) {
            return false;
        }
        self.contains.as_ref().is_none_or(|needle| {
            turn.messages
                .iter()
                .any(|message| message.text.to_lowercase().contains(needle))
        })
    }

    fn compaction_matches(&self, row: &CompactionRecord) -> bool {
        if self.start.as_ref().is_some_and(|start| row.ts < *start)
            || self.end.as_ref().is_some_and(|end| row.ts > *end)
        {
            return false;
        }
        self.contains.as_ref().is_none_or(|needle| {
            [
                &row.assistant_before,
                &row.summary_after,
                &row.intent,
                &row.user_after,
            ]
            .into_iter()
            .any(|text| text.to_lowercase().contains(needle))
        })
    }

    pub(crate) fn validate_turn_ids(&self, dataset: &Dataset) -> Result<(), Refusal> {
        if self.turn_ids.is_empty() || dataset.turns.is_empty() {
            return Ok(());
        }
        if dataset.turns.iter().all(|turn| turn.turn_id.is_none()) {
            return Err(refusal(
                "session::turn-id-unavailable",
                Some(self.turn_ids.iter().cloned().collect::<Vec<_>>().join(",")),
                "filter by --start, --end, or --contains; this transcript carries no \
                 turn identities",
            ));
        }
        Ok(())
    }
}

pub(crate) fn recall(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let max_lines = number(matches, "max-lines", DEFAULT_RECALL_MAX_LINES)?;
    let max_bytes = number(matches, "max-bytes", DEFAULT_RECALL_MAX_BYTES)?;
    let explain = flag(matches, "explain-pruning");
    let selected_compactions = dataset
        .compactions
        .iter()
        .filter(|row| filters.end.as_ref().is_none_or(|end| row.ts <= *end))
        .rev()
        .take(DEFAULT_HORIZON_COMPACTIONS)
        .collect::<Vec<_>>();
    let horizon_start = filters.start.clone().or_else(|| {
        selected_compactions
            .last()
            .map(|compaction| compaction.ts.clone())
    });
    let mut turns = dataset
        .turns
        .iter()
        .filter(|turn| filters.turn_matches(turn))
        .filter(|turn| {
            horizon_start
                .as_ref()
                .is_none_or(|start| turn_end(turn) >= *start)
        })
        .collect::<Vec<_>>();
    let before = turns.len();
    while turns.len() > 1 {
        let (lines, bytes) = estimated_budget(&turns);
        if lines <= max_lines && bytes <= max_bytes {
            break;
        }
        turns.remove(0);
    }
    let window = recall_window(
        1,
        &turns,
        dataset
            .compactions
            .iter()
            .filter(|row| filters.compaction_matches(row))
            .filter(|row| horizon_start.as_ref().is_none_or(|start| row.ts >= *start))
            .collect(),
        false,
    );
    Ok(Report::Recall {
        sources: dataset.sources.clone(),
        horizon_basis: if dataset.compactions.is_empty() {
            "full-history-no-compactions"
        } else {
            "compaction-count"
        }
        .to_owned(),
        requested_compactions: DEFAULT_HORIZON_COMPACTIONS,
        selected_compactions: selected_compactions.len(),
        window,
        pruning: PruningView {
            max_lines,
            max_bytes,
            turns_before: before,
            turns_after: turns.len(),
            explanation: explain.then(|| {
                format!(
                    "oldest turns were removed whole until the projected report fit \
                     {max_lines} lines and {max_bytes} bytes"
                )
            }),
        },
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn sweep(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let count = number(matches, "count", DEFAULT_HORIZON_COMPACTIONS)?;
    let compactions = dataset
        .compactions
        .iter()
        .filter(|row| filters.end.as_ref().is_none_or(|end| row.ts <= *end))
        .collect::<Vec<_>>();
    let selected = compactions
        .iter()
        .rev()
        .take(count)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let mut windows = Vec::new();
    if selected.is_empty() {
        let turns = dataset
            .turns
            .iter()
            .filter(|turn| filters.turn_matches(turn))
            .collect::<Vec<_>>();
        windows.push(recall_window(1, &turns, Vec::new(), false));
    } else {
        let horizon_start = filters.start.as_deref().unwrap_or(selected[0].ts.as_str());
        let boundaries = selected
            .iter()
            .filter(|compaction| compaction.ts.as_str() > horizon_start)
            .collect::<Vec<_>>();
        for index in 0..=boundaries.len() {
            let start = index
                .checked_sub(1)
                .and_then(|prior| boundaries.get(prior))
                .map_or(horizon_start, |compaction| compaction.ts.as_str());
            let end = boundaries
                .get(index)
                .map(|compaction| compaction.ts.as_str());
            let turns = dataset
                .turns
                .iter()
                .filter(|turn| filters.turn_matches(turn))
                .filter(|turn| {
                    turn.start_ts.as_str() >= start
                        && end.is_none_or(|end| turn.start_ts.as_str() < end)
                })
                .collect::<Vec<_>>();
            let anchors = selected
                .iter()
                .copied()
                .filter(|compaction| compaction.ts == start)
                .collect();
            windows.push(recall_window(index + 1, &turns, anchors, false));
        }
    }
    Ok(Report::Sweep {
        sources: dataset.sources.clone(),
        windows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn summary(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let recent = number(matches, "recent", DEFAULT_SUMMARY_RECENT)?;
    let prompts = dataset
        .messages
        .iter()
        .filter(|row| row.side == "user" && row.human)
        .rev()
        .take(recent)
        .map(message_view)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let finals = dataset
        .messages
        .iter()
        .filter(|row| row.side == "assistant" && row.phase == "final_answer")
        .rev()
        .take(recent)
        .map(message_view)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Ok(Report::Summary {
        sources: dataset.sources.clone(),
        turns: dataset.turns.len(),
        completed: dataset.turns.iter().filter(|turn| turn.completed).count(),
        compactions: dataset.compactions.len(),
        activity: activity(dataset.turns.iter().collect()),
        recent_prompts: prompts,
        recent_finals: finals,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn tokens(dataset: &Dataset, matches: &ArgMatches) -> Report {
    let by_file = flag(matches, "by-file") || dataset.sources.len() > 1;
    let mut rows = Vec::new();
    if by_file {
        for source in &dataset.sources {
            rows.push(token_view(
                &source.name,
                dataset
                    .tokens
                    .iter()
                    .filter(|row| row.source == source.path),
            ));
        }
    }
    rows.push(token_view("TOTAL", dataset.tokens.iter()));
    Report::Tokens {
        sources: dataset.sources.clone(),
        usage: rows,
        residues: dataset.residues.clone(),
    }
}

pub(crate) fn timeline(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let limit = number(matches, "limit", DEFAULT_TIMELINE_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_TIMELINE_TEXT)?;
    let mut rows = dataset
        .turns
        .iter()
        .filter(|turn| filters.turn_matches(turn))
        .map(|turn| TimelineView {
            source: turn.source.clone(),
            ts: turn.start_ts.clone(),
            kind: "turn".to_owned(),
            turn_id: turn.turn_id.clone(),
            text: clip(&turn_timeline_text(turn), max_text),
        })
        .collect::<Vec<_>>();
    if filters.turn_ids.is_empty() && filters.tools.is_empty() {
        rows.extend(
            dataset
                .compactions
                .iter()
                .filter(|row| filters.compaction_matches(row))
                .map(|row| TimelineView {
                    source: row.source.clone(),
                    ts: row.ts.clone(),
                    kind: "compaction".to_owned(),
                    turn_id: None,
                    text: clip(
                        &format!(
                            "assistant_before={} summary_after={} intent={} user_after={}",
                            row.assistant_before, row.summary_after, row.intent, row.user_after
                        ),
                        max_text,
                    ),
                }),
        );
    }
    rows.sort_by(|left, right| (&left.ts, &left.kind).cmp(&(&right.ts, &right.kind)));
    trim_oldest(&mut rows, limit);
    Ok(Report::Timeline {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn turns(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let limit = number(matches, "limit", DEFAULT_TURNS_LIMIT)?;
    let full = one(matches, "view").is_some_and(|view| view == "full");
    let mut rows = dataset
        .turns
        .iter()
        .filter(|turn| filters.turn_matches(turn))
        .map(|turn| turn_view(turn, full, DEFAULT_TIMELINE_TEXT))
        .collect::<Vec<_>>();
    trim_oldest(&mut rows, limit);
    Ok(Report::Turns {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn compactions(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMPACTIONS_LIMIT)?;
    let mut rows = dataset
        .compactions
        .iter()
        .map(compaction_view)
        .collect::<Vec<_>>();
    trim_oldest(&mut rows, limit);
    Ok(Report::Compactions {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn slices(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number_allow_zero(matches, "limit", DEFAULT_SLICES_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_SLICE_TEXT)?;
    let full = one(matches, "view").is_some_and(|view| view == "full");
    let selected = values(matches, "slice-id")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut rows = build_slices(dataset, full, max_text);
    rows.reverse();
    if !selected.is_empty() {
        rows.retain(|row| selected.contains(&row.id));
    }
    rows.truncate(limit);
    Ok(Report::Slices {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn phases(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let examples = number_allow_zero(matches, "examples", DEFAULT_PHASE_EXAMPLES)?;
    let max_text = number(matches, "max-text", DEFAULT_PHASE_TEXT)?;
    let turns = dataset
        .turns
        .iter()
        .filter(|turn| filters.turn_matches(turn))
        .collect::<Vec<_>>();
    let mut rows = build_phases(&turns, examples, max_text);
    if one(matches, "limit").is_some() {
        rows.truncate(number(matches, "limit", rows.len().max(1))?);
    }
    Ok(Report::Phases {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn messages(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let filters = Filters::from_matches(matches)?;
    let sides = values(matches, "side").into_iter().collect::<BTreeSet<_>>();
    let phases = values(matches, "phase-kind")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let flavors = values(matches, "flavor")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let limit = number(matches, "limit", DEFAULT_MESSAGES_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_MESSAGE_TEXT)?;
    let mut rows = dataset
        .messages
        .iter()
        .filter(|row| {
            filters.start.as_ref().is_none_or(|start| row.ts >= *start)
                && filters.end.as_ref().is_none_or(|end| row.ts <= *end)
                && filters
                    .contains
                    .as_ref()
                    .is_none_or(|needle| row.text.to_lowercase().contains(needle))
                && (filters.turn_ids.is_empty()
                    || row
                        .turn_id
                        .as_ref()
                        .is_some_and(|turn_id| filters.turn_ids.contains(turn_id)))
        })
        .filter(|row| sides.is_empty() || sides.contains(&row.side))
        .filter(|row| phases.is_empty() || phases.contains(&row.phase))
        .filter(|row| {
            flavors.is_empty()
                || flavors.contains(&row.primary_flavor)
                || row.flavor_tags.iter().any(|tag| flavors.contains(tag))
        })
        .map(|row| {
            let mut view = message_view(row);
            view.text = clip(&view.text, max_text);
            view
        })
        .collect::<Vec<_>>();
    if !flag(matches, "oldest-first") {
        rows.reverse();
    }
    rows.truncate(limit);
    Ok(Report::Messages {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn commits(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMMITS_LIMIT)?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for turn in &dataset.turns {
        let user = turn
            .messages
            .iter()
            .find(|message| message.side == "user" && message.human)
            .map(|message| clip(&message.text, 120))
            .unwrap_or_default();
        let mut messages = turn
            .messages
            .iter()
            .rfind(|message| message.side == "assistant" && message.phase == "final_answer")
            .into_iter()
            .collect::<Vec<_>>();
        messages.extend(
            turn.messages
                .iter()
                .rev()
                .filter(|message| message.side == "assistant" && message.phase != "final_answer"),
        );
        for message in messages {
            for line in message.text.lines() {
                if !commit_line(line) {
                    continue;
                }
                for sha in commit_tokens(line) {
                    if sha.bytes().all(|byte| byte.is_ascii_digit()) || !seen.insert(sha.clone()) {
                        continue;
                    }
                    rows.push(CommitView {
                        source: turn.source.clone(),
                        ts: turn.start_ts.clone(),
                        turn_id: turn.turn_id.clone(),
                        sha,
                        line: clip(line, 160),
                        user: user.clone(),
                    });
                }
            }
        }
    }
    trim_oldest(&mut rows, limit);
    Ok(Report::Commits {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn commands(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMMANDS_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_COMMAND_TEXT)?;
    let population = dataset
        .commands
        .iter()
        .filter(|row| row.exit_code.is_some() && row.status != "running")
        .collect::<Vec<_>>();
    let latest_compaction = dataset.compactions.last().map(|row| row.ts.as_str());
    let mut filters = Vec::new();
    let mut rows = population.clone();
    if flag(matches, "since-compaction") {
        filters.push("since-compaction".to_owned());
        if let Some(ts) = latest_compaction {
            rows.retain(|row| row.ts.as_str() > ts);
        }
    }
    if flag(matches, "failed") {
        filters.push("failed".to_owned());
        rows.retain(|row| row.exit_code.is_some_and(|code| code != 0));
    }
    if flag(matches, "pipelines") {
        filters.push("pipelines".to_owned());
        rows.retain(|row| has_pipeline(&row.command));
    }
    if flag(matches, "noncanonical-pipelines") {
        filters.push("noncanonical-pipelines".to_owned());
        rows.retain(|row| noncanonical_pipeline(&row.command));
    }
    if flag(matches, "summary") {
        return Ok(Report::Commands {
            sources: dataset.sources.clone(),
            audit: Some(command_audit(population.len(), &rows, filters)),
            rows: Vec::new(),
            residues: dataset.residues.clone(),
        });
    }
    let newest = flag(matches, "newest-first");
    let selected = if newest {
        rows.into_iter().rev().take(limit).collect::<Vec<_>>()
    } else {
        rows.into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    Ok(Report::Commands {
        sources: dataset.sources.clone(),
        rows: selected
            .into_iter()
            .map(|row| command_view(row, max_text))
            .collect(),
        audit: None,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn thread(source: SourceView, dataset: &Dataset, id: String) -> Report {
    let latest = dataset.turns.last();
    let thread = ThreadView {
        id,
        vendor: source.vendor,
        transcript: source.path.clone(),
        turns: dataset.turns.len(),
        compactions: dataset.compactions.len(),
        start: dataset.turns.first().map(|turn| turn.start_ts.clone()),
        end: dataset.turns.last().map(turn_end),
        latest_turn: latest.map(|turn| turn_view(turn, false, DEFAULT_TIMELINE_TEXT)),
    };
    Report::Thread {
        source,
        thread,
        residues: dataset.residues.clone(),
    }
}

fn recall_window(
    index: usize,
    turns: &[&TurnRecord],
    compactions: Vec<&CompactionRecord>,
    full: bool,
) -> RecallWindow {
    RecallWindow {
        index,
        start: turns
            .first()
            .map(|turn| turn.start_ts.clone())
            .or_else(|| compactions.first().map(|row| row.ts.clone())),
        end: turns
            .last()
            .map(|turn| turn_end(turn))
            .or_else(|| compactions.last().map(|row| row.ts.clone())),
        turns: turns
            .iter()
            .map(|turn| turn_view(turn, full, DEFAULT_TIMELINE_TEXT))
            .collect(),
        compactions: compactions.into_iter().map(compaction_view).collect(),
        latest_final: turns.iter().rev().find_map(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|message| message.phase == "final_answer")
                .map(|message| message.text.clone())
        }),
        activity: activity(turns.to_vec()),
    }
}

fn activity(turns: Vec<&TurnRecord>) -> ActivityView {
    ActivityView {
        commands: turns.iter().map(|turn| turn.command_count).sum(),
        patches: turns.iter().map(|turn| turn.patch_count).sum(),
        errors: turns.iter().map(|turn| turn.error_count).sum(),
        web_searches: turns.iter().map(|turn| turn.web_search_count).sum(),
    }
}

fn estimated_budget(turns: &[&TurnRecord]) -> (usize, usize) {
    let lines = turns
        .iter()
        .map(|turn| 1 + turn.messages.len())
        .sum::<usize>();
    let bytes = turns
        .iter()
        .flat_map(|turn| turn.messages.iter())
        .map(|message| message.text.len())
        .sum();
    (lines, bytes)
}

fn turn_view(turn: &TurnRecord, full: bool, max_text: usize) -> TurnView {
    TurnView {
        source: turn.source.clone(),
        vendor: turn.vendor,
        turn_id: turn.turn_id.clone(),
        start: turn.start_ts.clone(),
        end: turn.end_ts.clone(),
        completed: turn.completed,
        commands: turn.command_count,
        patches: turn.patch_count,
        errors: turn.error_count,
        web_searches: turn.web_search_count,
        compactions: turn.compaction_count,
        dialogue: turn
            .messages
            .iter()
            .map(|message| DialogueLine {
                side: message.side.clone(),
                text: if full {
                    message.text.clone()
                } else {
                    clip(&message.text, max_text)
                },
            })
            .collect(),
    }
}

fn message_view(row: &MessageRecord) -> MessageView {
    MessageView {
        source: row.source.clone(),
        ts: row.ts.clone(),
        turn_id: row.turn_id.clone(),
        side: row.side.clone(),
        phase: row.phase.clone(),
        flavor: row.primary_flavor.clone(),
        tags: row.flavor_tags.clone(),
        cues: row.matched_cues.clone(),
        text: row.text.clone(),
    }
}

fn compaction_view(row: &CompactionRecord) -> CompactionView {
    CompactionView {
        source: row.source.clone(),
        ts: row.ts.clone(),
        assistant_before: clip(&row.assistant_before, DEFAULT_TIMELINE_TEXT),
        summary_after: clip(&row.summary_after, DEFAULT_TIMELINE_TEXT),
        intent: clip(&row.intent, DEFAULT_TIMELINE_TEXT),
        user_after: clip(&row.user_after, DEFAULT_TIMELINE_TEXT),
    }
}

fn token_view<'a>(source: &str, records: impl Iterator<Item = &'a TokenRecord>) -> TokenView {
    let rows = records.collect::<Vec<_>>();
    let input = rows.iter().map(|row| row.input).sum();
    let cached = rows.iter().map(|row| row.cached).sum();
    TokenView {
        source: source.to_owned(),
        total: rows.iter().map(|row| row.total).sum(),
        input,
        cached,
        uncached: input.saturating_sub(cached),
        output: rows.iter().map(|row| row.output).sum(),
        reasoning: rows.iter().map(|row| row.reasoning).sum(),
        model_context_window: rows.last().and_then(|row| row.model_context_window),
        snapshots: rows.len() as u64,
        first_snapshot: rows.first().map(|row| row.ts.clone()),
        last_snapshot: rows.last().map(|row| row.ts.clone()),
    }
}

fn turn_timeline_text(turn: &TurnRecord) -> String {
    let user = turn
        .messages
        .iter()
        .rev()
        .find(|message| message.side == "user")
        .map(|message| message.text.as_str())
        .unwrap_or("");
    let assistant = turn
        .messages
        .iter()
        .rev()
        .find(|message| message.side == "assistant")
        .map(|message| message.text.as_str())
        .unwrap_or("");
    format!(
        "completed={} cmds={} patches={} errors={} user={} assistant={}",
        turn.completed, turn.command_count, turn.patch_count, turn.error_count, user, assistant
    )
}

fn build_slices(dataset: &Dataset, full: bool, max_text: usize) -> Vec<SliceView> {
    if dataset.compactions.is_empty() {
        let Some(first) = dataset.turns.first() else {
            return Vec::new();
        };
        let last = dataset
            .turns
            .last()
            .expect("one first turn means one last turn");
        let start = first.start_ts.clone();
        let end = turn_end(last);
        return vec![slice_view(
            format!("{}-{}", compact_timestamp(&start), compact_timestamp(&end)),
            first.source.clone(),
            start,
            end,
            "full_history_no_compactions",
            "full_history",
            &dataset.turns.iter().collect::<Vec<_>>(),
            None,
            full,
            max_text,
        )];
    }
    let mut rows = Vec::new();
    for (index, compaction) in dataset.compactions.iter().enumerate() {
        let start = index
            .checked_sub(1)
            .and_then(|prior| dataset.compactions.get(prior))
            .map(|row| row.ts.clone())
            .or_else(|| {
                dataset
                    .turns
                    .iter()
                    .filter(|turn| turn.source == compaction.source)
                    .map(|turn| turn.start_ts.clone())
                    .min()
            })
            .unwrap_or_else(|| compaction.ts.clone());
        let turns = dataset
            .turns
            .iter()
            .filter(|turn| {
                turn.source == compaction.source
                    && turn_end(turn) >= start
                    && turn.start_ts <= compaction.ts
            })
            .collect::<Vec<_>>();
        rows.push(slice_view(
            format!("compaction-{}", index + 1),
            compaction.source.clone(),
            start,
            compaction.ts.clone(),
            "compaction_interval",
            "compaction",
            &turns,
            Some(compaction),
            full,
            max_text,
        ));
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn slice_view(
    id: String,
    source: String,
    start: String,
    end: String,
    basis: &str,
    anchor: &str,
    turns: &[&TurnRecord],
    compaction: Option<&CompactionRecord>,
    full: bool,
    max_text: usize,
) -> SliceView {
    let mut messages = Vec::new();
    if full && let Some(compaction) = compaction {
        for (side, text) in [
            ("assistant_before", &compaction.assistant_before),
            ("intent", &compaction.intent),
            ("summary_after", &compaction.summary_after),
            ("user_after", &compaction.user_after),
        ] {
            if !text.is_empty() {
                messages.push(DialogueLine {
                    side: side.to_owned(),
                    text: clip(text, max_text),
                });
            }
        }
    }
    SliceView {
        id,
        source,
        start,
        end,
        basis: basis.to_owned(),
        anchor: anchor.to_owned(),
        status: if turns.iter().any(|turn| !turn.completed) {
            "open"
        } else {
            "closed"
        }
        .to_owned(),
        compactions: usize::from(compaction.is_some()),
        turns: turns
            .iter()
            .filter_map(|turn| turn.turn_id.clone())
            .collect(),
        patches: turns.iter().map(|turn| turn.patch_count).sum(),
        crossing_files: turns
            .iter()
            .flat_map(|turn| turn.touched_files.iter())
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(8)
            .collect(),
        messages,
    }
}

fn compact_timestamp(value: &str) -> String {
    let mut compact = String::with_capacity(value.len());
    for character in value.chars() {
        if character == char::from(90_u8) {
            compact.push(char::from(122_u8));
        } else if !"-:.+".contains(character) {
            compact.push(character);
        }
    }
    compact
}

fn build_phases(turns: &[&TurnRecord], examples: usize, max_text: usize) -> Vec<PhaseView> {
    let mut groups: Vec<(String, Vec<&TurnRecord>)> = Vec::new();
    for turn in turns {
        let family = turn_family(turn);
        if let Some((last, rows)) = groups.last_mut()
            && *last == family
        {
            rows.push(*turn);
        } else {
            groups.push((family, vec![*turn]));
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(index, (family, rows))| {
            let top_paths = path_counts(&rows);
            PhaseView {
                index: index + 1,
                primary: family.clone(),
                family: family.clone(),
                turns: rows.len(),
                commands: rows.iter().map(|turn| turn.command_count).sum(),
                patches: rows.iter().map(|turn| turn.patch_count).sum(),
                compactions: rows.iter().map(|turn| turn.compaction_count).sum(),
                errors: rows.iter().map(|turn| turn.error_count).sum(),
                duration_millis: None,
                start: rows
                    .first()
                    .map_or_else(String::new, |turn| turn.start_ts.clone()),
                end: rows.last().map_or_else(String::new, |turn| turn_end(turn)),
                top_paths,
                examples: rows
                    .iter()
                    .take(examples)
                    .map(|turn| phase_example(turn, &family, max_text))
                    .collect(),
            }
        })
        .collect()
}

fn turn_family(turn: &TurnRecord) -> String {
    if turn.patch_count > 0 {
        "implementation"
    } else if turn.command_count > 0 {
        "execution"
    } else if turn.web_search_count > 0 {
        "research"
    } else {
        "dialogue"
    }
    .to_owned()
}

fn path_counts(turns: &[&TurnRecord]) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for path in turns.iter().flat_map(|turn| turn.touched_files.iter()) {
        *counts.entry(path.clone()).or_insert(0_usize) += 1;
    }
    let mut rows = counts.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.into_iter().take(5).map(|(path, _)| path).collect()
}

fn phase_example(turn: &TurnRecord, family: &str, max_text: usize) -> PhaseExample {
    PhaseExample {
        start: turn.start_ts.clone(),
        turn_id: turn.turn_id.clone(),
        archetype: family.to_owned(),
        path: turn.touched_files.iter().next().cloned(),
        user: turn
            .messages
            .iter()
            .find(|message| message.side == "user" && message.human)
            .map(|message| clip(&message.text, max_text)),
        final_answer: turn
            .messages
            .iter()
            .rev()
            .find(|message| message.phase == "final_answer")
            .map(|message| clip(&message.text, max_text)),
    }
}

fn commit_tokens(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|word| word.trim_matches(|character: char| !character.is_ascii_hexdigit()))
        .filter(|word| {
            (7..=40).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(str::to_ascii_lowercase)
        .collect()
}

fn commit_line(line: &str) -> bool {
    line.split(|character: char| !character.is_alphanumeric())
        .any(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "commit" | "committed" | "sha"
            )
        })
}

fn command_view(row: &CommandRecord, max_text: usize) -> CommandView {
    CommandView {
        source: row.source.clone(),
        ts: row.ts.clone(),
        turn_id: row.turn_id.clone(),
        cwd: row.cwd.clone(),
        command: clip(&row.command, max_text),
        exit_code: row.exit_code,
        status: row.status.clone(),
        wrapper: starts_with_wrapper(&row.command),
        pipeline: if noncanonical_pipeline(&row.command) {
            Some("noncanonical".to_owned())
        } else if has_pipeline(&row.command) {
            Some("canonical".to_owned())
        } else {
            None
        },
    }
}

fn command_audit(population: usize, rows: &[&CommandRecord], filters: Vec<String>) -> CommandAudit {
    let wrapper = rows
        .iter()
        .filter(|row| starts_with_wrapper(&row.command))
        .count();
    let pipelines = rows.iter().filter(|row| has_pipeline(&row.command)).count();
    let canonical = rows
        .iter()
        .filter(|row| canonical_pipeline(&row.command))
        .count();
    let mut labels = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| noncanonical_pipeline(&row.command))
    {
        *labels.entry(command_label(&row.command)).or_insert(0_usize) += 1;
    }
    let mut labels = labels.into_iter().collect::<Vec<_>>();
    labels.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    CommandAudit {
        population,
        matched: rows.len(),
        filters,
        failed: rows
            .iter()
            .filter(|row| row.exit_code.is_some_and(|code| code != 0))
            .count(),
        wrapper,
        non_wrapper: rows.len().saturating_sub(wrapper),
        pipelines,
        canonical_pipelines: canonical,
        noncanonical_pipelines: pipelines.saturating_sub(canonical),
        top_noncanonical: labels
            .into_iter()
            .take(4)
            .map(|(label, count)| format!("{label}:{count}"))
            .collect(),
    }
}

fn split_pipeline(command: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let bytes = command.as_bytes();
    for (index, character) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if character == '|'
            && index.checked_sub(1).and_then(|offset| bytes.get(offset)) != Some(&b'|')
            && bytes.get(index + 1) != Some(&b'|')
        {
            rows.push(command[start..index].trim().to_owned());
            start = index + 1;
        }
    }
    if rows.is_empty() {
        vec![command.trim().to_owned()]
    } else {
        rows.push(command[start..].trim().to_owned());
        rows
    }
}

fn has_pipeline(command: &str) -> bool {
    split_pipeline(command).len() > 1
}

fn canonical_pipeline(command: &str) -> bool {
    let segments = split_pipeline(command);
    segments.len() > 1 && segments.iter().all(|segment| starts_with_wrapper(segment))
}

fn noncanonical_pipeline(command: &str) -> bool {
    has_pipeline(command) && !canonical_pipeline(command)
}

fn starts_with_wrapper(command: &str) -> bool {
    let words = shell_words::split(command).unwrap_or_else(|_| {
        command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let words = words
        .iter()
        .map(String::as_str)
        .skip_while(|word| environment_assignment(word))
        .collect::<Vec<_>>();
    (words.starts_with(&["spice", "agent", "run"])
        && (words.len() == 3 || words.get(3) == Some(&"--")))
        || (words.starts_with(&["uv", "run", "spice", "agent", "run"])
            && (words.len() == 5 || words.get(5) == Some(&"--")))
}

fn environment_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || index > 0 && byte.is_ascii_digit()
        })
}

fn command_label(command: &str) -> String {
    if starts_with_wrapper(command) {
        return "spice agent run".to_owned();
    }
    shell_words::split(command)
        .unwrap_or_else(|_| command.split_whitespace().map(str::to_owned).collect())
        .into_iter()
        .find(|word| !environment_assignment(word))
        .unwrap_or_else(|| "-".to_owned())
}

fn turn_end(turn: &TurnRecord) -> String {
    turn.end_ts
        .clone()
        .unwrap_or_else(|| turn.last_activity_ts.clone())
}

fn optional_bound(matches: &ArgMatches, name: &str) -> Result<Option<String>, Refusal> {
    one(matches, name)
        .map(|value| normalize_bound(value))
        .transpose()
}

fn normalize_bound(value: &str) -> Result<String, Refusal> {
    value
        .parse::<jiff::Timestamp>()
        .ok()
        .and_then(|timestamp| wire::Instant::new(timestamp).ok())
        .map(|timestamp| timestamp.to_string())
        .ok_or_else(|| {
            refusal(
                "session::timestamp-invalid",
                Some(value.to_owned()),
                "pass an ISO-8601 timestamp with an explicit UTC offset",
            )
        })
}

pub(crate) fn values(matches: &ArgMatches, name: &str) -> Vec<String> {
    matches
        .try_get_many::<String>(name)
        .ok()
        .flatten()
        .map(|values| {
            values
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn number(matches: &ArgMatches, name: &str, default: usize) -> Result<usize, Refusal> {
    signed_number(matches, name, default).map(|value| value.max(1))
}

fn number_allow_zero(matches: &ArgMatches, name: &str, default: usize) -> Result<usize, Refusal> {
    signed_number(matches, name, default)
}

fn signed_number(matches: &ArgMatches, name: &str, default: usize) -> Result<usize, Refusal> {
    one(matches, name).map_or(Ok(default), |value| {
        value
            .parse::<i128>()
            .map_err(|_| {
                refusal(
                    "session::bound-invalid",
                    Some(format!("--{name}={value}")),
                    "pass a decimal integer bound representable by this process",
                )
            })
            .and_then(|value| {
                usize::try_from(value.max(0)).map_err(|_| {
                    refusal(
                        "session::bound-invalid",
                        Some(format!("--{name}={value}")),
                        "pass a smaller decimal integer bound",
                    )
                })
            })
    })
}

fn one<'a>(matches: &'a ArgMatches, name: &str) -> Option<&'a String> {
    matches.try_get_one::<String>(name).ok().flatten()
}

fn flag(matches: &ArgMatches, name: &str) -> bool {
    matches
        .try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

fn trim_oldest<T>(rows: &mut Vec<T>, limit: usize) {
    if rows.len() > limit {
        rows.drain(..rows.len() - limit);
    }
}

fn clip(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut clipped = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    clipped.push('…');
    clipped
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Refusal {
    Refusal {
        reason: reason.to_owned(),
        witness: Some("session-query".to_owned()),
        expected: Some("one admitted bounded transcript query".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    }
}
