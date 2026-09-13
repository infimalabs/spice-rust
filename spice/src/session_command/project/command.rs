//! What a session actually ran.
//!
//! The command fold reports the shell an agent issued and audits how much
//! of it went through the recorded wrapper rather than around it, which is why
//! the pipeline reading here is structural rather than a substring search.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use clap::ArgMatches;
use wire::Refusal;

use super::super::model::{
    CommandAudit, CommandRecord, CommandView, FoldBuffer, Report, Residue, SourceView, Vendor,
};
use super::{
    DEFAULT_COMMAND_TEXT, DEFAULT_COMMANDS_LIMIT, MAX_WINDOW_TEXT, clip, flag, number, refusal,
};
use crate::session_command::dataset::stream::{ForwardCursor, ForwardProgress, compressed};
use crate::session_command::dataset::{
    DecodedRow, FoldState, FoldStorage, FrontierDirection, Marker, OPEN_TOOL_LIMIT,
    OPEN_TURN_LIMIT, ResidueLedger, ResultFrontier, SessionWork, SessionWorkBudget, admit_sources,
    finish_fold, frontier_limit, recent_compaction_horizon, recent_turn_start, reduce_record,
    source_view_text_bytes, text_bytes,
};
use crate::session_command::source::ResolvedSource;

const MAX_COMMAND_AUDIT_ROWS: usize = 256 * 1_024;
const MAX_COMMAND_LABELS: usize = 4 * 1_024;

pub(crate) struct CommandQuery {
    since_compaction: bool,
    failed: bool,
    pipelines: bool,
    noncanonical_pipelines: bool,
    summary: bool,
    limit: NonZeroUsize,
}

struct CommandDataset {
    sources: Vec<SourceView>,
    rows: Vec<CommandRecord>,
    audit: Option<CommandAudit>,
    residues: Vec<Residue>,
}

impl CommandQuery {
    fn new(
        since_compaction: bool,
        failed: bool,
        pipelines: bool,
        noncanonical_pipelines: bool,
        summary: bool,
        limit: usize,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            since_compaction,
            failed,
            pipelines,
            noncanonical_pipelines,
            summary,
            limit: frontier_limit(limit)?,
        })
    }

    fn completed(row: &CommandRecord) -> bool {
        row.exit_code.is_some() && row.status != "running"
    }

    fn matches(&self, row: &CommandRecord, latest_compaction: Option<&str>) -> bool {
        (!self.since_compaction
            || latest_compaction.is_none_or(|timestamp| row.ts.as_str() > timestamp))
            && (!self.failed || row.exit_code.is_some_and(|code| code != 0))
            && (!self.pipelines || has_pipeline(&row.command))
            && (!self.noncanonical_pipelines || noncanonical_pipeline(&row.command))
    }

    fn filters(&self) -> Vec<String> {
        [
            (self.since_compaction, "since-compaction"),
            (self.failed, "failed"),
            (self.pipelines, "pipelines"),
            (self.noncanonical_pipelines, "noncanonical-pipelines"),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, name)| name.to_owned())
        .collect()
    }
}

pub(crate) struct CommandOptions {
    query: CommandQuery,
    max_text: usize,
    newest_first: bool,
}

pub(crate) fn options(matches: &ArgMatches) -> Result<CommandOptions, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMMANDS_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_COMMAND_TEXT)?;
    if max_text > MAX_WINDOW_TEXT {
        return Err(refusal(
            "session::bound-exceeded",
            Some(format!("--max-text={max_text} maximum={MAX_WINDOW_TEXT}")),
            "pass a command text bound within the repository ceiling",
        ));
    }
    let newest_first = flag(matches, "newest-first");
    let query = CommandQuery::new(
        flag(matches, "since-compaction"),
        flag(matches, "failed"),
        flag(matches, "pipelines"),
        flag(matches, "noncanonical-pipelines"),
        flag(matches, "summary"),
        limit,
    )?;
    Ok(CommandOptions {
        query,
        max_text,
        newest_first,
    })
}

