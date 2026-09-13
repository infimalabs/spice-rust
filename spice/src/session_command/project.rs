use std::collections::BTreeSet;
use std::path::Path;

use clap::ArgMatches;
use wire::Refusal;

use super::dataset::{MessageDataset, MessageQuery, WindowOptions, WindowScan, WindowTurn};
use super::model::{
    ActivityView, CommitView, CompactionRecord, CompactionView, Dataset, DialogueLine,
    MessageRecord, MessageView, PhaseExample, PruningView, RecallAuthorities, RecallWindow, Report,
    SliceView, TimelineView, TurnRecord, TurnView,
};

// Two report families are split out by `mod` rather than left inline, following
// the decision recorded at `board_command.rs`: a child module already reads its
// ancestor's private items, so the seam costs no visibility widening. Each is
// named for the question it answers, and each re-exports its entry point so the
// dispatch table in `session_command.rs` still reads as one column of reports.
mod command;
mod phase;

pub(crate) use command::{commands, options as command_options};
pub(crate) use phase::{PhaseOptions, options as phase_options, phase_example, turn_family};

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
const MAX_AGGREGATE_RECENT: usize = 256;
const MAX_WINDOW_COUNT: usize = 256;
const MAX_WINDOW_LINES: usize = 4_096;
const MAX_WINDOW_BYTES: usize = 1_048_576;
const MAX_WINDOW_TEXT: usize = 16_384;
const MAX_SLICE_ORDINAL: usize = 4 * 1_024;

struct WindowPlan<'a> {
    horizon_start: Option<&'a str>,
    selected: Vec<&'a CompactionRecord>,
    intervals: Vec<WindowInterval<'a>>,
}

struct WindowInterval<'a> {
    start: Option<&'a str>,
    end: Option<&'a str>,
    anchors: Vec<&'a CompactionRecord>,
}

pub(crate) struct MessageOptions {
    pub(crate) query: MessageQuery,
    max_text: usize,
    oldest_first: bool,
}

pub(crate) struct SliceOptions {
    pub(crate) limit: usize,
    max_text: usize,
    full: bool,
    selected: BTreeSet<String>,
}

