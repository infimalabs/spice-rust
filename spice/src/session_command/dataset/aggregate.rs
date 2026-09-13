//! Bounded retained state for complete-scan session aggregates.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use wire::Refusal;

use super::super::model::{
    ActivityView, CompactionRecord, Dataset, MessageRecord, PhaseExample, PhaseView, Report,
    Residue, SourceView, ThreadView, TokenRecord, TokenView, TurnRecord, TurnView, Vendor,
};
use super::super::project::{
    Filters, PhaseOptions, authored_message_view, phase_example, turn_end, turn_family, turn_view,
};
use super::super::source::ResolvedSource;
use super::stream::{ForwardCursor, ForwardProgress};
use super::{
    DecodedRow, FoldState, FoldStorage, FrontierDirection, Marker, MessageQuery, OPEN_TOOL_LIMIT,
    OPEN_TURN_LIMIT, ResidueLedger, ResultFrontier, SessionWork, SessionWorkBudget,
    compaction_intent, compaction_text_bytes, finish_fold, message_text_bytes, reduce_record,
    refusal, source_view_text_bytes, stream_records,
};

mod commit;
pub(crate) use commit::CommitScan;

const PHASE_GAP_SECONDS: i64 = 45 * 60;
const PHASE_PATH_LIMIT: usize = 4_096;
const SOURCE_LIMIT: usize = 64;
const MAX_MESSAGE_IDENTITIES: usize = 4 * 1_024;
const WINDOW_TURN_LIMIT: usize = 4 * 1_024;

impl MessageQuery {
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
    pub(super) residues: Vec<Residue>,
    pub(super) vendor: Vendor,
}

#[derive(Clone, Copy)]
pub(crate) enum WindowOptions {
    Briefing {
        max_lines: usize,
        max_bytes: usize,
        explain: bool,
        compactions: usize,
    },
    Sweep {
        compactions: usize,
    },
}

impl WindowOptions {
    pub(crate) const fn compactions(self) -> usize {
        match self {
            Self::Briefing { compactions, .. } | Self::Sweep { compactions } => compactions,
        }
    }

    pub(crate) const fn briefing(self) -> Option<(usize, usize, bool)> {
        match self {
            Self::Briefing {
                max_lines,
                max_bytes,
                explain,
                ..
            } => Some((max_lines, max_bytes, explain)),
            Self::Sweep { .. } => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WindowTurn {
    pub(crate) view: TurnView,
    pub(crate) end: String,
    pub(crate) latest_final: Option<String>,
    lines: usize,
    bytes: usize,
}

impl WindowTurn {
    fn project(turn: TurnRecord) -> Self {
        let latest_final = turn.messages.iter().rev().find_map(|message| {
            (message.phase == "final_answer")
                .then(|| message.authored.clone())
                .flatten()
        });
        let lines = 1 + turn
            .messages
            .iter()
            .filter(|message| message.authored.is_some())
            .count();
        let bytes = turn
            .messages
            .iter()
            .filter_map(|message| message.authored.as_ref())
            .map(String::len)
            .sum();
        let end = turn_end(&turn);
        let view = turn_view(&turn, false, 180, true);
        Self {
            view,
            end,
            latest_final,
            lines,
            bytes,
        }
    }
}

pub(crate) struct WindowScan {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) turns: Vec<WindowTurn>,
    pub(crate) compactions: Vec<CompactionRecord>,
    pub(crate) residues: Vec<Residue>,
    pub(crate) turns_before: usize,
    pub(crate) options: WindowOptions,
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
        let mut streams = sources
            .iter()
            .map(|source| PhaseSource::window(source, start, end, options.compactions(), &mut work))
            .collect::<Result<Vec<_>, _>>()?;
        for stream in &mut streams {
            stream.advance(&mut residues, &mut work)?;
        }
        let mut turns = WindowTurnFrontier::new(options);
        let mut any_turn_identity = false;
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
                .expect("selected window frontier is nonempty");
            any_turn_identity |= turn.turn_id.is_some();
            if filters.turn_matches(&turn) {
                turns.push(WindowTurn::project(turn))?;
            }
            streams[index].advance(&mut residues, &mut work)?;
        }
        filters.validate_turn_identity_available(any_turn_identity)?;
        let mut sources = Vec::with_capacity(streams.len());
        let mut compaction_sources = Vec::with_capacity(streams.len());
        for stream in streams {
            let (source, compactions) = stream.into_window(&mut work)?;
            work.admit_retained_text(source_view_text_bytes(&source), &source.path)?;
            sources.push(source);
            compaction_sources.push(VecDeque::from(compactions));
        }
        let mut compactions =
            ResultFrontier::new(options.compactions(), FrontierDirection::Newest)?;
        while let Some(index) = compaction_sources
            .iter()
            .enumerate()
            .filter(|(_, rows)| !rows.is_empty())
            .min_by_key(|(_, rows)| {
                let row = rows
                    .front()
                    .expect("filtered compaction source is nonempty");
                (&row.ts, &row.source)
            })
            .map(|(index, _)| index)
        {
            compactions.push(
                compaction_sources[index]
                    .pop_front()
                    .expect("selected compaction source is nonempty"),
            );
        }
        Ok(Self {
            sources,
            turns: turns.rows.into(),
            compactions: compactions.into_rows(),
            residues: residues.finish(&mut work, "window residue summary")?,
            turns_before: turns.before,
            options,
        })
    }
}

