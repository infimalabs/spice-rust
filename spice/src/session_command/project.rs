use std::collections::BTreeSet;
use std::path::Path;

use clap::ArgMatches;
use wire::output::{Admission, Bounded, Limits, Request, Surface};
use wire::{Refusal, SpiceCommand};

use super::authority::RecallAuthorities;
use super::dataset::{
    self, Aggregate, CommitScan, CompactionScan, MessageFilter, MessageQuery, Recent,
    WindowOptions, WindowScan, WindowTurn,
};
use super::model::{
    ActivityView, CompactionRecord, CompactionView, DialogueLine, LiveTurnView, LiveView,
    MessageRecord, MessageView, OutstandingAskView, RecallView, RecentTurnView, Report, SliceView,
    SweepWindow, TurnRecord, TurnView,
};
use super::source::ResolvedSource;
use crate::read_page::{self as page, Page, Paged};

// Two report families are split out by `mod` rather than left inline, following
// the decision recorded at `board_command.rs`: a child module already reads its
// ancestor's private items, so the seam costs no visibility widening. Each is
// named for the question it answers, and each re-exports its entry point so the
// dispatch table in `session_command.rs` still reads as one column of reports.
mod command;
mod phase;
mod timeline;

pub(crate) use command::commands;
pub(crate) use phase::{PhaseOptions, phase_example, phases, turn_family};
pub(crate) use timeline::{
    compaction_view as timeline_compaction_view, report as timeline,
    turn_view as timeline_turn_view,
};

/// Pending asks a briefing carries, each cut to a readable preview. Every
/// pending ask is shown whole by `spice inbox LANE`.
const ASKS: Surface = Surface::new(ASK_LIMITS, ASK_LIMITS);
const ASK_LIMITS: Limits = Limits {
    rows: 16,
    bytes: 16_384,
    text: 1_024,
};
/// Bytes of the live prompt and latest final answer a briefing carries.
const LIVE_TEXT: usize = 480;
/// Compactions a sweep retains after its horizon; the page itself is bounded
/// by its row and byte limits.
const SWEEP_COMPACTIONS: usize = 4 * 1_024;
const MAX_SLICE_ORDINAL: usize = 4 * 1_024;

struct SweepInterval<'a> {
    start: Option<&'a str>,
    end: Option<&'a str>,
    anchor: Option<&'a CompactionRecord>,
}

