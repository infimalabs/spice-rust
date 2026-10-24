//! Bounded retained state for complete-scan session aggregates.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use wire::Refusal;

use super::super::model::{
    ActivityView, CompactionRecord, FoldBuffer, MessageRecord, PhaseExample, PhaseView, Report,
    Residue, SourceView, TokenRecord, TokenView, TurnRecord, TurnView, Vendor,
};
use super::super::project::{
    Filters, PhaseOptions, authored_message_view, phase_example, turn_end, turn_family, turn_view,
};
use super::super::source::ResolvedSource;
use super::stream::{ForwardCursor, ForwardProgress, compressed};
use super::{
    CompactionTracker, DecodedRow, FoldState, FoldStorage, FrontierDirection, Marker,
    MessageFilter, MessageQuery, OPEN_TURN_LIMIT, ResidueLedger, ResultFrontier, SessionWork,
    SessionWorkBudget, admit_source_order, finish_fold, message_text_bytes, reduce_record, refusal,
    source_view_text_bytes, stream_records,
};
use crate::read_page::{Page, Paged};

mod commit;
mod recent;
mod thread;
pub(crate) mod timeline;
pub(crate) use commit::CommitScan;
pub(crate) use recent::Recent;
const PHASE_GAP_SECONDS: i64 = 45 * 60;
const PHASE_PATH_LIMIT: usize = 4_096;
const SOURCE_LIMIT: usize = 64;
const MAX_MESSAGE_IDENTITIES: usize = 4 * 1_024;
const WINDOW_TURN_LIMIT: usize = 4 * 1_024;

impl MessageFilter {
    fn matches(&self, row: &MessageRecord) -> bool {
        self.start.as_ref().is_none_or(|start| row.ts >= *start)
            && self.end.as_ref().is_none_or(|end| row.ts <= *end)
            && self
                .contains
                .as_ref()
                .is_none_or(|needle| row.text.to_lowercase().contains(needle))
            && (self.turn_ids.is_empty()
                || row
                    .turn_id
                    .as_ref()
                    .is_some_and(|turn_id| self.turn_ids.contains(turn_id)))
            && (self.sides.is_empty() || self.sides.contains(&row.side))
            && (self.phases.is_empty() || self.phases.contains(&row.phase))
            && (self.flavors.is_empty()
                || self.flavors.contains(&row.primary_flavor)
                || row.flavor_tags.iter().any(|tag| self.flavors.contains(tag)))
    }
}

pub(super) struct MessageSourceScan {
    pub(super) rows: Vec<MessageRecord>,
    pub(super) offered: usize,
    pub(super) residues: Vec<Residue>,
    pub(super) vendor: Vendor,
}

/// Turns of the live transcript a briefing retains: the newest, and the one
/// before it so an open turn still has the latest final answer beside it.
const LIVE_TURNS: usize = 2;

#[derive(Clone, Copy)]
pub(crate) enum WindowOptions {
    /// The newest turns after the latest compaction, without dialogue.
    Live,
    Sweep {
        compactions: usize,
    },
    Slices {
        compactions: usize,
        oldest: bool,
    },
    Turns {
        limit: usize,
        full: bool,
        max_text: usize,
    },
}

impl WindowOptions {
    pub(crate) const fn compactions(self) -> usize {
        match self {
            Self::Sweep { compactions } | Self::Slices { compactions, .. } => compactions,
            Self::Live | Self::Turns { .. } => 1,
        }
    }

    const fn scans_full_source(self) -> bool {
        matches!(self, Self::Slices { oldest: true, .. })
    }