struct WindowTurnFrontier {
    options: WindowOptions,
    rows: VecDeque<WindowTurn>,
    lines: usize,
    bytes: usize,
    before: usize,
}

impl WindowTurnFrontier {
    fn new(options: WindowOptions) -> Self {
        Self {
            options,
            rows: VecDeque::new(),
            lines: 0,
            bytes: 0,
            before: 0,
        }
    }

    fn push(&mut self, turn: WindowTurn) -> Result<(), Refusal> {
        if matches!(self.options, WindowOptions::Sweep { .. })
            && self.rows.len() == WINDOW_TURN_LIMIT
        {
            return Err(refusal(
                "session::window-turn-limit",
                Some(format!("maximum={WINDOW_TURN_LIMIT}")),
                "narrow the requested compaction windows or transcript filters",
            ));
        }
        self.before = self.before.saturating_add(1);
        self.lines = self.lines.saturating_add(turn.lines);
        self.bytes = self.bytes.saturating_add(turn.bytes);
        self.rows.push_back(turn);
        if let Some((max_lines, max_bytes, _)) = self.options.briefing() {
            while self.rows.len() > 1 && (self.lines > max_lines || self.bytes > max_bytes) {
                let removed = self
                    .rows
                    .pop_front()
                    .expect("multiple turns have an oldest");
                self.lines -= removed.lines;
                self.bytes -= removed.bytes;
            }
        }
        Ok(())
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
    direction: FrontierDirection,
    work: &mut SessionWork,
) -> Result<MessageSourceScan, Refusal> {
    let mut frontier = ResultFrontier::new(query.limit.get(), direction)?;
    let mut cursor = ForwardCursor::new(source, start, work)?;
    let mut scratch = Dataset::default();
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
                if dedup.admit(&message, work, &at)? && query.matches(&message) {
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
            if query.oldest_first && frontier.is_full() {
                break;
            }
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
    prompts: Vec<MessageRecord>,
    finals: Vec<MessageRecord>,
    tokens: BTreeMap<String, TokenAggregate>,
    start: Option<String>,
    end: Option<String>,
    latest: Option<TurnRecord>,
    phase: Option<PhaseState>,
    any_turn_identity: bool,
    residues: Vec<super::super::model::Residue>,
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
    pub(crate) fn scan(sources: &[ResolvedSource], recent: usize) -> Result<Self, Refusal> {
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
            prompts: Vec::with_capacity(recent),
            finals: Vec::with_capacity(recent),
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
            aggregate.scan_source(source, recent, &mut residues, &mut work)?;
        }
        aggregate.residues = residues.finish(&mut work, "aggregate residue summary")?;
        Ok(aggregate)
    }

    pub(crate) fn scan_phases(
        sources: &[ResolvedSource],
        options: PhaseOptions,
        filters: Filters,
    ) -> Result<Report, Refusal> {
        let mut aggregate = Self::scan_with_phase(sources, options, filters)?;
        let phase = aggregate
            .phase
            .take()
            .expect("phase scan retains phase state");
        phase
            .filters
            .validate_turn_identity_available(aggregate.any_turn_identity)?;
        Ok(Report::Phases {
            sources: aggregate.sources,
            rows: phase.finish()?,
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
            prompts: Vec::new(),
            finals: Vec::new(),
            tokens: BTreeMap::new(),
            start: None,
            end: None,
            latest: None,
            phase: Some(PhaseState::new(options, filters)),
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
        for stream in &mut streams {
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
                .expect("selected phase frontier is nonempty");
            self.add_turn(turn)?;
            streams[index].advance(residues, work)?;
        }
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
        recent: usize,
        residues: &mut ResidueLedger,
        work: &mut SessionWork,
    ) -> Result<(), Refusal> {
        let source_name = source.path.display().to_string();
        let mut scratch = super::super::model::Dataset::default();
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
                    self.drain_scratch(&mut scratch, &mut tokens, &mut fold, recent, false, at)?;
                }
                DecodedRow::Residue(residue) => residues.push(residue, work, at)?,
                DecodedRow::Ignored => {}
            }
            Ok(())
        })?;
        finish_fold(&mut scratch, fold, work, &source_name)?;
        let mut finished = FoldState::default();
        self.drain_scratch(
            &mut scratch,
            &mut tokens,
            &mut finished,
            recent,
            true,
            &source_name,
        )?;
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
        scratch: &mut super::super::model::Dataset,
        tokens: &mut Vec<TokenRecord>,
        fold: &mut FoldState,
        recent: usize,
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
            if message.side == "user" && message.authored.is_some() {
                retain_recent(&mut self.prompts, message, recent);
            } else if message.side == "assistant"
                && message.phase == "final_answer"
                && message.authored.is_some()
            {
                retain_recent(&mut self.finals, message, recent);
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

    pub(crate) fn summary(self) -> Report {
        Report::Summary {
            sources: self.sources,
            turns: self.turns,
            completed: self.completed,
            compactions: self.compactions,
            activity: self.activity,
            recent_prompts: self.prompts.iter().map(authored_message_view).collect(),
            recent_finals: self.finals.iter().map(authored_message_view).collect(),
            residues: self.residues,
        }
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

    pub(crate) fn thread(self, id: String) -> Report {
        let source = self
            .sources
            .last()
            .cloned()
            .expect("one resolved thread yields at least one source");
        let thread = ThreadView {
            id,
            vendor: source.vendor,
            transcript: source.path.clone(),
            transcripts: self
                .sources
                .iter()
                .map(|source| source.path.clone())
                .collect(),
            turns: self.turns,
            compactions: self.compactions,
            start: self.start,
            end: self.end,
            latest_turn: self
                .latest
                .as_ref()
                .map(|turn| turn_view(turn, false, 180, false)),
        };
        Report::Thread {
            source,
            sources: self.sources,
            thread,
            residues: self.residues,
        }
    }
}

struct PendingCompaction {
    marker: Marker,
    assistant_before: String,
    summary: String,
    summary_seen: bool,
    user_after: String,
    user_seen: bool,
}

struct CompactionTracker {
    last_assistant: Option<String>,
    last: Option<String>,
    pending: Option<PendingCompaction>,
    rows: ResultFrontier<CompactionRecord>,
    end: Option<String>,
}

impl CompactionTracker {
    fn new(limit: usize, end: Option<&str>) -> Result<Self, Refusal> {
        Ok(Self {
            last_assistant: None,
            last: None,
            pending: None,
            rows: ResultFrontier::new(limit, FrontierDirection::Newest)?,
            end: end.map(str::to_owned),
        })
    }

    fn observe(
        &mut self,
        markers: impl IntoIterator<Item = Marker>,
        messages: &[MessageRecord],
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        for marker in markers {
            self.finish_pending(work, at)?;
            if self.end.as_ref().is_none_or(|end| marker.ts <= *end) {
                let summary_seen = !marker.summary.is_empty();
                self.pending = Some(PendingCompaction {
                    summary: marker.summary.clone(),
                    marker,
                    assistant_before: self.last_assistant.clone().unwrap_or_default(),
                    summary_seen,
                    user_after: String::new(),
                    user_seen: false,
                });
            }
        }
        for message in messages {
            if let Some(pending) = &mut self.pending
                && message.position > pending.marker.position
            {
                if !pending.summary_seen
                    && message.shape == crate::message_shape::MessageShape::CompactionRehydration
                {
                    pending.summary = message.text.clone();
                    pending.summary_seen = true;
                }
                if !pending.user_seen && message.side == "user" && message.shape.is_authored() {
                    pending.user_after = message.authored.clone().unwrap_or_default();
                    pending.user_seen = true;
                }
            }
            if message.side == "assistant" {
                self.last_assistant = Some(message.text.clone());
            }
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.summary_seen && pending.user_seen)
        {
            self.finish_pending(work, at)?;
        }
        Ok(())
    }

    fn finish_pending(&mut self, work: &mut SessionWork, at: &str) -> Result<(), Refusal> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let row = CompactionRecord {
            source: pending.marker.source,
            ts: pending.marker.ts,
            assistant_before: pending.assistant_before,
            intent: compaction_intent(&pending.summary),
            summary_after: pending.summary,
            user_after: pending.user_after,
        };
        admit_source_order(&mut self.last, &row.ts)?;
        work.admit_retained_text(compaction_text_bytes(&row), at)?;
        self.rows.push(row);
        Ok(())
    }

    fn finish(
        mut self,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<Vec<CompactionRecord>, Refusal> {
        self.finish_pending(work, at)?;
        Ok(self.rows.into_rows())
    }
}

struct PhaseSource<'a> {
    source: &'a ResolvedSource,
    cursor: ForwardCursor<'a>,
    scratch: super::super::model::Dataset,
    markers: Vec<Marker>,
    fold: FoldState,
    ready: VecDeque<TurnRecord>,
    last_turn: Option<String>,
    compactions: Option<CompactionTracker>,
    complete: bool,
}

impl<'a> PhaseSource<'a> {
    fn new(source: &'a ResolvedSource, work: &mut SessionWork) -> Result<Self, Refusal> {
        Ok(Self {
            source,
            cursor: ForwardCursor::new(source, None, work)?,
            scratch: super::super::model::Dataset::default(),
            markers: Vec::new(),
            fold: FoldState::default(),
            ready: VecDeque::with_capacity(OPEN_TURN_LIMIT),
            last_turn: None,
            compactions: None,
            complete: false,
        })
    }

