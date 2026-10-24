use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use listen::Event;
use wire::Refusal;

use super::stream::{
    ForwardContinuation, ForwardCursor, ForwardProgress, compressed, decode_window_row,
};
use super::work::SessionWork;
use super::{
    CompactionHorizon, CompactionRecord, CompactionSourceHorizon, DecodedRow, FrontierDirection,
    FrontierPage, FrontierProgress, MAX_LINE_BYTES, Marker, MessageRecord, ResolvedSource,
    ResultFrontier, TypedRecord, compaction_intent, compaction_text_bytes, frontier_limit,
    record_timestamp, refusal, text_bytes,
};
use crate::session_command::project::Filters;

const BACKWARD_CHUNK_BYTES: u64 = 64 * 1024;
const HORIZON_CONTEXT_LINES: usize = 20;

struct PendingCompaction {
    marker: Marker,
    assistant_before: String,
    summary: String,
    summary_seen: bool,
    user_after: String,
    user_seen: bool,
}

pub(super) struct CompactionTracker {
    stop_when_full: bool,
    last_assistant: Option<String>,
    last: Option<String>,
    pending: Option<PendingCompaction>,
    rows: ResultFrontier<CompactionRecord>,
    filter: Option<Filters>,
    start: Option<String>,
    end: Option<String>,
}