impl SliceOptions {
    pub(crate) fn scan(&self) -> (usize, bool) {
        if self.selected.is_empty() {
            return (self.limit.max(1), false);
        }
        let count = self
            .selected
            .iter()
            .filter_map(|id| id.strip_prefix("compaction-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(1)
            .max(1);
        (count, true)
    }
}

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

    pub(crate) fn start_bound(&self) -> Option<&str> {
        self.start.as_deref()
    }

    pub(crate) fn end_bound(&self) -> Option<&str> {
        self.end.as_deref()
    }

    pub(crate) fn turn_matches(&self, turn: &TurnRecord) -> bool {
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

    pub(crate) fn validate_turn_identity_available(&self, available: bool) -> Result<(), Refusal> {
        if self.turn_ids.is_empty() || available {
            return Ok(());
        }
        Err(refusal(
            "session::turn-id-unavailable",
            Some(self.turn_ids.iter().cloned().collect::<Vec<_>>().join(",")),
            "filter by --start, --end, or --contains; this transcript carries no turn identities",
        ))
    }
}

pub(crate) fn horizon_count(matches: &ArgMatches, action: &str) -> Result<usize, Refusal> {
    if action == "sweep" {
        number(matches, "count", DEFAULT_HORIZON_COMPACTIONS)
    } else {
        Ok(DEFAULT_HORIZON_COMPACTIONS)
    }
}

pub(crate) fn window_options(matches: &ArgMatches, action: &str) -> Result<WindowOptions, Refusal> {
    let compactions = horizon_count(matches, action)?;
    if action == "briefing" {
        Ok(WindowOptions::Briefing {
            max_lines: number(matches, "max-lines", DEFAULT_RECALL_MAX_LINES)?,
            max_bytes: number(matches, "max-bytes", DEFAULT_RECALL_MAX_BYTES)?,
            explain: flag(matches, "explain-pruning"),
            compactions,
        })
    } else {
        Ok(WindowOptions::Sweep { compactions })
    }
}

pub(crate) fn validate_window_options(matches: &ArgMatches, action: &str) -> Result<(), Refusal> {
    let bounded = |name: &str, value: usize, maximum: usize| {
        if value <= maximum {
            Ok(())
        } else {
            Err(refusal(
                "session::bound-exceeded",
                Some(format!("--{name}={value} maximum={maximum}")),
                "pass a session window bound within the repository ceiling",
            ))
        }
    };
    match action {
        "briefing" => {
            bounded(
                "max-lines",
                number(matches, "max-lines", DEFAULT_RECALL_MAX_LINES)?,
                MAX_WINDOW_LINES,
            )?;
            bounded(
                "max-bytes",
                number(matches, "max-bytes", DEFAULT_RECALL_MAX_BYTES)?,
                MAX_WINDOW_BYTES,
            )
        }
        "sweep" => bounded("count", horizon_count(matches, action)?, MAX_WINDOW_COUNT),
        "slices" => {
            bounded(
                "limit",
                number_allow_zero(matches, "limit", DEFAULT_SLICES_LIMIT)?,
                MAX_WINDOW_COUNT,
            )?;
            bounded(
                "max-text",
                number(matches, "max-text", DEFAULT_SLICE_TEXT)?,
                MAX_WINDOW_TEXT,
            )
        }
        _ => Ok(()),
    }
}

pub(crate) fn recall(
    dataset: &WindowScan,
    filters: &Filters,
    authorities: RecallAuthorities,
) -> Result<Report, Refusal> {
    let (max_lines, max_bytes, explain) = dataset
        .options
        .briefing()
        .expect("briefing projection carries briefing options");
    let plan = window_plan(&dataset.compactions, filters, dataset.options.compactions());
    let turns = plan_turns(&dataset.turns, plan.horizon_start, None);
    let window = recall_window(
        1,
        &turns,
        plan.selected
            .iter()
            .copied()
            .filter(|row| filters.compaction_matches(row))
            .collect(),
    );
    Ok(Report::Recall {
        sources: dataset.sources.clone(),
        horizon_basis: if plan.selected.is_empty() {
            "full-history-no-compactions"
        } else {
            "compaction-count"
        }
        .to_owned(),
        requested_compactions: dataset.options.compactions(),
        selected_compactions: plan.selected.len(),
        authorities: Box::new(authorities),
        window,
        pruning: PruningView {
            max_lines,
            max_bytes,
            turns_before: dataset.turns_before,
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

pub(crate) fn sweep(dataset: &WindowScan, filters: &Filters) -> Result<Report, Refusal> {
    let plan = window_plan(&dataset.compactions, filters, dataset.options.compactions());
    let mut windows = Vec::new();
    for (index, interval) in plan.intervals.iter().enumerate() {
        let turns = plan_turns(&dataset.turns, interval.start, interval.end);
        windows.push(recall_window(index + 1, &turns, interval.anchors.clone()));
    }
    Ok(Report::Sweep {
        sources: dataset.sources.clone(),
        windows,
        residues: dataset.residues.clone(),
    })
}

fn window_plan<'a>(
    compactions: &'a [CompactionRecord],
    filters: &'a Filters,
    count: usize,
) -> WindowPlan<'a> {
    let mut selected = compactions
        .iter()
        .filter(|row| filters.end.as_ref().is_none_or(|end| row.ts <= *end))
        .collect::<Vec<_>>();
    let first = selected.len().saturating_sub(count);
    selected.drain(..first);
    let horizon_start = filters
        .start
        .as_deref()
        .or_else(|| selected.first().map(|compaction| compaction.ts.as_str()));
    let mut intervals = Vec::new();
    let boundaries = selected
        .iter()
        .copied()
        .filter(|compaction| horizon_start.is_none_or(|start| compaction.ts.as_str() > start))
        .collect::<Vec<_>>();
    for index in 0..=boundaries.len() {
        let start = index
            .checked_sub(1)
            .and_then(|prior| boundaries.get(prior))
            .map(|compaction| compaction.ts.as_str())
            .or(horizon_start);
        let end = boundaries
            .get(index)
            .map(|compaction| compaction.ts.as_str());
        let anchors = selected
            .iter()
            .copied()
            .filter(|compaction| Some(compaction.ts.as_str()) == start)
            .collect();
        intervals.push(WindowInterval {
            start,
            end,
            anchors,
        });
    }
    WindowPlan {
        horizon_start,
        selected,
        intervals,
    }
}

fn plan_turns<'a>(
    turns: &'a [WindowTurn],
    start: Option<&str>,
    end: Option<&str>,
) -> Vec<&'a WindowTurn> {
    turns
        .iter()
        .filter(|turn| turn_overlaps_interval(turn, start, end))
        .collect()
}

fn turn_overlaps_interval(turn: &WindowTurn, start: Option<&str>, end: Option<&str>) -> bool {
    start.is_none_or(|start| turn.end.as_str() >= start)
        && end.is_none_or(|end| turn.view.start.as_str() < end)
}

pub(crate) fn summary_recent(matches: &ArgMatches) -> Result<usize, Refusal> {
    let recent = number(matches, "recent", DEFAULT_SUMMARY_RECENT)?;
    if recent > MAX_AGGREGATE_RECENT {
        return Err(refusal(
            "session::bound-exceeded",
            Some(format!("--recent={recent} maximum={MAX_AGGREGATE_RECENT}")),
            "pass a recent-message bound within the repository ceiling",
        ));
    }
    Ok(recent)
}

pub(crate) fn tokens_by_file(matches: &ArgMatches) -> bool {
    flag(matches, "by-file")
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
        .map(|turn| turn_view(turn, full, DEFAULT_TIMELINE_TEXT, false))
        .collect::<Vec<_>>();
    trim_oldest(&mut rows, limit);
    Ok(Report::Turns {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}

pub(crate) fn compaction_limit(matches: &ArgMatches) -> Result<usize, Refusal> {
    number(matches, "limit", DEFAULT_COMPACTIONS_LIMIT)
}

pub(crate) fn compactions(dataset: WindowScan) -> Report {
    Report::Compactions {
        sources: dataset.sources,
        rows: dataset.compactions.iter().map(compaction_view).collect(),
        residues: dataset.residues,
    }
}

pub(crate) fn slice_options(matches: &ArgMatches) -> Result<SliceOptions, Refusal> {
    let selected = values(matches, "slice-id")
        .into_iter()
        .collect::<BTreeSet<_>>();
    if selected
        .iter()
        .filter_map(|id| id.strip_prefix("compaction-")?.parse::<usize>().ok())
        .any(|ordinal| ordinal > MAX_SLICE_ORDINAL)
    {
        return Err(refusal(
            "session::bound-exceeded",
            Some(format!("slice ordinal maximum={MAX_SLICE_ORDINAL}")),
            "select a compaction slice within the repository ceiling",
        ));
    }
    Ok(SliceOptions {
        limit: number_allow_zero(matches, "limit", DEFAULT_SLICES_LIMIT)?,
        max_text: number(matches, "max-text", DEFAULT_SLICE_TEXT)?,
        full: one(matches, "view").is_some_and(|view| view == "full"),
        selected,
    })
}

pub(crate) fn slices(dataset: WindowScan, options: SliceOptions) -> Report {
    let mut rows = build_slices(&dataset, options.full, options.max_text);
    rows.reverse();
    if !options.selected.is_empty() {
        rows.retain(|row| options.selected.contains(&row.id));
    }
    rows.truncate(options.limit);
    Report::Slices {
        sources: dataset.sources,
        rows,
        residues: dataset.residues,
    }
}

pub(crate) fn message_options(matches: &ArgMatches) -> Result<MessageOptions, Refusal> {
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
    if max_text > MAX_WINDOW_TEXT {
        return Err(refusal(
            "session::bound-exceeded",
            Some(format!("--max-text={max_text} maximum={MAX_WINDOW_TEXT}")),
            "pass a message text bound within the repository ceiling",
        ));
    }
    let oldest_first = flag(matches, "oldest-first");
    let query = MessageQuery::new(
        filters.start,
        filters.end,
        filters.contains,
        filters.turn_ids,
        sides,
        phases,
        flavors,
        limit,
        oldest_first,
    )?;
    Ok(MessageOptions {
        query,
        max_text,
        oldest_first,
    })
}

pub(crate) fn messages(dataset: MessageDataset, options: MessageOptions) -> Report {
    let mut rows = dataset
        .rows
        .iter()
        .map(|row| {
            let mut view = message_view(row);
            view.text = clip(&view.text, options.max_text);
            view
        })
        .collect::<Vec<_>>();
    if !options.oldest_first {
        rows.reverse();
    }
    Report::Messages {
        sources: dataset.sources,
        rows,
        residues: dataset.residues,
    }
}

pub(crate) fn commits(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMMITS_LIMIT)?;
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for turn in &dataset.turns {
        let user = turn
            .messages
            .iter()
            .find(|message| message.side == "user" && message.authored.is_some())
            .and_then(|message| message.authored.as_deref())
            .map(|message| clip(message, 120))
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
            let Some(authored) = message.authored.as_deref() else {
                continue;
            };
            for line in authored.lines() {
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

fn recall_window(
    index: usize,
    turns: &[&WindowTurn],
    compactions: Vec<&CompactionRecord>,
) -> RecallWindow {
    RecallWindow {
        index,
        start: turns
            .first()
            .map(|turn| turn.view.start.clone())
            .or_else(|| compactions.first().map(|row| row.ts.clone())),
        end: turns
            .last()
            .map(|turn| turn.end.clone())
            .or_else(|| compactions.last().map(|row| row.ts.clone())),
        turns: turns.iter().map(|turn| turn.view.clone()).collect(),
        compactions: compactions.into_iter().map(compaction_view).collect(),
        latest_final: turns
            .iter()
            .rev()
            .find_map(|turn| turn.latest_final.clone()),
        activity: activity(turns.to_vec()),
    }
}

fn activity(turns: Vec<&WindowTurn>) -> ActivityView {
    ActivityView {
        commands: turns.iter().map(|turn| turn.view.commands).sum(),
        patches: turns.iter().map(|turn| turn.view.patches).sum(),
        errors: turns.iter().map(|turn| turn.view.errors).sum(),
        web_searches: turns.iter().map(|turn| turn.view.web_searches).sum(),
    }
}

pub(crate) fn turn_view(
    turn: &TurnRecord,
    full: bool,
    max_text: usize,
    authored_only: bool,
) -> TurnView {
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
        touched_paths: turn.touched_files.iter().cloned().collect(),
        dialogue: turn
            .messages
            .iter()
            .filter_map(|message| {
                let text = if authored_only {
                    message.authored.as_deref()?
                } else {
                    &message.text
                };
                Some(DialogueLine {
                    side: message.side.clone(),
                    text: if full {
                        text.to_owned()
                    } else {
                        clip(text, max_text)
                    },
                })
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
        shape: row.shape,
        flavor: row.primary_flavor.clone(),
        tags: row.flavor_tags.clone(),
        cues: row.matched_cues.clone(),
        text: row.text.clone(),
    }
}

pub(crate) fn authored_message_view(row: &MessageRecord) -> MessageView {
    let mut view = message_view(row);
    view.text = row
        .authored
        .clone()
        .expect("authored message views are selected by authored presence");
    view
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

fn turn_timeline_text(turn: &TurnRecord) -> String {
    let user = turn
        .messages
        .iter()
        .rev()
        .find(|message| message.side == "user" && message.authored.is_some())
        .and_then(|message| message.authored.as_deref())
        .unwrap_or("");
    let assistant = turn
        .messages
        .iter()
        .rev()
        .find(|message| message.side == "assistant" && message.authored.is_some())
        .and_then(|message| message.authored.as_deref())
        .unwrap_or("");
    format!(
        "completed={} cmds={} patches={} errors={} user={} assistant={}",
        turn.completed, turn.command_count, turn.patch_count, turn.error_count, user, assistant
    )
}

fn build_slices(dataset: &WindowScan, full: bool, max_text: usize) -> Vec<SliceView> {
    if dataset.compactions.is_empty() {
        let Some(first) = dataset.turns.first() else {
            return Vec::new();
        };
        let last = dataset
            .turns
            .last()
            .expect("one first turn means one last turn");
        let start = first.view.start.clone();
        let end = last.end.clone();
        return vec![slice_view(
            format!("{}-{}", compact_timestamp(&start), compact_timestamp(&end)),
            first.view.source.clone(),
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
                    .filter(|turn| turn.view.source == compaction.source)
                    .map(|turn| turn.view.start.clone())
                    .min()
            })
            .unwrap_or_else(|| compaction.ts.clone());
        let turns = dataset
            .turns
            .iter()
            .filter(|turn| {
                turn.view.source == compaction.source
                    && turn.end >= start
                    && turn.view.start <= compaction.ts
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
    turns: &[&WindowTurn],
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
        status: if turns.iter().any(|turn| !turn.view.completed) {
            "open"
        } else {
            "closed"
        }
        .to_owned(),
        compactions: usize::from(compaction.is_some()),
        turns: turns
            .iter()
            .filter_map(|turn| turn.view.turn_id.clone())
            .collect(),
        patches: turns.iter().map(|turn| turn.view.patches).sum(),
        crossing_files: turns
            .iter()
            .flat_map(|turn| turn.view.touched_paths.iter())
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

pub(crate) fn turn_end(turn: &TurnRecord) -> String {
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

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Refusal {
    Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("session-query".to_owned()),
            Some("one admitted bounded transcript query".to_owned()),
            observed,
        )
        .with_detail(detail),
    )
}