    fn window(
        source: &'a ResolvedSource,
        start: Option<&str>,
        end: Option<&str>,
        compactions: usize,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            source,
            cursor: ForwardCursor::new(source, start, work)?,
            scratch: super::super::model::Dataset::default(),
            markers: Vec::new(),
            fold: FoldState::default(),
            ready: VecDeque::with_capacity(OPEN_TURN_LIMIT),
            last_turn: None,
            compactions: Some(CompactionTracker::new(compactions, end)?),
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
            if let Some(row) = window.row {
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
                self.drain_ready(false)?;
            }
            if matches!(window.progress, ForwardProgress::Eof(_)) {
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
        drain_fold_turns(&mut self.scratch, &mut self.fold, finish, &at, |turn| {
            admit_source_order(last_turn, &turn.start_ts)?;
            ready.push_back(turn);
            Ok(())
        })
    }

    fn source_view(self) -> SourceView {
        let path = self.source.path.display().to_string();
        SourceView {
            name: self
                .source
                .path
                .file_name()
                .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
            path,
            vendor: self.cursor.vendor(),
            thread: self.source.thread.clone(),
            compressed: self.source.path.to_string_lossy().ends_with(".gz"),
        }
    }

    fn into_window(
        mut self,
        work: &mut SessionWork,
    ) -> Result<(SourceView, Vec<CompactionRecord>), Refusal> {
        let compactions = self
            .compactions
            .take()
            .expect("window source retains compaction state")
            .finish(work, &self.source.path.display().to_string())?;
        Ok((self.source_view(), compactions))
    }
}

fn admit_source_order(previous: &mut Option<String>, timestamp: &str) -> Result<(), Refusal> {
    if previous
        .as_deref()
        .is_some_and(|previous| timestamp < previous)
    {
        return Err(refusal(
            "session::source-order-regressed",
            Some(format!("previous={previous:?} current={timestamp:?}")),
            "repair source-local timestamp order before merging transcript windows",
        ));
    }
    *previous = Some(timestamp.to_owned());
    Ok(())
}

fn drain_fold_turns(
    scratch: &mut super::super::model::Dataset,
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
        fold.current
            .into_iter()
            .chain(fold.pending.values().map(|pending| pending.turn))
            .min()
            .unwrap_or(scratch.turns.len())
    };
    for turn in scratch.turns.drain(..retain) {
        accept(turn)?;
    }
    if retain > 0 {
        fold.current = fold.current.map(|index| index - retain);
        for pending in fold.pending.values_mut() {
            pending.turn -= retain;
        }
    }
    if scratch.turns.len() > OPEN_TURN_LIMIT {
        return Err(refusal(
            "session::aggregate-open-turn-limit",
            Some(format!("limit={OPEN_TURN_LIMIT} at={at:?}")),
            "repair unresolved tool spans or narrow the immutable transcript inputs",
        ));
    }
    if fold.pending.len() > OPEN_TOOL_LIMIT {
        return Err(refusal(
            "session::aggregate-open-tool-limit",
            Some(format!("limit={OPEN_TOOL_LIMIT} at={at:?}")),
            "repair unresolved tool spans or narrow the immutable transcript inputs",
        ));
    }
    Ok(())
}

struct PhaseState {
    options: PhaseOptions,
    filters: Filters,
    current: Option<PhaseAccumulator>,
    candidate: Vec<PhaseTurn>,
    rows: Vec<PhaseView>,
}

impl PhaseState {
    fn new(options: PhaseOptions, filters: Filters) -> Self {
        Self {
            options,
            filters,
            current: None,
            candidate: Vec::with_capacity(2),
            rows: Vec::with_capacity(options.rows),
        }
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

    fn finish_current(&mut self) {
        if self.rows.len() < self.options.rows
            && let Some(current) = self.current.take()
        {
            self.rows.push(current.view(self.rows.len() + 1));
        } else {
            self.current = None;
        }
    }

    fn finish(mut self) -> Result<Vec<PhaseView>, Refusal> {
        self.merge_candidate()?;
        self.finish_current();
        Ok(self.rows)
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

fn retain_recent(rows: &mut Vec<MessageRecord>, row: MessageRecord, limit: usize) {
    if limit == 0 {
        return;
    }
    rows.push(row);
    rows.sort_by(|left, right| {
        (&left.ts, &left.source, &left.side).cmp(&(&right.ts, &right.source, &right.side))
    });
    if rows.len() > limit {
        rows.remove(0);
    }
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