impl CompactionTracker {
    pub(super) fn new(
        limit: usize,
        direction: FrontierDirection,
        start: Option<&str>,
        end: Option<&str>,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            stop_when_full: matches!(direction, FrontierDirection::Oldest),
            last_assistant: None,
            last: None,
            pending: None,
            rows: ResultFrontier::new(limit, direction)?,
            filter: None,
            start: start.map(str::to_owned),
            end: end.map(str::to_owned),
        })
    }

    pub(super) fn observe(
        &mut self,
        markers: impl IntoIterator<Item = Marker>,
        messages: &[MessageRecord],
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        for marker in markers {
            self.finish_pending(work, at)?;
            if self.complete() {
                return Ok(());
            }
            if self.start.as_ref().is_none_or(|start| marker.ts >= *start)
                && self.end.as_ref().is_none_or(|end| marker.ts <= *end)
            {
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

    pub(super) fn seed_assistant(&mut self, assistant: String) {
        self.last_assistant = (!assistant.is_empty()).then_some(assistant);
    }

    pub(super) fn filter_with(&mut self, filter: Filters) {
        self.filter = Some(filter);
    }

    pub(super) fn observe_record(
        &mut self,
        record: &TypedRecord,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(), Refusal> {
        for sourced in &record.events {
            match &sourced.event {
                Event::Compaction(compaction) => {
                    let marker = Marker {
                        source: record.source.clone(),
                        position: record.line,
                        ts: record_timestamp(record).to_owned(),
                        summary: compaction.summary.clone().unwrap_or_default(),
                    };
                    work.admit_retained_text(super::marker_text_bytes(&marker), at)?;
                    self.observe(std::iter::once(marker), &[], work, at)?;
                }
                Event::Prose(prose) if !prose.text.trim().is_empty() => {
                    let side = match prose.role {
                        listen::ProseRole::Operator => "user",
                        listen::ProseRole::Assistant => "assistant",
                    };
                    let classified = crate::message_shape::classify(&prose.text);
                    if let Some(pending) = &mut self.pending
                        && record.line > pending.marker.position
                    {
                        if !pending.summary_seen
                            && classified.shape
                                == crate::message_shape::MessageShape::CompactionRehydration
                        {
                            pending.summary.clone_from(&prose.text);
                            pending.summary_seen = true;
                        }
                        if !pending.user_seen && side == "user" && classified.shape.is_authored() {
                            pending.user_after = classified.authored.unwrap_or_default();
                            pending.user_seen = true;
                        }
                    }
                    if side == "assistant" {
                        work.admit_retained_text(text_bytes([prose.text.as_str()]), at)?;
                        self.last_assistant = Some(prose.text.clone());
                    }
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.summary_seen && pending.user_seen)
                    {
                        self.finish_pending(work, at)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn complete(&self) -> bool {
        self.stop_when_full && self.pending.is_none() && self.rows.is_full()
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
        if self
            .filter
            .as_ref()
            .is_none_or(|filter| filter.timeline_compaction_matches(&row))
        {
            work.admit_retained_text(compaction_text_bytes(&row), at)?;
            self.rows.push(row);
        }
        Ok(())
    }

    /// Retained compactions, and every compaction the tracker matched.
    pub(super) fn finish(
        mut self,
        work: &mut SessionWork,
        at: &str,
    ) -> Result<(Vec<CompactionRecord>, usize), Refusal> {
        self.finish_pending(work, at)?;
        let population = self.rows.offered();
        Ok((self.rows.into_rows(), population))
    }
}

pub(super) fn admit_source_order(
    previous: &mut Option<String>,
    timestamp: &str,
) -> Result<(), Refusal> {
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

pub(crate) fn recent_compaction_horizon(
    sources: &[ResolvedSource],
    count: usize,
    end: Option<&str>,
) -> Result<CompactionHorizon, Refusal> {
    compaction_horizon(sources, count, end, false)
}

/// The recent horizon, and every compaction at or before `end` counted.
pub(crate) fn counted_compaction_horizon(
    sources: &[ResolvedSource],
    count: usize,
    end: Option<&str>,
) -> Result<CompactionHorizon, Refusal> {
    compaction_horizon(sources, count, end, true)
}

fn compaction_horizon(
    sources: &[ResolvedSource],
    count: usize,
    end: Option<&str>,
    exhaustive: bool,
) -> Result<CompactionHorizon, Refusal> {
    if count == 0 {
        return Ok(CompactionHorizon {
            start: None,
            total: exhaustive.then_some(0),
            sources: sources
                .iter()
                .map(|source| CompactionSourceHorizon {
                    has_compaction: false,
                    total: 0,
                    start_offset: None,
                    assistant_before: String::new(),
                    vendor: source.vendor,
                })
                .collect(),
        });
    }
    frontier_limit(count)?;
    let mut markers = Vec::new();
    let mut source_horizons = Vec::with_capacity(sources.len());
    let mut work = SessionWork::new(super::work::SessionWorkBudget::report());
    for source in sources {
        let (page, source_horizon) =
            read_recent_compaction_markers(source, count, end, exhaustive, &mut work)?;
        match page.progress {
            FrontierProgress::More(continuation) | FrontierProgress::Complete(continuation) => {
                let _ = continuation.position();
                markers.extend(page.rows);
            }
            FrontierProgress::Refused {
                continuation,
                error,
            } => {
                let _ = continuation.position();
                return Err(error);
            }
        }
        source_horizons.push(source_horizon);
    }
    markers.sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    let first = markers.len().saturating_sub(count);
    let total = exhaustive.then(|| {
        source_horizons
            .iter()
            .map(|source| source.total)
            .fold(0_usize, usize::saturating_add)
    });
    Ok(CompactionHorizon {
        start: markers.into_iter().nth(first).map(|marker| marker.ts),
        total,
        sources: source_horizons,
    })
}

fn read_recent_compaction_markers(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    exhaustive: bool,
    work: &mut SessionWork,
) -> Result<
    (
        FrontierPage<Marker, HorizonContinuation>,
        CompactionSourceHorizon,
    ),
    Refusal,
> {
    if compressed(&source.path) {
        return read_recent_compaction_markers_forward(source, count, end, work);
    }
    let source_name = source.path.display().to_string();
    let mut frontier = ResultFrontier::new(count, FrontierDirection::Oldest)?;
    let mut start_offset = None;
    let mut assistant_before = String::new();
    let mut vendor = source.vendor;
    let mut locating_assistant = false;
    let mut total = 0_usize;
    work.begin_pass(&source_name)?;
    let progress = reverse_lines(&source.path, work, |line, offset, _, terminal, work| {
        let live_tail = source.thread.is_some() && terminal;
        let at = format!("{source_name}:byte {offset}");
        let row = decode_reverse_row(source, line, offset, live_tail, work, &at)?;
        let typed = match row {
            Some(DecodedRow::Typed(row)) => {
                vendor = row.vendor;
                Some(row)
            }
            Some(DecodedRow::Residue(_)) | Some(DecodedRow::Ignored) | None => None,
        };
        let marker = typed.as_deref().and_then(marker_from_record);
        if locating_assistant {
            if marker
                .as_ref()
                .is_some_and(|marker| end.is_none_or(|end| marker.ts.as_str() <= end))
            {
                total = total.saturating_add(1);
            }
            if let Some(assistant) = typed.as_deref().and_then(last_assistant_from_record) {
                work.admit_retained_text(text_bytes([assistant.as_str()]), &at)?;
                assistant_before = assistant;
                locating_assistant = false;
                return Ok(exhaustive);
            }
            return Ok(true);
        }
        if let Some(marker) = marker
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            total = total.saturating_add(1);
            if total <= count {
                work.admit_retained_text(super::marker_text_bytes(&marker), &at)?;
                frontier.push(marker);
                start_offset = Some(offset);
                assistant_before.clear();
                if frontier.is_full() {
                    locating_assistant = true;
                }
            }
            return Ok(true);
        }
        if assistant_before.is_empty()
            && let Some(assistant) = typed.as_deref().and_then(last_assistant_from_record)
        {
            work.admit_retained_text(text_bytes([assistant.as_str()]), &at)?;
            assistant_before = assistant;
        }
        Ok(true)
    })?;
    let has_compaction = start_offset.is_some();
    let mut page = frontier.into_page(progress.map(HorizonContinuation::Reverse));
    page.rows.reverse();
    Ok((
        page,
        CompactionSourceHorizon {
            has_compaction,
            total,
            start_offset,
            assistant_before,
            vendor,
        },
    ))
}

fn read_recent_compaction_markers_forward(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    work: &mut SessionWork,
) -> Result<
    (
        FrontierPage<Marker, HorizonContinuation>,
        CompactionSourceHorizon,
    ),
    Refusal,
> {
    let mut frontier = ResultFrontier::new(count, FrontierDirection::Newest)?;
    let mut total = 0_usize;
    let mut cursor = ForwardCursor::new(source, None, work)?;
    let progress = loop {
        let window = match cursor.advance(work) {
            Ok(window) => window,
            Err(error) => {
                break FrontierProgress::Refused {
                    continuation: cursor.continuation(),
                    error,
                };
            }
        };
        let accepted = if let Some(DecodedRow::Typed(row)) = window.row
            && let Some(marker) = marker_from_record(&row)
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            total = total.saturating_add(1);
            work.admit_retained_text(
                super::marker_text_bytes(&marker),
                &format!(
                    "{}:record {}",
                    source.path.display(),
                    cursor.continuation().line
                ),
            )
            .map(|()| frontier.push(marker))
        } else {
            Ok(())
        };
        if let Err(error) = accepted {
            break FrontierProgress::Refused {
                continuation: cursor.continuation(),
                error,
            };
        }
        match window.progress {
            ForwardProgress::More(_) => {}
            ForwardProgress::Eof(end) => break FrontierProgress::Complete(end),
        }
    };
    let vendor = cursor.vendor();
    let page = frontier.into_page(progress.map(HorizonContinuation::Forward));
    let has_compaction = !page.rows.is_empty();
    Ok((
        page,
        CompactionSourceHorizon {
            has_compaction,
            total,
            start_offset: None,
            assistant_before: String::new(),
            vendor,
        },
    ))
}

pub(super) fn last_assistant_from_record(record: &TypedRecord) -> Option<String> {
    record.events.iter().rev().find_map(|sourced| {
        if let Event::Prose(prose) = &sourced.event
            && prose.role == listen::ProseRole::Assistant
            && !prose.text.trim().is_empty()
        {
            Some(prose.text.clone())
        } else {
            None
        }
    })
}

enum HorizonContinuation {
    Forward(ForwardContinuation),
    Reverse(ReverseContinuation),
}

impl<C> FrontierProgress<C> {
    fn map<D>(self, map: impl FnOnce(C) -> D + Copy) -> FrontierProgress<D> {
        match self {
            Self::More(continuation) => FrontierProgress::More(map(continuation)),
            Self::Complete(continuation) => FrontierProgress::Complete(map(continuation)),
            Self::Refused {
                continuation,
                error,
            } => FrontierProgress::Refused {
                continuation: map(continuation),
                error,
            },
        }
    }
}

impl HorizonContinuation {
    fn position(&self) -> u64 {
        match self {
            Self::Forward(continuation) => continuation.line,
            Self::Reverse(continuation) => continuation.position,
        }
    }
}

pub(super) fn start_offset_after_prior_timestamp(
    source: &ResolvedSource,
    file: File,
    transcript_length: u64,
    start: &str,
    work: &mut SessionWork,
) -> Result<u64, Refusal> {
    start_boundary(source, file, transcript_length, start, false, work).map(|(offset, _)| offset)
}

pub(super) fn start_offset_after_prior_assistant(
    source: &ResolvedSource,
    file: File,
    transcript_length: u64,
    start: &str,
    work: &mut SessionWork,
) -> Result<(u64, Option<String>), Refusal> {
    start_boundary(source, file, transcript_length, start, true, work)
}

fn start_boundary(
    source: &ResolvedSource,
    file: File,
    transcript_length: u64,
    start: &str,
    require_assistant: bool,
    work: &mut SessionWork,
) -> Result<(u64, Option<String>), Refusal> {
    let mut offset = 0_u64;
    let mut context: Option<usize> = None;
    let mut assistant = None;
    let cursor = ReverseCursor::from_file(file, &source.path, transcript_length);
    let progress = reverse_cursor(cursor, work, |line, current, _, _, work| {
        if let Some(remaining) = context.as_mut() {
            if *remaining > 0 {
                offset = current;
                *remaining -= 1;
            }
            if !require_assistant {
                return Ok(*remaining > 0);
            }
            let at = format!("{}:byte {current}", source.path.display());
            let typed =
                decode_reverse_row(source, line, current, false, work, &at)?.and_then(|row| {
                    match row {
                        DecodedRow::Typed(row) => Some(row),
                        DecodedRow::Residue(_) | DecodedRow::Ignored => None,
                    }
                });
            if assistant.is_none()
                && let Some(record) = typed.as_ref()
                && record_timestamp(record) < start
            {
                assistant = last_assistant_from_record(record);
            }
            return Ok(*remaining > 0 || assistant.is_none());
        }
        let at = format!("{}:byte {current}", source.path.display());
        let typed =
            decode_reverse_row(source, line, current, false, work, &at)?.and_then(
                |row| match row {
                    DecodedRow::Typed(row) => Some(row),
                    DecodedRow::Residue(_) | DecodedRow::Ignored => None,
                },
            );
        if typed
            .as_ref()
            .is_some_and(|record| record_timestamp(record) < start)
        {
            offset = current;
            context = Some(HORIZON_CONTEXT_LINES);
            if require_assistant {
                assistant = typed.as_deref().and_then(last_assistant_from_record);
            }
        }
        Ok(true)
    })?;
    if let FrontierProgress::Refused { error, .. } = progress {
        return Err(error);
    }
    Ok((offset, assistant))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReverseContinuation {
    pub(super) position: u64,
    pub(super) carry_bytes: usize,
}

pub(super) enum ReverseProgress {
    More(ReverseContinuation),
    Eof(ReverseContinuation),
}

#[derive(Clone, Copy)]
pub(super) struct ReverseRecordRange {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) offset: u64,
    pub(super) next: u64,
    pub(super) terminal: bool,
}

pub(super) struct ReverseWindow {
    pub(super) bytes: Vec<u8>,
    pub(super) records: Vec<ReverseRecordRange>,
    pub(super) progress: ReverseProgress,
}

pub(super) struct ReverseCursor {
    file: File,
    path: std::path::PathBuf,
    position: u64,
    transcript_length: u64,
    carry: Vec<u8>,
}

fn reverse_lines<F>(
    path: &Path,
    work: &mut SessionWork,
    visit: F,
) -> Result<FrontierProgress<ReverseContinuation>, Refusal>
where
    F: FnMut(&[u8], u64, u64, bool, &mut SessionWork) -> Result<bool, Refusal>,
{
    reverse_cursor(ReverseCursor::new(path)?, work, visit)
}

fn reverse_cursor<F>(
    mut cursor: ReverseCursor,
    work: &mut SessionWork,
    mut visit: F,
) -> Result<FrontierProgress<ReverseContinuation>, Refusal>
where
    F: FnMut(&[u8], u64, u64, bool, &mut SessionWork) -> Result<bool, Refusal>,
{
    loop {
        let ReverseWindow {
            bytes,
            records,
            progress,
        } = match cursor.advance(work) {
            Ok(window) => window,
            Err(error) => return Ok(refused(cursor.continuation(), error)),
        };
        for record in records.into_iter().rev() {
            let at = format!("{}:byte {}", cursor.path.display(), record.offset);
            if let Err(error) = work.admit_record(&at) {
                return Ok(refused(reverse_continuation(record.next), error));
            }
            let keep_going = match visit(
                &bytes[record.start..record.end],
                record.offset,
                record.next,
                record.terminal,
                work,
            ) {
                Ok(keep_going) => keep_going,
                Err(error) => return Ok(refused(reverse_continuation(record.next), error)),
            };
            if !keep_going && record.offset > 0 {
                return Ok(FrontierProgress::More(reverse_continuation(record.offset)));
            }
        }
        match progress {
            ReverseProgress::More(next) => debug_assert!(cursor.is_at(next)),
            ReverseProgress::Eof(end) => {
                debug_assert!(cursor.is_at(end));
                return Ok(FrontierProgress::Complete(end));
            }
        }
    }
}

fn reverse_continuation(position: u64) -> ReverseContinuation {
    ReverseContinuation {
        position,
        carry_bytes: 0,
    }
}

fn refused(
    continuation: ReverseContinuation,
    error: Refusal,
) -> FrontierProgress<ReverseContinuation> {
    FrontierProgress::Refused {
        continuation,
        error,
    }
}

impl ReverseCursor {
    pub(super) fn new(path: &Path) -> Result<Self, Refusal> {
        let file = File::open(path).map_err(|error| unreadable(path, &error))?;
        let position = file
            .metadata()
            .map_err(|error| unreadable(path, &error))?
            .len();
        Ok(Self::from_file(file, path, position))
    }

    fn from_file(file: File, path: &Path, transcript_length: u64) -> Self {
        Self {
            file,
            path: path.to_owned(),
            position: transcript_length,
            transcript_length,
            carry: Vec::new(),
        }
    }

    pub(super) fn advance(&mut self, work: &mut SessionWork) -> Result<ReverseWindow, Refusal> {
        if self.position == 0 {
            return Ok(self.window(Vec::new(), Vec::new()));
        }
        let at = format!("{}:byte {}", self.path.display(), self.position);
        work.check_deadline(&at)?;
        let read = self.position.min(BACKWARD_CHUNK_BYTES);
        work.admit_source_bytes(read, &at)?;
        let position = self.position - read;
        self.file
            .seek(SeekFrom::Start(position))
            .map_err(|error| unreadable(&self.path, &error))?;
        let mut block = vec![0_u8; read as usize];
        self.file
            .read_exact(&mut block)
            .map_err(|error| unreadable(&self.path, &error))?;
        admit_reverse_carry(&block, &self.carry, &self.path, position)?;
        block.extend_from_slice(&self.carry);
        self.carry.clear();
        self.position = position;
        let parse_start = if self.position == 0 {
            0
        } else if let Some(split) = block.iter().position(|byte| *byte == b'\n') {
            self.carry = block[..split].to_vec();
            split + 1
        } else {
            self.carry = block;
            return Ok(self.window(Vec::new(), Vec::new()));
        };
        let mut records = Vec::new();
        let mut start = parse_start;
        for (index, byte) in block.iter().enumerate().skip(parse_start) {
            if *byte == b'\n' {
                self.push_record(&block, &mut records, start, index, index + 1);
                start = index + 1;
            }
        }
        if start < block.len() {
            self.push_record(&block, &mut records, start, block.len(), block.len());
        }
        Ok(self.window(block, records))
    }

    fn continuation(&self) -> ReverseContinuation {
        ReverseContinuation {
            position: self.position,
            carry_bytes: self.carry.len(),
        }
    }

    fn push_record(
        &self,
        block: &[u8],
        records: &mut Vec<ReverseRecordRange>,
        start: usize,
        mut end: usize,
        next: usize,
    ) {
        while end > start && block[end - 1] == b'\r' {
            end -= 1;
        }
        if start != end {
            let next = self.position.saturating_add(next as u64);
            records.push(ReverseRecordRange {
                start,
                end,
                offset: self.position.saturating_add(start as u64),
                next,
                terminal: next == self.transcript_length,
            });
        }
    }

    fn window(&self, bytes: Vec<u8>, records: Vec<ReverseRecordRange>) -> ReverseWindow {
        let continuation = self.continuation();
        let progress = if self.position == 0 {
            ReverseProgress::Eof(continuation)
        } else {
            ReverseProgress::More(continuation)
        };
        ReverseWindow {
            bytes,
            records,
            progress,
        }
    }

    fn is_at(&self, continuation: ReverseContinuation) -> bool {
        self.position == continuation.position && self.carry.len() == continuation.carry_bytes
    }
}

fn unreadable(path: &Path, error: &std::io::Error) -> Refusal {
    refusal(
        "session::transcript-unreadable",
        Some(format!("{}: {error}", path.display())),
        "restore read access to the transcript and retry",
    )
}

fn admit_reverse_carry(
    block: &[u8],
    carry: &[u8],
    path: &Path,
    position: u64,
) -> Result<(), Refusal> {
    let suffix_start = block
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1);
    let terminal_cr = carry.last().or_else(|| block[suffix_start..].last()) == Some(&b'\r');
    let logical_bytes = block
        .len()
        .saturating_sub(suffix_start)
        .saturating_add(carry.len())
        .saturating_sub(usize::from(terminal_cr));
    if logical_bytes > MAX_LINE_BYTES {
        return Err(refusal(
            "session::transcript-record-oversized",
            Some(format!(
                "{}:byte {} exceeds {MAX_LINE_BYTES} bytes",
                path.display(),
                position.saturating_add(suffix_start as u64)
            )),
            "restore a bounded transcript record or select an earlier intact copy",
        ));
    }
    Ok(())
}

fn marker_from_record(record: &TypedRecord) -> Option<Marker> {
    record.events.iter().find_map(|sourced| {
        if let Event::Compaction(compaction) = &sourced.event {
            Some(Marker {
                source: record.source.clone(),
                position: record.line,
                ts: record_timestamp(record).to_owned(),
                summary: compaction.summary.clone().unwrap_or_default(),
            })
        } else {
            None
        }
    })
}

fn decode_reverse_row(
    source: &ResolvedSource,
    line: &[u8],
    offset: u64,
    live_tail: bool,
    work: &mut SessionWork,
    at: &str,
) -> Result<Option<DecodedRow>, Refusal> {
    decode_window_row(source, offset, line, source.dialect, live_tail, work, at).map(|(_, row)| row)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::stream::{ForwardCursor, stream_records};
    use super::super::work::SessionWorkBudget;
    use super::super::{WindowOptions, WindowScan};
    use super::*;
    use crate::session_command::model::Vendor;
    use crate::session_command::project::Filters;

    #[test]
    fn bounded_stream_and_reverse_horizon_share_canonical_complete_records() {
        let source = fixture_source();
        let horizon = recent_compaction_horizon(std::slice::from_ref(&source), 3, None)
            .expect("bounded horizon scans");
        assert_eq!(horizon.start.as_deref(), Some("2026-01-01T00:00:01.000Z"));
        assert_fold_accounting(&source);
        assert_budget_refusals(&source);
        assert_exact_generation(&source);
        fs::remove_file(source.path).expect("fixture transcript is removed");
    }

    fn fixture_source() -> ResolvedSource {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spice-session-horizon-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"compaction\",\"id\":\"c1\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"summary\":\"s\",\"firstKeptEntryId\":\"e0\",\"tokensBefore\":10}\n",
                "{\"type\":\"message\",\"id\":\"u\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"go\"}]}}\n",
                "{\"type\":\"message\",\"id\":\"a\",\"timestamp\":\"2026-01-01T00:00:03Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"x\"}],\"stopReason\":\"toolUse\"}}\n",
                "{\"type\":\"message\",\"id\":\"t\",\"timestamp\":\"2026-01-01T00:00:04Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"c\",\"name\":\"bash\",\"arguments\":{\"command\":\"\"}}],\"stopReason\":\"toolUse\"}}\n",
                "{\"type\":\"compaction\",\"id\":\"c2\",\"timestamp\":\"2026-01-01T00:00:05Z\"",
            ),
        )
        .expect("live transcript is written");
        ResolvedSource {
            path,
            vendor: Vendor::UNKNOWN,
            dialect: Some(listen::Dialect::Omp),
            thread: Some("018f7777-7777-7444-8888-999999999999".to_owned()),
        }
    }

    fn assert_fold_accounting(source: &ResolvedSource) {
        let scan = WindowScan::scan(
            std::slice::from_ref(source),
            None,
            None,
            &Filters::default(),
            WindowOptions::Turns {
                limit: 8,
                full: true,
                max_text: 240,
            },
        )
        .expect("bounded window folds canonical records");
        assert_eq!(scan.sources.len(), 1);
        assert_eq!(scan.turns.len(), 1);
        assert_eq!(scan.turns[0].view.commands, 1);
        assert_eq!(scan.compactions.len(), 1);
        assert_eq!(scan.residues[0].kind, "live-tail-truncated");
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let progress = reverse_lines(&source.path, &mut work, |_, _, _, _, _| Ok(false))
            .expect("reverse cursor stops at a record boundary");
        assert!(matches!(progress, FrontierProgress::More(_)));
    }

    fn assert_budget_refusals(source: &ResolvedSource) {
        let deadline = std::time::Duration::from_secs(1);
        let budget = SessionWorkBudget::new(1, 1, 1, 1, 1, deadline).expect("positive budget");
        let mut exhausted = SessionWork::new(budget);
        let refusal = stream_records(source, None, &mut exhausted, |_, _, _| Ok(()))
            .expect_err("source bytes stop before the first record");
        assert_eq!(refusal.reason(), "session::transcript-work-exhausted");
        assert_eq!(exhausted.report().source_bytes, 0);
        let mut reverse_exhausted = SessionWork::new(
            SessionWorkBudget::new(1, 1, 1, 1, 1, deadline).expect("positive budget"),
        );
        let progress = reverse_lines(&source.path, &mut reverse_exhausted, |_, _, _, _, _| {
            Ok(true)
        })
        .expect("reverse refusal");
        assert!(matches!(progress, FrontierProgress::Refused { .. }));
        assert!(SessionWorkBudget::new(0, 1, 1, 1, 1, deadline).is_err());
    }

    fn assert_exact_generation(source: &ResolvedSource) {
        let captured = File::open(&source.path).expect("capture original generation");
        let displaced = source.path.with_extension("captured");
        fs::rename(&source.path, &displaced).expect("replace source path");
        fs::write(&source.path, b"replacement generation\n").expect("write replacement");
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut cursor = ForwardCursor::with_preceding_assistant_from_file(
            source,
            captured,
            "2026-01-01T00:00:03.500Z",
            &mut work,
        )
        .expect("captured generation locates its own boundary");
        let assistant = (0..4).find_map(|_| {
            cursor.advance(&mut work).expect("captured fold advances");
            cursor.take_preceding_assistant()
        });
        assert_eq!(assistant.as_deref(), Some("x"));
        drop(cursor);

        let captured = File::open(&displaced).expect("recapture original generation");
        let mut cursor = ForwardCursor::with_preceding_assistant_from_file(
            source,
            captured,
            "2026-01-01T00:00:03.500Z",
            &mut work,
        )
        .expect("capture before truncation");
        OpenOptions::new()
            .write(true)
            .open(&displaced)
            .expect("open captured source for truncation")
            .set_len(0)
            .expect("truncate captured source");
        let Err(refusal) = cursor.advance(&mut work) else {
            panic!("truncation must refuse")
        };
        assert_eq!(refusal.reason(), "session::transcript-generation-changed");

        let compressed = ResolvedSource {
            path: source.path.with_extension("jsonl.gz"),
            ..source.clone()
        };
        fs::write(&compressed.path, b"stationary malformed gzip")
            .expect("write malformed compressed source");
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut cursor = ForwardCursor::with_preceding_assistant(
            &compressed,
            Some("2026-01-01T00:00:03.500Z"),
            &mut work,
        )
        .expect("capture malformed compressed generation");
        let window = cursor
            .advance(&mut work)
            .expect("stationary gzip corruption is not generation movement");
        assert!(matches!(window.row, Some(DecodedRow::Residue(_))));
        fs::remove_file(compressed.path).expect("remove malformed compressed source");
        fs::remove_file(&source.path).expect("remove replacement generation");
        fs::rename(displaced, &source.path).expect("restore original source path");
    }
}