#[derive(Clone, Debug, Default)]
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

    /// The same filters ending at `end`.
    fn ending(self, end: Option<String>) -> Self {
        Self { end, ..self }
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

    pub(crate) fn timeline_compaction_matches(&self, row: &CompactionRecord) -> bool {
        if !self.turn_ids.is_empty() || !self.tools.is_empty() {
            return false;
        }
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

/// The briefing: what binds this lane now, then the least transcript that
/// lets it resume. History stays behind the bounded views it names.
pub(crate) fn recall(
    sources: &[ResolvedSource],
    files: &[String],
    authorities: RecallAuthorities,
) -> Result<Report, Refusal> {
    let horizon = dataset::recent_compaction_horizon(sources, 1, None)?;
    // One compaction further back, so a briefing read right after a
    // compaction still recalls the turns that compaction folded away.
    let reach = dataset::recent_compaction_horizon(sources, RECENT_COMPACTIONS, None)?;
    let data = WindowScan::scan(
        sources,
        reach.start.as_deref(),
        None,
        &Filters::default(),
        WindowOptions::Live,
    )?;
    let recent = data
        .turns
        .iter()
        .rev()
        .take(RECENT_TURNS)
        .collect::<Vec<_>>();
    let mut touched = Vec::<String>::new();
    for turn in &recent {
        for path in turn.view.touched_paths.iter().rev() {
            if touched.len() < RECENT_PATHS && !touched.contains(path) {
                touched.push(path.clone());
            }
        }
    }
    let live = LiveView {
        turn: data.turns.last().map(|turn| LiveTurnView {
            start: turn.view.start.clone(),
            end: turn.view.end.clone(),
            completed: turn.view.completed,
            commands: turn.view.commands,
            patches: turn.view.patches,
            errors: turn.view.errors,
            prompt: turn
                .prompt
                .as_deref()
                .map(|prompt| page::clip(prompt, LIVE_TEXT)),
        }),
        latest_final: data
            .turns
            .iter()
            .rev()
            .find_map(|turn| turn.latest_final.as_deref())
            .map(|text| page::clip(text, LIVE_TEXT)),
        compacted: horizon.start,
        recent: recent
            .iter()
            .map(|turn| RecentTurnView {
                start: turn.view.start.clone(),
                completed: turn.view.completed,
                commands: turn.view.commands,
                patches: turn.view.patches,
                errors: turn.view.errors,
                final_answer: turn
                    .latest_final
                    .as_deref()
                    .map(|text| page::clip(text, RECENT_FINAL_TEXT)),
            })
            .collect(),
        touched,
        history: ["sweep", "turns"]
            .into_iter()
            .map(|view| session_command(view, files, []))
            .collect(),
    };
    let lane = authorities.held_work.lane.clone();
    let asks = authorities
        .asks
        .map(|pending| {
            let mut asks = Admission::new(
                ASKS.budget(Request::default())
                    .expect("the briefing ask surface admits its own defaults"),
            );
            for ask in pending {
                asks.offer(OutstandingAskView {
                    directive: ask.directive,
                    key: ask.key,
                    stage: ask.stage,
                    text: page::clip(&ask.text, ASK_LIMITS.text),
                    attachments: ask.attachments,
                    authored: ask.authored,
                    expires: ask.expires,
                })
                .map_err(unencodable)?;
            }
            Ok(asks
                .finish(|_| SpiceCommand::new("inbox", lane).expect("inbox is one Spice command")))
        })
        .transpose()?;
    let mut recall = Box::new(RecallView {
        asks,
        held_work: authorities.held_work,
        posture: authorities.posture,
        peers: authorities.peers,
        directions: authorities.directions,
        board: authorities.board,
        repository: authorities.repository,
        unknowns: authorities.unknowns,
        live,
        sources: data.sources,
        residues: data.residues.len(),
        digest: String::new(),
    });
    recall.digest = super::digest::fold(&recall, files);
    Ok(Report::Recall(recall))
}

/// Compactions a briefing reaches back across for recent turns.
const RECENT_COMPACTIONS: usize = 2;
/// Recent turns a briefing recalls as outcomes.
const RECENT_TURNS: usize = 6;
/// Paths those turns touched that a briefing names.
const RECENT_PATHS: usize = 16;
/// Bytes of one recent turn's final answer.
const RECENT_FINAL_TEXT: usize = 360;

/// One row per compaction window, newest first, each summarizing its turns
/// and naming the bounded read of them.
pub(crate) fn sweep(
    sources: &[ResolvedSource],
    files: &[String],
    page: Page,
) -> Result<Report, Refusal> {
    let through = page.end(None)?;
    let count = page.frontier()?;
    let horizon = dataset::counted_compaction_horizon(sources, count, through.as_deref())?;
    let total = horizon
        .total
        .expect("a counted compaction horizon counts every compaction");
    let data = WindowScan::scan(
        sources,
        horizon.start.as_deref(),
        None,
        &Filters::default(),
        WindowOptions::Sweep {
            compactions: SWEEP_COMPACTIONS,
        },
    )?;
    let rows = sweep_intervals(&data.compactions, through.as_deref(), count)
        .iter()
        .rev()
        .filter_map(|interval| sweep_window(interval, &data.turns, files, &page))
        .collect::<Vec<_>>();
    let population = if total == 0 { rows.len() } else { total };
    let paged = page.newest(rows, population, |row| &row.start)?;
    Ok(report(paged, |page| Report::Sweep {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

fn sweep_intervals<'a>(
    compactions: &'a [CompactionRecord],
    through: Option<&str>,
    count: usize,
) -> Vec<SweepInterval<'a>> {
    if compactions.is_empty() {
        return vec![SweepInterval {
            start: None,
            end: None,
            anchor: None,
        }];
    }
    let split = compactions.partition_point(|row| through.is_none_or(|end| row.ts.as_str() <= end));
    let closing = compactions.get(split).map(|row| row.ts.as_str());
    let selected = &compactions[split.saturating_sub(count)..split];
    selected
        .iter()
        .enumerate()
        .map(|(index, row)| SweepInterval {
            start: Some(row.ts.as_str()),
            end: selected
                .get(index + 1)
                .map(|next| next.ts.as_str())
                .or(closing),
            anchor: Some(row),
        })
        .collect()
}

fn sweep_window(
    interval: &SweepInterval<'_>,
    turns: &[WindowTurn],
    files: &[String],
    page: &Page,
) -> Option<SweepWindow> {
    let turns = turns
        .iter()
        .filter(|turn| {
            interval
                .start
                .is_none_or(|start| turn.end.as_str() >= start)
                && interval
                    .end
                    .is_none_or(|end| turn.view.start.as_str() < end)
        })
        .collect::<Vec<_>>();
    let start = interval
        .start
        .map(str::to_owned)
        .or_else(|| turns.first().map(|turn| turn.view.start.clone()))?;
    // `turns --end` admits turns starting at or before it, so the window's
    // last turn start bounds exactly the turns this window counted.
    let last = turns
        .last()
        .map_or(start.as_str(), |turn| turn.view.start.as_str());
    let detail = vec![
        "--start".to_owned(),
        start.clone(),
        "--end".to_owned(),
        last.to_owned(),
    ];
    Some(SweepWindow {
        compaction: interval.anchor.map(|row| compaction_view(row, page)),
        end: turns.last().map(|turn| turn.end.clone()),
        turns: turns.len(),
        open: turns.iter().any(|turn| !turn.view.completed),
        activity: ActivityView {
            commands: turns.iter().map(|turn| turn.view.commands).sum(),
            patches: turns.iter().map(|turn| turn.view.patches).sum(),
            errors: turns.iter().map(|turn| turn.view.errors).sum(),
            web_searches: turns.iter().map(|turn| turn.view.web_searches).sum(),
        },
        latest_final: turns
            .iter()
            .rev()
            .find_map(|turn| turn.latest_final.as_deref())
            .map(|text| page.clip(text)),
        detail: session_command("turns", files, detail),
        start,
    })
}

/// One page of turns, newest first.
pub(crate) fn turns(
    sources: &[ResolvedSource],
    matches: &ArgMatches,
    page: Page,
) -> Result<Report, Refusal> {
    let filters = Filters::from_matches(matches)?;
    let end = page.end(filters.end_bound())?;
    let filters = filters.ending(end.clone());
    let data = WindowScan::scan(
        sources,
        None,
        end.as_deref(),
        &filters,
        WindowOptions::Turns {
            limit: page.frontier()?,
            full: one(matches, "view").is_some_and(|view| view == "full"),
            max_text: page.limits().text,
        },
    )?;
    let rows = data.turns.into_iter().rev().map(|turn| turn.view).collect();
    let paged = page.newest(rows, data.population, |row| &row.start)?;
    Ok(report(paged, |page| Report::Turns {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

/// One page of compactions, newest first.
pub(crate) fn compactions(sources: &[ResolvedSource], page: Page) -> Result<Report, Refusal> {
    let end = page.end(None)?;
    let limit = page.frontier()?;
    let horizon = dataset::counted_compaction_horizon(sources, limit, end.as_deref())?;
    let population = horizon
        .total
        .expect("a counted compaction horizon counts every compaction");
    let data = CompactionScan::scan(sources, horizon, end.as_deref(), limit)?;
    let rows = data
        .compactions
        .iter()
        .rev()
        .map(|row| compaction_view(row, &page))
        .collect();
    let paged = page.newest(rows, population, |row| &row.ts)?;
    Ok(report(paged, |page| Report::Compactions {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

/// One page of compaction slices, newest first.
pub(crate) fn slices(
    sources: &[ResolvedSource],
    matches: &ArgMatches,
    page: Page,
) -> Result<Report, Refusal> {
    let selected = values(matches, "slice-id")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let newest_selected = selected.iter().filter_map(|id| slice_ordinal(id)).max();
    if newest_selected.is_some_and(|ordinal| ordinal > MAX_SLICE_ORDINAL) {
        return Err(refusal(
            "session::bound-exceeded",
            Some(format!("slice ordinal maximum={MAX_SLICE_ORDINAL}")),
            "select a compaction slice within the repository ceiling",
        ));
    }
    let end = page.end(None)?;
    let (start, count, oldest, total) = if selected.is_empty() {
        let count = page.frontier()?;
        let horizon = dataset::counted_compaction_horizon(sources, count, end.as_deref())?;
        let total = horizon
            .total
            .expect("a counted compaction horizon counts every compaction");
        (horizon.start, count, false, total)
    } else {
        (None, newest_selected.unwrap_or(1), true, 0)
    };
    let data = WindowScan::scan(
        sources,
        start.as_deref(),
        end.as_deref(),
        &Filters::default().ending(end.clone()),
        WindowOptions::Slices {
            compactions: count,
            oldest,
        },
    )?;
    let ordinal_base = total.saturating_sub(data.compactions.len());
    let full = one(matches, "view").is_some_and(|view| view == "full");
    let mut rows = build_slices(&data, full, &page, ordinal_base);
    rows.reverse();
    rows.retain(|row| selected.is_empty() || selected.contains(&row.id));
    let population = if oldest || total == 0 {
        rows.len()
    } else {
        total
    };
    let paged = page.newest(rows, population, |row| &row.end)?;
    Ok(report(paged, |page| Report::Slices {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

fn slice_ordinal(id: &str) -> Option<usize> {
    id.strip_prefix("compaction-")?.parse().ok()
}

/// One page of messages, newest first.
pub(crate) fn messages(
    sources: &[ResolvedSource],
    matches: &ArgMatches,
    page: Page,
) -> Result<Report, Refusal> {
    let filters = Filters::from_matches(matches)?;
    let end = page.end(filters.end_bound())?;
    let query = MessageQuery::new(
        MessageFilter {
            start: filters.start,
            end,
            contains: filters.contains,
            turn_ids: filters.turn_ids,
            sides: values(matches, "side").into_iter().collect(),
            phases: values(matches, "phase-kind").into_iter().collect(),
            flavors: values(matches, "flavor").into_iter().collect(),
        },
        page.frontier()?,
    )?;
    let data = dataset::messages(sources, &query)?;
    let rows = data
        .rows
        .iter()
        .rev()
        .map(|row| message_view(row, &row.text, &page))
        .collect();
    let paged = page.newest(rows, data.population, |row| &row.ts)?;
    Ok(report(paged, |page| Report::Messages {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

/// One page of first-mentioned commits, newest first.
pub(crate) fn commits(sources: &[ResolvedSource], page: Page) -> Result<Report, Refusal> {
    let end = page.end(None)?;
    let data = CommitScan::scan(
        sources,
        end.as_deref(),
        page.frontier()?,
        page.limits().text,
    )?;
    let rows = data.rows.into_iter().rev().collect();
    let paged = page.newest(rows, data.population, |row| &row.ts)?;
    Ok(report(paged, |page| Report::Commits {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

/// Whole-transcript counts and one page of recent prompts and final answers.
pub(crate) fn summary(sources: &[ResolvedSource], page: Page) -> Result<Report, Refusal> {
    let recent = Recent::new(page.frontier()?, page.end(None)?);
    Aggregate::scan(sources, recent)?.summary(page)
}

/// A shown page as its report, or the receipt of an exported one.
fn report<T>(paged: Paged<T>, build: impl FnOnce(Bounded<T>) -> Report) -> Report {
    match paged {
        Paged::Shown(page) => build(page),
        Paged::Exported(export) => Report::Exported { export },
    }
}

/// `spice session VIEW FILES... ARGS...`.
fn session_command(
    view: &str,
    files: &[String],
    arguments: impl IntoIterator<Item = String>,
) -> SpiceCommand {
    SpiceCommand::new(
        "session",
        std::iter::once(view.to_owned())
            .chain(files.iter().cloned())
            .chain(arguments),
    )
    .expect("a session view is one Spice command")
}

/// One turn; with `dialogue`, its messages (authored text only when the flag
/// is set) cut to the text bound.
pub(crate) fn turn_view(turn: &TurnRecord, dialogue: Option<(bool, usize)>) -> TurnView {
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
        dialogue: dialogue
            .map(|(authored_only, max_text)| {
                turn.messages
                    .iter()
                    .filter_map(|message| {
                        let text = if authored_only {
                            message.authored.as_deref()?
                        } else {
                            &message.text
                        };
                        Some(DialogueLine {
                            side: message.side.clone(),
                            text: page::clip(text, max_text),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn message_view(row: &MessageRecord, text: &str, page: &Page) -> MessageView {
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
        text: page.clip(text),
    }
}

pub(crate) fn authored_message_view(row: &MessageRecord, page: &Page) -> MessageView {
    message_view(
        row,
        row.authored
            .as_deref()
            .expect("authored message views are selected by authored presence"),
        page,
    )
}

fn compaction_view(row: &CompactionRecord, page: &Page) -> CompactionView {
    CompactionView {
        source: row.source.clone(),
        ts: row.ts.clone(),
        assistant_before: page.clip(&row.assistant_before),
        summary_after: page.clip(&row.summary_after),
        intent: page.clip(&row.intent),
        user_after: page.clip(&row.user_after),
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

fn build_slices(
    dataset: &WindowScan,
    full: bool,
    page: &Page,
    ordinal_base: usize,
) -> Vec<SliceView> {
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
            SliceSpan {
                id: format!("{}-{}", compact_timestamp(&start), compact_timestamp(&end)),
                source: first.view.source.clone(),
                start,
                end,
                basis: "full_history_no_compactions",
                anchor: "full_history",
            },
            &dataset.turns.iter().collect::<Vec<_>>(),
            None,
            full,
            page,
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
            SliceSpan {
                id: format!("compaction-{}", ordinal_base + index + 1),
                source: compaction.source.clone(),
                start,
                end: compaction.ts.clone(),
                basis: "compaction_interval",
                anchor: "compaction",
            },
            &turns,
            Some(compaction),
            full,
            page,
        ));
    }
    rows
}

struct SliceSpan {
    id: String,
    source: String,
    start: String,
    end: String,
    basis: &'static str,
    anchor: &'static str,
}

fn slice_view(
    span: SliceSpan,
    turns: &[&WindowTurn],
    compaction: Option<&CompactionRecord>,
    full: bool,
    page: &Page,
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
                    text: page.clip(text),
                });
            }
        }
    }
    SliceView {
        id: span.id,
        source: span.source,
        start: span.start,
        end: span.end,
        basis: span.basis.to_owned(),
        anchor: span.anchor.to_owned(),
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

pub(crate) fn turn_end(turn: &TurnRecord) -> String {
    turn.end_ts
        .clone()
        .unwrap_or_else(|| turn.last_activity_ts.clone())
}

pub(crate) fn tokens_by_file(matches: &ArgMatches) -> bool {
    flag(matches, "by-file")
}

fn optional_bound(matches: &ArgMatches, name: &str) -> Result<Option<String>, Refusal> {
    one(matches, name)
        .map(|value| normalize_bound(value))
        .transpose()
}

pub(crate) fn normalize_bound(value: &str) -> Result<String, Refusal> {
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

fn unencodable(error: serde_json::Error) -> Refusal {
    refusal(
        "session::record-unencodable",
        Some(error.to_string()),
        "report the transcript record that cannot be encoded",
    )
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