pub(crate) fn commands(
    sources: &[ResolvedSource],
    options: CommandOptions,
) -> Result<Report, Refusal> {
    let dataset = scan_commands(sources, &options.query)?;
    Ok(project_commands(dataset, options))
}

fn scan_commands(
    sources: &[ResolvedSource],
    query: &CommandQuery,
) -> Result<CommandDataset, Refusal> {
    admit_sources(sources)?;
    let latest_compaction = if query.since_compaction {
        recent_compaction_horizon(sources, 1, None)?.start
    } else {
        None
    };
    let mut selected = Vec::with_capacity(sources.len().saturating_mul(query.limit.get()));
    let mut source_views = Vec::with_capacity(sources.len());
    let mut residues = ResidueLedger::new();
    let mut work = SessionWork::new(SessionWorkBudget::report());
    let mut audit = query.summary.then(|| CommandAuditFold::new(query));
    for source in sources {
        let scan = scan_source_plan(
            source,
            latest_compaction.as_deref(),
            query,
            &mut audit,
            &mut work,
        )?;
        selected.extend(scan.rows);
        for residue in scan.residues {
            residues.push(residue, &mut work, "command source residue")?;
        }
        source_views.push(source_view(source, scan.vendor, &mut work)?);
    }
    selected.sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    if selected.len() > query.limit.get() {
        selected.drain(..selected.len() - query.limit.get());
    }
    Ok(CommandDataset {
        sources: source_views,
        rows: selected,
        audit: audit
            .map(|audit| audit.finish(&mut work, "command audit"))
            .transpose()?,
        residues: residues.finish(&mut work, "command residue summary")?,
    })
}

fn scan_source_plan(
    source: &ResolvedSource,
    latest_compaction: Option<&str>,
    query: &CommandQuery,
    audit: &mut Option<CommandAuditFold>,
    work: &mut SessionWork,
) -> Result<CommandSourceScan, Refusal> {
    if query.summary || latest_compaction.is_some() || compressed(&source.path) {
        let start = if query.summary {
            None
        } else {
            latest_compaction
        };
        return scan_command_source(source, start, latest_compaction, query, audit, work);
    }
    let mut turn_count = query.limit.get();
    loop {
        let start = recent_turn_start(source, turn_count, work)?;
        let scan = scan_command_source(source, start.as_deref(), None, query, audit, work)?;
        if scan.rows.len() == query.limit.get() || start.is_none() {
            return Ok(scan);
        }
        turn_count = turn_count.saturating_add(query.limit.get());
    }
}

fn source_view(
    source: &ResolvedSource,
    vendor: Vendor,
    work: &mut SessionWork,
) -> Result<SourceView, Refusal> {
    let path = source.path.display().to_string();
    let view = SourceView {
        name: source
            .path
            .file_name()
            .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
        path,
        vendor,
        thread: source.thread.clone(),
        compressed: compressed(&source.path),
    };
    work.admit_retained_text(source_view_text_bytes(&view), &view.path)?;
    Ok(view)
}

fn project_commands(dataset: CommandDataset, options: CommandOptions) -> Report {
    let mut rows = dataset
        .rows
        .iter()
        .map(|row| command_view(row, options.max_text))
        .collect::<Vec<_>>();
    if options.newest_first {
        rows.reverse();
    }
    Report::Commands {
        sources: dataset.sources,
        rows,
        audit: dataset.audit,
        residues: dataset.residues,
    }
}

struct CommandAuditFold {
    population: usize,
    matched: usize,
    filters: Vec<String>,
    failed: usize,
    wrapper: usize,
    pipelines: usize,
    canonical: usize,
    labels: BTreeMap<String, usize>,
}

impl CommandAuditFold {
    fn new(query: &CommandQuery) -> Self {
        Self {
            population: 0,
            matched: 0,
            filters: query.filters(),
            failed: 0,
            wrapper: 0,
            pipelines: 0,
            canonical: 0,
            labels: BTreeMap::new(),
        }
    }