    const fn compaction_direction(self) -> FrontierDirection {
        match self {
            Self::Slices { oldest: true, .. } => FrontierDirection::Oldest,
            _ => FrontierDirection::Newest,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WindowTurn {
    pub(crate) view: TurnView,
    pub(crate) end: String,
    pub(crate) prompt: Option<String>,
    pub(crate) latest_final: Option<String>,
}

impl WindowTurn {
    fn project(turn: TurnRecord, options: WindowOptions) -> Self {
        let latest_final = turn.messages.iter().rev().find_map(|message| {
            (message.phase == "final_answer")
                .then(|| message.authored.clone())
                .flatten()
        });
        let prompt = turn
            .messages
            .iter()
            .find(|message| message.side == "user")
            .and_then(|message| message.authored.clone());
        let end = turn_end(&turn);
        let view = match options {
            WindowOptions::Turns { full, max_text, .. } => {
                turn_view(&turn, Some((!full, max_text)))
            }
            WindowOptions::Live | WindowOptions::Sweep { .. } | WindowOptions::Slices { .. } => {
                turn_view(&turn, None)
            }
        };
        Self {
            view,
            end,
            prompt,
            latest_final,
        }
    }
}

pub(crate) struct WindowScan {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) turns: Vec<WindowTurn>,
    pub(crate) compactions: Vec<CompactionRecord>,
    pub(crate) residues: Vec<Residue>,
    /// Every turn the scan matched, retained or not.
    pub(crate) population: usize,
}

impl WindowScan {
    pub(crate) fn scan(
        sources: &[ResolvedSource],
        start: Option<&str>,
        end: Option<&str>,
        filters: &Filters,
        options: WindowOptions,
    ) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut residues = ResidueLedger::new();
        let cursor_start = (!options.scans_full_source()).then_some(start).flatten();
        let mut streams = sources
            .iter()
            .map(|source| {
                PhaseSource::window(
                    source,
                    cursor_start,
                    start,
                    end,
                    options.compactions(),
                    options.compaction_direction(),
                    &mut work,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut turns = WindowTurnFrontier::new(options);
        let mut any_turn_identity = false;
        merge_ready_turns(&mut streams, &mut residues, &mut work, |turn| {
            any_turn_identity |= turn.turn_id.is_some();
            if filters.turn_matches(&turn) {
                turns.push(WindowTurn::project(turn, options))?;
            }
            Ok(())
        })?;
        filters.validate_turn_identity_available(any_turn_identity)?;
        let mut sources = Vec::with_capacity(streams.len());
        let mut compaction_sources = Vec::with_capacity(streams.len());
        for stream in streams {
            let (source, compactions, _) = stream.into_window(&mut work)?;
            work.admit_retained_text(source_view_text_bytes(&source), &source.path)?;
            sources.push(source);
            compaction_sources.push(VecDeque::from(compactions));
        }
        let compactions = merge_compactions(
            compaction_sources,
            options.compactions(),
            options.compaction_direction(),
        )?;
        Ok(Self {
            sources,
            population: turns.population,
            turns: turns.rows.into(),
            compactions,
            residues: residues.finish(&mut work, "window residue summary")?,
        })
    }
}

struct WindowTurnFrontier {
    options: WindowOptions,
    rows: VecDeque<WindowTurn>,
    population: usize,
}

impl WindowTurnFrontier {
    fn new(options: WindowOptions) -> Self {
        Self {
            options,
            rows: VecDeque::new(),
            population: 0,
        }
    }

    fn push(&mut self, turn: WindowTurn) -> Result<(), Refusal> {
        let retained = match self.options {
            WindowOptions::Live => LIVE_TURNS,
            WindowOptions::Turns { limit, .. } => limit,
            WindowOptions::Sweep { .. } | WindowOptions::Slices { .. } => {
                if self.rows.len() == WINDOW_TURN_LIMIT {
                    return Err(refusal(
                        "session::window-turn-limit",
                        Some(format!("maximum={WINDOW_TURN_LIMIT}")),
                        "narrow the requested compaction windows",
                    ));
                }
                WINDOW_TURN_LIMIT
            }
        };
        self.population = self.population.saturating_add(1);
        self.rows.push_back(turn);
        if self.rows.len() > retained {
            self.rows.pop_front();
        }
        Ok(())
    }
}

pub(super) fn merge_compactions(
    mut sources: Vec<VecDeque<CompactionRecord>>,
    limit: usize,
    direction: FrontierDirection,
) -> Result<Vec<CompactionRecord>, Refusal> {
    let mut rows = ResultFrontier::new(limit, direction)?;
    while let Some(index) = sources
        .iter()
        .enumerate()
        .filter(|(_, source)| !source.is_empty())
        .min_by_key(|(_, source)| {
            let row = source
                .front()
                .expect("filtered compaction source is nonempty");
            (&row.ts, &row.source)
        })
        .map(|(index, _)| index)
    {
        rows.push(
            sources[index]
                .pop_front()
                .expect("selected compaction source is nonempty"),
        );
    }
    Ok(rows.into_rows())
}

pub(super) fn source_view(source: &ResolvedSource, vendor: Vendor) -> SourceView {
    let path = source.path.display().to_string();
    SourceView {
        name: source
            .path
            .file_name()
            .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
        path,
        vendor,
        thread: source.thread.clone(),
        compressed: compressed(&source.path),
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum MessageIdentity {
    Final {
        turn: Option<String>,
        generation: u64,
        text: String,
    },
    Other {
        turn: Option<String>,
        generation: u64,
        ts: String,
        side: String,
        phase: String,
        text: String,
    },
}

struct MessageDedup {
    identities: BTreeSet<MessageIdentity>,
    generation: u64,
}

impl MessageDedup {
    fn new() -> Self {
        Self {
            identities: BTreeSet::new(),
            generation: 0,
        }
    }

    fn begin(&mut self, moment: Option<listen::TurnMoment>) {
        if moment == Some(listen::TurnMoment::Started) {
            self.generation = self.generation.saturating_add(1);
        }
    }

    fn finish(&mut self, moment: Option<listen::TurnMoment>) {
        if moment == Some(listen::TurnMoment::Completed) {
            self.generation = self.generation.saturating_add(1);
        }
    }

    fn admit(
        &mut self,
        message: &MessageRecord,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<bool, Refusal> {
        work.admit_retained_text(message_text_bytes(message), at)?;
        let identity = if message.side == "assistant" && message.phase == "final_answer" {
            MessageIdentity::Final {
                turn: message.turn_id.clone(),
                generation: self.generation,
                text: message.text.clone(),
            }
        } else {
            MessageIdentity::Other {
                turn: message.turn_id.clone(),
                generation: self.generation,
                ts: message.ts.clone(),
                side: message.side.clone(),
                phase: message.phase.clone(),
                text: message.text.clone(),
            }
        };
        if self.identities.contains(&identity) {
            return Ok(false);
        }
        if self.identities.len() == MAX_MESSAGE_IDENTITIES {
            return Err(refusal(
                "session::message-identity-limit",
                Some(format!("limit={MAX_MESSAGE_IDENTITIES} at={at:?}")),
                "narrow the message window or filters and retry",
            ));
        }
        self.identities.insert(identity);
        Ok(true)
    }
}

pub(super) fn scan_message_source(
    source: &ResolvedSource,
    start: Option<&str>,
    query: &MessageQuery,
    work: &mut SessionWork,
) -> Result<MessageSourceScan, Refusal> {
    let mut frontier = ResultFrontier::new(query.limit.get(), FrontierDirection::Newest)?;
    let mut cursor = ForwardCursor::new(source, start, work)?;
    let mut scratch = FoldBuffer::default();
    let mut markers = Vec::new();
    let mut residues = ResidueLedger::new();
    let mut state = FoldState::default();
    let mut dedup = MessageDedup::new();
    loop {
        let window = cursor.advance(work)?;
        if let Some(row) = window.row {
            let at = format!(
                "{}:record {}",
                source.path.display(),
                cursor.continuation().line
            );
            let moment = match row {
                DecodedRow::Typed(record) => {
                    let moment = record.context.turn;
                    dedup.begin(moment);
                    reduce_record(
                        &record,
                        &mut FoldStorage {
                            dataset: &mut scratch,
                            markers: &mut markers,
                            residues: &mut residues,
                            state: &mut state,
                            tokens: None,
                        },
                        work,
                        &at,
                    )?;
                    moment
                }
                DecodedRow::Residue(residue) => {
                    residues.push(residue, work, &at)?;
                    None
                }
                DecodedRow::Ignored => None,
            };
            markers.clear();
            for message in scratch.messages.drain(..) {
                if dedup.admit(&message, work, &at)? && query.filter.matches(&message) {
                    frontier.push(message);
                }
            }
            for turn in &mut scratch.turns {
                turn.messages.clear();
            }
            dedup.finish(moment);
            drain_fold_turns(
                &mut scratch,
                &mut state,
                false,
                &source.path.display().to_string(),
                |_| Ok(()),
            )?;
        }
        if matches!(window.progress, ForwardProgress::Eof(_)) {
            let state = std::mem::take(&mut state);
            finish_fold(
                &mut scratch,
                state,
                work,
                &source.path.display().to_string(),
            )?;
            break;
        }
    }
    Ok(MessageSourceScan {
        offered: frontier.offered(),
        rows: frontier.into_rows(),
        residues: residues.finish(work, "message source residue summary")?,
        vendor: cursor.vendor(),
    })
}

pub(crate) struct Aggregate {
    sources: Vec<SourceView>,
    turns: usize,
    completed: usize,
    compactions: usize,
    activity: ActivityView,
    recent: Recent,
    tokens: BTreeMap<String, TokenAggregate>,
    start: Option<String>,
    end: Option<String>,
    latest: Option<TurnRecord>,
    phase: Option<PhaseState>,
    any_turn_identity: bool,
    residues: Vec<super::super::model::Residue>,
}

/// Every phase a scan numbered at or before its cursor, newest retained first.
pub(crate) struct PhaseScan {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) rows: Vec<PhaseView>,
    pub(crate) population: usize,
    pub(crate) residues: Vec<Residue>,
}

#[derive(Clone, Default)]
struct TokenAggregate {
    total: u64,
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    context: Option<u64>,
    snapshots: u64,
    first: Option<String>,
    last: Option<String>,
}

impl Aggregate {
    pub(crate) fn scan(sources: &[ResolvedSource], recent: Recent) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        let mut aggregate = Self {
            sources: Vec::with_capacity(sources.len()),
            turns: 0,
            completed: 0,
            compactions: 0,
            activity: ActivityView {
                commands: 0,
                patches: 0,
                errors: 0,
                web_searches: 0,
            },
            recent,
            tokens: BTreeMap::new(),
            start: None,
            end: None,
            latest: None,
            phase: None,
            any_turn_identity: false,
            residues: Vec::new(),
        };
        let mut residues = ResidueLedger::new();
        let mut work = SessionWork::new(SessionWorkBudget::report());
        for source in sources {
            aggregate.scan_source(source, &mut residues, &mut work)?;
        }
        aggregate.residues = residues.finish(&mut work, "aggregate residue summary")?;
        Ok(aggregate)
    }

    /// Every phase at or before the cursor, newest retained first.
    pub(crate) fn scan_phases(
        sources: &[ResolvedSource],
        options: PhaseOptions,
        filters: Filters,
    ) -> Result<PhaseScan, Refusal> {
        let mut aggregate = Self::scan_with_phase(sources, options, filters)?;
        let phase = aggregate
            .phase
            .take()
            .expect("phase scan retains phase state");
        phase
            .filters
            .validate_turn_identity_available(aggregate.any_turn_identity)?;
        let (rows, population) = phase.finish()?;
        Ok(PhaseScan {
            sources: aggregate.sources,
            rows,
            population,
            residues: aggregate.residues,
        })
    }

    fn scan_with_phase(
        sources: &[ResolvedSource],
        options: PhaseOptions,
        filters: Filters,
    ) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        let mut aggregate = Self {
            sources: Vec::with_capacity(sources.len()),
            turns: 0,
            completed: 0,
            compactions: 0,
            activity: ActivityView {
                commands: 0,
                patches: 0,
                errors: 0,
                web_searches: 0,
            },
            recent: Recent::NONE,
            tokens: BTreeMap::new(),
            start: None,
            end: None,
            latest: None,
            phase: Some(PhaseState::new(options, filters)?),
            any_turn_identity: false,
            residues: Vec::new(),
        };
        let mut residues = ResidueLedger::new();
        let mut work = SessionWork::new(SessionWorkBudget::report());
        aggregate.scan_phase_sources(sources, &mut residues, &mut work)?;
        aggregate.residues = residues.finish(&mut work, "phase residue summary")?;
        Ok(aggregate)
    }

    fn scan_phase_sources(
        &mut self,
        sources: &[ResolvedSource],
        residues: &mut ResidueLedger,
        work: &mut SessionWork,
    ) -> Result<(), Refusal> {
        let mut streams = sources
            .iter()
            .map(|source| PhaseSource::new(source, work))
            .collect::<Result<Vec<_>, _>>()?;
        merge_ready_turns(&mut streams, residues, work, |turn| self.add_turn(turn))?;
        for stream in streams {
            let source = stream.source_view();
            work.admit_retained_text(source_view_text_bytes(&source), &source.path)?;
            self.sources.push(source);
        }
        Ok(())
    }

    fn scan_source(
        &mut self,
        source: &ResolvedSource,
        residues: &mut ResidueLedger,
        work: &mut SessionWork,
    ) -> Result<(), Refusal> {
        let source_name = source.path.display().to_string();
        let mut scratch = super::super::model::FoldBuffer::default();
        let mut tokens = Vec::new();
        let mut markers = Vec::<Marker>::new();
        let mut fold = FoldState::default();
        let vendor = stream_records(source, None, work, |row, work, at| {
            match row {
                DecodedRow::Typed(record) => {
                    let before = markers.len();
                    reduce_record(
                        &record,
                        &mut FoldStorage {
                            dataset: &mut scratch,
                            markers: &mut markers,
                            residues,
                            state: &mut fold,
                            tokens: Some(&mut tokens),
                        },
                        work,
                        at,
                    )?;
                    let added = markers.len() - before;
                    self.compactions = self.compactions.saturating_add(added);
                    if added > 0
                        && let Some(turn) = fold
                            .current
                            .or_else(|| scratch.turns.len().checked_sub(1))
                            .and_then(|index| scratch.turns.get_mut(index))
                    {
                        turn.compaction_count = turn.compaction_count.saturating_add(added as u64);
                    }
                    markers.clear();
                    self.drain_scratch(&mut scratch, &mut tokens, &mut fold, false, at)?;
                }
                DecodedRow::Residue(residue) => residues.push(residue, work, at)?,
                DecodedRow::Ignored => {}
            }
            Ok(())
        })?;
        finish_fold(&mut scratch, fold, work, &source_name)?;
        let mut finished = FoldState::default();
        self.drain_scratch(&mut scratch, &mut tokens, &mut finished, true, &source_name)?;
        let source_view = SourceView {
            name: source.path.file_name().map_or_else(
                || source_name.clone(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: source_name,
            vendor,
            thread: source.thread.clone(),
            compressed: source.path.to_string_lossy().ends_with(".gz"),
        };
        work.admit_retained_text(source_view_text_bytes(&source_view), &source_view.path)?;
        self.sources.push(source_view);
        Ok(())
    }

    fn drain_scratch(
        &mut self,
        scratch: &mut super::super::model::FoldBuffer,
        tokens: &mut Vec<TokenRecord>,
        fold: &mut FoldState,
        finish: bool,
        at: &str,
    ) -> Result<(), Refusal> {
        for token in tokens.drain(..) {
            self.tokens
                .entry(token.source.clone())
                .or_default()
                .add(&token);
        }
        for message in scratch.messages.drain(..) {
            let recent = message.authored.is_some()
                && (message.side == "user"
                    || (message.side == "assistant" && message.phase == "final_answer"));
            if recent {
                self.recent.offer(message);
            }
        }
        drain_fold_turns(scratch, fold, finish, at, |turn| self.add_turn(turn))
    }

    fn add_turn(&mut self, turn: TurnRecord) -> Result<(), Refusal> {
        self.any_turn_identity |= turn.turn_id.is_some();
        if let Some(phase) = &mut self.phase {
            phase.push(&turn)?;
        }
        self.turns += 1;
        self.completed += usize::from(turn.completed);
        self.activity.commands = self.activity.commands.saturating_add(turn.command_count);
        self.activity.patches = self.activity.patches.saturating_add(turn.patch_count);
        self.activity.errors = self.activity.errors.saturating_add(turn.error_count);
        self.activity.web_searches = self
            .activity
            .web_searches
            .saturating_add(turn.web_search_count);
        self.start = Some(self.start.take().map_or_else(
            || turn.start_ts.clone(),
            |start| start.min(turn.start_ts.clone()),
        ));
        let end = turn_end(&turn);
        self.end = Some(self.end.take().map_or(end.clone(), |prior| prior.max(end)));
        if self.phase.is_none() {
            let replace = self.latest.as_ref().is_none_or(|latest| {
                (&turn.start_ts, &turn.source) > (&latest.start_ts, &latest.source)
            });
            if replace {
                self.latest = Some(turn);
            }
        }
        Ok(())
    }

    /// Whole-transcript counts, and one page of the newest authored prompts
    /// and final answers.
    pub(crate) fn summary(self, page: Page) -> Result<Report, Refusal> {
        let rows = self
            .recent
            .rows
            .iter()
            .rev()
            .map(|row| authored_message_view(row, &page))
            .collect();
        Ok(
            match page.newest(rows, self.recent.offered, |row| &row.ts)? {
                Paged::Shown(recent) => Report::Summary {
                    sources: self.sources,
                    turns: self.turns,
                    completed: self.completed,
                    compactions: self.compactions,
                    activity: self.activity,
                    recent,
                    residues: self.residues,
                },
                Paged::Exported(export) => Report::Exported { export },
            },
        )
    }

    pub(crate) fn tokens(self, by_file: bool) -> Report {
        let mut usage = Vec::new();
        if by_file || self.sources.len() > 1 {
            for source in &self.sources {
                usage.push(
                    self.tokens
                        .get(&source.path)
                        .cloned()
                        .unwrap_or_default()
                        .view(source.name.clone()),
                );
            }
        }
        let mut total = TokenAggregate::default();
        for aggregate in self.tokens.values() {
            total.merge(aggregate);
        }
        usage.push(total.view("TOTAL".to_owned()));
        Report::Tokens {
            sources: self.sources,
            usage,
            residues: self.residues,
        }
    }
}

struct PhaseSource<'a> {
    source: &'a ResolvedSource,
    cursor: ForwardCursor<'a>,
    scratch: super::super::model::FoldBuffer,
    markers: Vec<Marker>,
    fold: FoldState,
    ready: VecDeque<TurnRecord>,
    last_turn: Option<String>,
    turn_start: Option<String>,
    compactions: Option<CompactionTracker>,
    complete: bool,
}

impl<'a> PhaseSource<'a> {
    fn new(source: &'a ResolvedSource, work: &mut SessionWork) -> Result<Self, Refusal> {
        Ok(Self {
            source,
            cursor: ForwardCursor::new(source, None, work)?,
            scratch: super::super::model::FoldBuffer::default(),
            markers: Vec::new(),
            fold: FoldState::default(),
            ready: VecDeque::with_capacity(OPEN_TURN_LIMIT),
            last_turn: None,
            turn_start: None,
            compactions: None,
            complete: false,
        })
    }

    fn window(
        source: &'a ResolvedSource,
        cursor_start: Option<&str>,
        marker_start: Option<&str>,
        end: Option<&str>,
        compactions: usize,
        direction: FrontierDirection,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let cursor = ForwardCursor::new(source, cursor_start, work)?;
        Self::window_from_cursor(source, cursor, marker_start, end, compactions, direction)
    }

    pub(super) fn timeline_window(
        source: &'a ResolvedSource,
        start: Option<&str>,
        end: Option<&str>,
        compactions: usize,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let cursor = ForwardCursor::with_preceding_assistant(source, start, work)?;
        Self::window_from_cursor(
            source,
            cursor,
            start,
            end,
            compactions,
            FrontierDirection::Newest,
        )
    }

    fn window_from_cursor(
        source: &'a ResolvedSource,
        cursor: ForwardCursor<'a>,
        marker_start: Option<&str>,
        end: Option<&str>,
        compactions: usize,
        direction: FrontierDirection,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            source,
            cursor,
            scratch: super::super::model::FoldBuffer::default(),
            markers: Vec::new(),
            fold: FoldState::default(),
            ready: VecDeque::with_capacity(OPEN_TURN_LIMIT),
            last_turn: None,
            turn_start: marker_start.map(str::to_owned),
            compactions: Some(CompactionTracker::new(
                compactions,
                direction,
                marker_start,
                end,
            )?),
            complete: false,
        })
    }

    fn advance(
        &mut self,
        residues: &mut ResidueLedger,
        work: &mut SessionWork,
    ) -> Result<(), Refusal> {
        while self.ready.is_empty() && !self.complete {
            let window = self.cursor.advance(work)?;
            if let Some(assistant) = self.cursor.take_preceding_assistant()
                && let Some(compactions) = &mut self.compactions
            {
                compactions.seed_assistant(assistant);
            }
            if let Some(row) = window.row {
                let mut stop = false;
                match row {
                    DecodedRow::Typed(record) => {
                        let before = self.markers.len();
                        reduce_record(
                            &record,
                            &mut FoldStorage {
                                dataset: &mut self.scratch,
                                markers: &mut self.markers,
                                residues,
                                state: &mut self.fold,
                                tokens: None,
                            },
                            work,
                            &format!(
                                "{}:record {}",
                                self.source.path.display(),
                                self.cursor.continuation().line
                            ),
                        )?;
                        let added = self.markers.len() - before;
                        if added > 0
                            && let Some(turn) = self
                                .fold
                                .current
                                .or_else(|| self.scratch.turns.len().checked_sub(1))
                                .and_then(|index| self.scratch.turns.get_mut(index))
                        {
                            turn.compaction_count =
                                turn.compaction_count.saturating_add(added as u64);
                        }
                        if let Some(compactions) = &mut self.compactions {
                            compactions.observe(
                                self.markers.drain(..),
                                &self.scratch.messages,
                                work,
                                &format!(
                                    "{}:record {}",
                                    self.source.path.display(),
                                    self.cursor.continuation().line
                                ),
                            )?;
                            stop = compactions.complete();
                        } else {
                            self.markers.clear();
                        }
                    }
                    DecodedRow::Residue(residue) => residues.push(
                        residue,
                        work,
                        &format!(
                            "{}:record {}",
                            self.source.path.display(),
                            self.cursor.continuation().line
                        ),
                    )?,
                    DecodedRow::Ignored => {}
                }
                self.scratch.messages.clear();
                if stop {
                    let state = std::mem::take(&mut self.fold);
                    finish_fold(
                        &mut self.scratch,
                        state,
                        work,
                        &self.source.path.display().to_string(),
                    )?;
                    self.drain_ready(true)?;
                    self.complete = true;
                } else {
                    self.drain_ready(false)?;
                }
            }
            if !self.complete && matches!(window.progress, ForwardProgress::Eof(_)) {
                let state = std::mem::take(&mut self.fold);
                finish_fold(
                    &mut self.scratch,
                    state,
                    work,
                    &self.source.path.display().to_string(),
                )?;
                self.drain_ready(true)?;
                self.complete = true;
            }
        }
        Ok(())
    }

    fn drain_ready(&mut self, finish: bool) -> Result<(), Refusal> {
        let at = self.source.path.display().to_string();
        let ready = &mut self.ready;
        let last_turn = &mut self.last_turn;
        let turn_start = self.turn_start.as_deref();
        drain_fold_turns(&mut self.scratch, &mut self.fold, finish, &at, |turn| {
            admit_source_order(last_turn, &turn.start_ts)?;
            if turn_start.is_none_or(|start| turn_end(&turn).as_str() >= start) {
                ready.push_back(turn);
            }
            Ok(())
        })
    }

    fn source_view(self) -> SourceView {
        source_view(self.source, self.cursor.vendor())
    }

    /// The source view, its retained compactions, and every compaction the
    /// window matched.
    fn into_window(
        mut self,
        work: &mut SessionWork,
    ) -> Result<(SourceView, Vec<CompactionRecord>, usize), Refusal> {
        let (compactions, population) = self
            .compactions
            .take()
            .expect("window source retains compaction state")
            .finish(work, &self.source.path.display().to_string())?;
        Ok((self.source_view(), compactions, population))
    }
}

fn drain_fold_turns(
    scratch: &mut super::super::model::FoldBuffer,
    fold: &mut FoldState,
    finish: bool,
    at: &str,
    mut accept: impl FnMut(TurnRecord) -> Result<(), Refusal>,
) -> Result<(), Refusal> {
    scratch.commands.clear();
    for pending in fold.pending.values_mut() {
        pending.command = None;
    }
    let retain = if finish {
        scratch.turns.len()
    } else {
        fold.closed_prefix(scratch.turns.len())
    };
    for turn in scratch.turns.drain(..retain) {
        accept(turn)?;
    }
    fold.release(retain, scratch.turns.len(), at)
}

/// Hands every buffered turn across `streams` to `accept` in start-time then
/// source order, refilling each stream's ready frontier as it empties.
fn merge_ready_turns(
    streams: &mut [PhaseSource],
    residues: &mut ResidueLedger,
    work: &mut SessionWork,
    mut accept: impl FnMut(TurnRecord) -> Result<(), Refusal>,
) -> Result<(), Refusal> {
    for stream in streams.iter_mut() {
        stream.advance(residues, work)?;
    }
    while let Some(index) = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| !stream.ready.is_empty())
        .min_by_key(|(_, stream)| {
            let turn = stream.ready.front().expect("filtered nonempty frontier");
            (&turn.start_ts, &turn.source)
        })
        .map(|(index, _)| index)
    {
        let turn = streams[index]
            .ready
            .pop_front()
            .expect("selected frontier is nonempty");
        accept(turn)?;
        streams[index].advance(residues, work)?;
    }
    Ok(())
}

struct PhaseState {
    options: PhaseOptions,
    filters: Filters,
    current: Option<PhaseAccumulator>,
    candidate: Vec<PhaseTurn>,
    finished: usize,
    rows: ResultFrontier<PhaseView>,
}

impl PhaseState {
    fn new(options: PhaseOptions, filters: Filters) -> Result<Self, Refusal> {
        Ok(Self {
            options,
            filters,
            current: None,
            candidate: Vec::with_capacity(2),
            finished: 0,
            rows: ResultFrontier::new(options.rows, FrontierDirection::Newest)?,
        })
    }

    fn push(&mut self, turn: &TurnRecord) -> Result<(), Refusal> {
        if !self.filters.turn_matches(turn) {
            return Ok(());
        }
        let turn = PhaseTurn::new(turn, self.options);
        if self.current.is_none() {
            self.current = Some(PhaseAccumulator::new(turn, self.options)?);
            return Ok(());
        }
        if has_idle_gap(self.last_turn_end(), &turn.start) {
            self.merge_candidate()?;
            self.finish_current();
            self.current = Some(PhaseAccumulator::new(turn, self.options)?);
            return Ok(());
        }
        let family = turn.family.clone();
        if self.candidate.is_empty() {
            if self
                .current
                .as_ref()
                .is_some_and(|phase| phase.tail_family == family)
            {
                self.current
                    .as_mut()
                    .expect("current phase exists")
                    .add(turn)?;
            } else {
                self.candidate.push(turn);
            }
            return Ok(());
        }
        let candidate_family = self.candidate[0].family.clone();
        if family == candidate_family {
            self.candidate.push(turn);
            if self.candidate.len() == 2 {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|phase| phase.tail_run >= 2)
                {
                    self.finish_current();
                    let mut candidate = self.candidate.drain(..);
                    let first = candidate.next().expect("supported candidate has two turns");
                    let mut phase = PhaseAccumulator::new(first, self.options)?;
                    phase.add(
                        candidate
                            .next()
                            .expect("supported candidate keeps lookahead"),
                    )?;
                    self.current = Some(phase);
                } else {
                    self.merge_candidate()?;
                }
            }
        } else {
            self.merge_candidate()?;
            if self
                .current
                .as_ref()
                .is_some_and(|phase| phase.tail_family == family)
            {
                self.current
                    .as_mut()
                    .expect("current phase exists")
                    .add(turn)?;
            } else {
                self.candidate.push(turn);
            }
        }
        Ok(())
    }

    fn last_turn_end(&self) -> &str {
        self.candidate.last().map_or_else(
            || {
                self.current
                    .as_ref()
                    .expect("current phase exists")
                    .end
                    .as_str()
            },
            |turn| turn.end.as_str(),
        )
    }

    fn merge_candidate(&mut self) -> Result<(), Refusal> {
        for turn in self.candidate.drain(..) {
            self.current
                .as_mut()
                .expect("current phase exists")
                .add(turn)?;
        }
        Ok(())
    }

    /// Number every phase from the first; retain the newest at or before the
    /// caller's cursor.
    fn finish_current(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        self.finished += 1;
        if self
            .options
            .through
            .is_none_or(|through| self.finished <= through)
        {
            self.rows.push(current.view(self.finished));
        }
    }

    /// Retained phases newest first, and every phase at or before the cursor.
    fn finish(mut self) -> Result<(Vec<PhaseView>, usize), Refusal> {
        self.merge_candidate()?;
        self.finish_current();
        let population = self.rows.offered();
        let mut rows = self.rows.into_rows();
        rows.reverse();
        Ok((rows, population))
    }
}

struct PhaseTurn {
    family: String,
    start: String,
    end: String,
    commands: u64,
    patches: u64,
    compactions: u64,
    errors: u64,
    paths: Vec<String>,
    example: Option<PhaseExample>,
}

impl PhaseTurn {
    fn new(turn: &TurnRecord, options: PhaseOptions) -> Self {
        Self {
            family: turn_family(turn),
            start: turn.start_ts.clone(),
            end: turn_end(turn),
            commands: turn.command_count,
            patches: turn.patch_count,
            compactions: turn.compaction_count,
            errors: turn.error_count,
            paths: turn.touched_files.iter().cloned().collect(),
            example: (options.examples > 0).then(|| phase_example(turn, options.max_text)),
        }
    }
}

struct PhaseAccumulator {
    options: PhaseOptions,
    families: Vec<(String, usize)>,
    tail_family: String,
    tail_run: usize,
    turns: usize,
    commands: u64,
    patches: u64,
    compactions: u64,
    errors: u64,
    start: String,
    end: String,
    paths: BTreeMap<String, usize>,
    examples: Vec<PhaseExample>,
}

impl PhaseAccumulator {
    fn new(turn: PhaseTurn, options: PhaseOptions) -> Result<Self, Refusal> {
        let family = turn.family.clone();
        let mut phase = Self {
            options,
            families: Vec::new(),
            tail_family: family,
            tail_run: 0,
            turns: 0,
            commands: 0,
            patches: 0,
            compactions: 0,
            errors: 0,
            start: turn.start.clone(),
            end: turn.end.clone(),
            paths: BTreeMap::new(),
            examples: Vec::with_capacity(options.examples),
        };
        phase.add(turn)?;
        Ok(phase)
    }