    fn observe(
        &mut self,
        row: &CommandRecord,
        latest_compaction: Option<&str>,
        query: &CommandQuery,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        if !CommandQuery::completed(row) {
            return Ok(());
        }
        if self.population == MAX_COMMAND_AUDIT_ROWS {
            return Err(refusal(
                "session::command-audit-limit",
                Some(format!("limit={MAX_COMMAND_AUDIT_ROWS} at={at:?}")),
                "narrow the command filters or transcript inputs and retry",
            ));
        }
        self.population = self.population.saturating_add(1);
        if !query.matches(row, latest_compaction) {
            return Ok(());
        }
        self.matched = self.matched.saturating_add(1);
        self.failed = self
            .failed
            .saturating_add(usize::from(row.exit_code.is_some_and(|code| code != 0)));
        let wrapper = starts_with_wrapper(&row.command);
        self.wrapper = self.wrapper.saturating_add(usize::from(wrapper));
        let pipeline = has_pipeline(&row.command);
        self.pipelines = self.pipelines.saturating_add(usize::from(pipeline));
        let canonical = canonical_pipeline(&row.command);
        self.canonical = self.canonical.saturating_add(usize::from(canonical));
        self.observe_label(row, pipeline && !canonical, work, at)
    }

    fn observe_label(
        &mut self,
        row: &CommandRecord,
        selected: bool,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        if !selected {
            return Ok(());
        }
        let label = command_label(&row.command);
        if !self.labels.contains_key(&label) && self.labels.len() == MAX_COMMAND_LABELS {
            return Err(refusal(
                "session::command-label-limit",
                Some(format!("limit={MAX_COMMAND_LABELS} at={at:?}")),
                "narrow the command filters or transcript inputs and retry",
            ));
        }
        if !self.labels.contains_key(&label) {
            work.admit_retained_text(text_bytes([label.as_str()]), at)?;
        }
        *self.labels.entry(label).or_default() += 1;
        Ok(())
    }

    fn finish(self, work: &mut SessionWork, at: &str) -> Result<CommandAudit, Refusal> {
        let mut labels = self.labels.into_iter().collect::<Vec<_>>();
        labels.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        let top_noncanonical = labels
            .into_iter()
            .take(4)
            .map(|(label, count)| format!("{label}:{count}"))
            .collect::<Vec<_>>();
        work.admit_retained_text(
            text_bytes(
                self.filters
                    .iter()
                    .chain(top_noncanonical.iter())
                    .map(String::as_str),
            ),
            at,
        )?;
        Ok(CommandAudit {
            population: self.population,
            matched: self.matched,
            filters: self.filters,
            failed: self.failed,
            wrapper: self.wrapper,
            non_wrapper: self.matched.saturating_sub(self.wrapper),
            pipelines: self.pipelines,
            canonical_pipelines: self.canonical,
            noncanonical_pipelines: self.pipelines.saturating_sub(self.canonical),
            top_noncanonical,
        })
    }
}

struct CommandSourceScan {
    rows: Vec<CommandRecord>,
    residues: Vec<Residue>,
    vendor: Vendor,
}

struct CommandSourceFold {
    frontier: ResultFrontier<CommandRecord>,
    scratch: FoldBuffer,
    markers: Vec<Marker>,
    residues: ResidueLedger,
    state: FoldState,
}

impl CommandSourceFold {
    fn new(limit: usize) -> Result<Self, Refusal> {
        Ok(Self {
            frontier: ResultFrontier::new(limit, FrontierDirection::Newest)?,
            scratch: FoldBuffer::default(),
            markers: Vec::new(),
            residues: ResidueLedger::new(),
            state: FoldState::default(),
        })
    }