    fn add(&mut self, turn: PhaseTurn) -> Result<(), Refusal> {
        let family = turn.family.clone();
        if family == self.tail_family {
            self.tail_run += 1;
        } else {
            self.tail_family = family.clone();
            self.tail_run = 1;
        }
        if let Some(count) = self.families.iter_mut().find(|row| row.0 == family) {
            count.1 += 1;
        } else {
            self.families.push((family, 1));
        }
        self.turns += 1;
        self.commands = self.commands.saturating_add(turn.commands);
        self.patches = self.patches.saturating_add(turn.patches);
        self.compactions = self.compactions.saturating_add(turn.compactions);
        self.errors = self.errors.saturating_add(turn.errors);
        self.end = turn.end;
        if let Some(example) = turn.example
            && self.examples.len() < self.options.examples
        {
            self.examples.push(example);
        }
        for path in turn.paths {
            if !self.paths.contains_key(&path) && self.paths.len() == PHASE_PATH_LIMIT {
                return Err(refusal(
                    "session::phase-path-limit",
                    Some(format!("maximum={PHASE_PATH_LIMIT}")),
                    "narrow the phase filters or touched-path population",
                ));
            }
            *self.paths.entry(path).or_default() += 1;
        }
        Ok(())
    }

    fn view(self, index: usize) -> PhaseView {
        let family = self
            .families
            .into_iter()
            .reduce(|best, next| if next.1 > best.1 { next } else { best })
            .map_or_else(String::new, |row| row.0);
        let mut paths = self.paths.into_iter().collect::<Vec<_>>();
        paths.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        PhaseView {
            index,
            primary: family.clone(),
            family,
            turns: self.turns,
            commands: self.commands,
            patches: self.patches,
            compactions: self.compactions,
            errors: self.errors,
            duration_millis: None,
            start: self.start,
            end: self.end,
            top_paths: paths.into_iter().take(5).map(|row| row.0).collect(),
            examples: self.examples,
        }
    }
}

fn has_idle_gap(previous: &str, current: &str) -> bool {
    let (Ok(previous), Ok(current)) = (
        previous.parse::<jiff::Timestamp>(),
        current.parse::<jiff::Timestamp>(),
    ) else {
        return false;
    };
    current.as_second() - previous.as_second() >= PHASE_GAP_SECONDS
}

pub(crate) fn admit_sources(sources: &[ResolvedSource]) -> Result<(), Refusal> {
    if sources.len() <= SOURCE_LIMIT {
        return Ok(());
    }
    Err(refusal(
        "session::aggregate-source-limit",
        Some(format!("observed={} maximum={SOURCE_LIMIT}", sources.len())),
        "select at most the repository aggregate source ceiling and retry",
    ))
}

impl TokenAggregate {
    fn add(&mut self, row: &TokenRecord) {
        self.total = self.total.saturating_add(row.total);
        self.input = self.input.saturating_add(row.input);
        self.cached = self.cached.saturating_add(row.cached);
        self.output = self.output.saturating_add(row.output);
        self.reasoning = self.reasoning.saturating_add(row.reasoning);
        self.snapshots = self.snapshots.saturating_add(1);
        self.first = Some(
            self.first
                .take()
                .map_or_else(|| row.ts.clone(), |ts| ts.min(row.ts.clone())),
        );
        if self.last.as_ref().is_none_or(|ts| row.ts >= *ts) {
            self.last = Some(row.ts.clone());
            self.context = row.model_context_window;
        }
    }

    fn merge(&mut self, other: &Self) {
        self.total = self.total.saturating_add(other.total);
        self.input = self.input.saturating_add(other.input);
        self.cached = self.cached.saturating_add(other.cached);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
        self.snapshots = self.snapshots.saturating_add(other.snapshots);
        if let Some(first) = &other.first {
            self.first = Some(
                self.first
                    .take()
                    .map_or_else(|| first.clone(), |ts| ts.min(first.clone())),
            );
        }
        if other
            .last
            .as_ref()
            .is_some_and(|last| self.last.as_ref().is_none_or(|ts| last >= ts))
        {
            self.last = other.last.clone();
            self.context = other.context;
        }
    }

    fn view(self, source: String) -> TokenView {
        TokenView {
            source,
            total: self.total,
            input: self.input,
            cached: self.cached,
            uncached: self.input.saturating_sub(self.cached),
            output: self.output,
            reasoning: self.reasoning,
            model_context_window: self.context,
            snapshots: self.snapshots,
            first_snapshot: self.first,
            last_snapshot: self.last,
        }
    }
}