    fn push(
        &mut self,
        row: DecodedRow,
        latest_compaction: Option<&str>,
        query: &CommandQuery,
        audit: &mut Option<CommandAuditFold>,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        match row {
            DecodedRow::Typed(record) => reduce_record(
                &record,
                &mut FoldStorage {
                    dataset: &mut self.scratch,
                    markers: &mut self.markers,
                    residues: &mut self.residues,
                    state: &mut self.state,
                    tokens: None,
                },
                work,
                at,
            )?,
            DecodedRow::Residue(residue) => self.residues.push(residue, work, at)?,
            DecodedRow::Ignored => {}
        }
        self.markers.clear();
        self.scratch.messages.clear();
        for command in drain_command_records(&mut self.scratch, &mut self.state) {
            if let Some(audit) = audit {
                audit.observe(&command, latest_compaction, query, work, at)?;
            } else if CommandQuery::completed(&command)
                && query.matches(&command, latest_compaction)
            {
                self.frontier.push(command);
            }
        }
        drain_command_turns(&mut self.scratch, &mut self.state, at)
    }

    fn finish(
        mut self,
        source: &ResolvedSource,
        vendor: Vendor,
        audit: bool,
        work: &mut SessionWork,
    ) -> Result<CommandSourceScan, Refusal> {
        let state = std::mem::take(&mut self.state);
        finish_fold(
            &mut self.scratch,
            state,
            work,
            &source.path.display().to_string(),
        )?;
        Ok(CommandSourceScan {
            rows: if audit {
                Vec::new()
            } else {
                self.frontier.into_rows()
            },
            residues: self
                .residues
                .finish(work, "command source residue summary")?,
            vendor,
        })
    }
}

fn scan_command_source(
    source: &ResolvedSource,
    start: Option<&str>,
    latest_compaction: Option<&str>,
    query: &CommandQuery,
    audit: &mut Option<CommandAuditFold>,
    work: &mut SessionWork,
) -> Result<CommandSourceScan, Refusal> {
    let mut cursor = ForwardCursor::new(source, start, work)?;
    let mut fold = CommandSourceFold::new(query.limit.get())?;
    loop {
        let window = cursor.advance(work)?;
        if let Some(row) = window.row {
            let at = format!(
                "{}:record {}",
                source.path.display(),
                cursor.continuation().line
            );
            fold.push(row, latest_compaction, query, audit, work, &at)?;
        }
        if matches!(window.progress, ForwardProgress::Eof(_)) {
            break;
        }
    }
    fold.finish(source, cursor.vendor(), audit.is_some(), work)
}

fn drain_command_records(scratch: &mut FoldBuffer, fold: &mut FoldState) -> Vec<CommandRecord> {
    let pending = fold
        .pending
        .values()
        .filter_map(|pending| pending.command)
        .collect::<BTreeSet<_>>();
    let mut retained = Vec::with_capacity(pending.len());
    let mut emitted = Vec::new();
    let mut remap = vec![None; scratch.commands.len()];
    for (index, command) in std::mem::take(&mut scratch.commands)
        .into_iter()
        .enumerate()
    {
        if pending.contains(&index) {
            remap[index] = Some(retained.len());
            retained.push(command);
        } else {
            emitted.push(command);
        }
    }
    for pending in fold.pending.values_mut() {
        if let Some(index) = pending.command {
            pending.command = Some(remap[index].expect("pending command remains in fold state"));
        }
    }
    scratch.commands = retained;
    emitted
}

fn drain_command_turns(
    scratch: &mut FoldBuffer,
    fold: &mut FoldState,
    at: &str,
) -> Result<(), Refusal> {
    let retain = fold
        .current
        .into_iter()
        .chain(fold.pending.values().map(|pending| pending.turn))
        .min()
        .unwrap_or(scratch.turns.len());
    scratch.turns.drain(..retain);
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
    words.starts_with(&["spice", "run"])
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
        return "spice run".to_owned();
    }
    shell_words::split(command)
        .unwrap_or_else(|_| command.split_whitespace().map(str::to_owned).collect())
        .into_iter()
        .find(|word| !environment_assignment(word))
        .unwrap_or_else(|| "-".to_owned())
}
