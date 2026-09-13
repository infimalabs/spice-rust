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
    CompactionHorizon, DecodedRow, FrontierDirection, FrontierPage, FrontierProgress,
    MAX_LINE_BYTES, Marker, ResolvedSource, ResultFrontier, TypedRecord, frontier_limit,
    record_timestamp, refusal,
};

const BACKWARD_CHUNK_BYTES: u64 = 64 * 1024;
const HORIZON_CONTEXT_LINES: usize = 20;

pub(crate) fn recent_compaction_horizon(
    sources: &[ResolvedSource],
    count: usize,
    end: Option<&str>,
) -> Result<CompactionHorizon, Refusal> {
    if count == 0 {
        return Ok(CompactionHorizon { start: None });
    }
    frontier_limit(count)?;
    let mut markers = Vec::new();
    let mut work = SessionWork::new(super::work::SessionWorkBudget::report());
    for source in sources {
        let page = read_recent_compaction_markers(source, count, end, &mut work)?;
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
    }
    markers.sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    let first = markers.len().saturating_sub(count);
    Ok(CompactionHorizon {
        start: markers.into_iter().nth(first).map(|marker| marker.ts),
    })
}

pub(crate) fn recent_turn_start(
    source: &ResolvedSource,
    count: usize,
    work: &mut SessionWork,
) -> Result<Option<String>, Refusal> {
    debug_assert!(count > 0);
    let source_name = source.path.display().to_string();
    work.begin_pass(&source_name)?;
    let mut cursor = ReverseCursor::new(&source.path)?;
    let mut observed = 0_usize;
    loop {
        let ReverseWindow {
            bytes,
            records,
            progress,
        } = cursor.advance(work)?;
        let mut oldest_start = None;
        for record in records.into_iter().rev() {
            let at = format!("{source_name}:byte {}", record.offset);
            work.admit_record(&at)?;
            let row = decode_reverse_row(
                source,
                &bytes[record.start..record.end],
                record.offset,
                source.thread.is_some() && record.terminal,
                work,
                &at,
            )?;
            if let Some(DecodedRow::Typed(row)) = row
                && row.context.turn == Some(listen::TurnMoment::Started)
            {
                observed = observed.saturating_add(1);
                oldest_start = Some(record_timestamp(&row).to_owned());
            }
        }
        if observed >= count {
            return Ok(oldest_start);
        }
        match progress {
            ReverseProgress::More(next) => debug_assert!(cursor.is_at(next)),
            ReverseProgress::Eof(end) => {
                debug_assert!(cursor.is_at(end));
                return Ok(None);
            }
        }
    }
}

fn read_recent_compaction_markers(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    work: &mut SessionWork,
) -> Result<FrontierPage<Marker, HorizonContinuation>, Refusal> {
    if compressed(&source.path) {
        return read_recent_compaction_markers_forward(source, count, end, work);
    }
    let source_name = source.path.display().to_string();
    let mut frontier = ResultFrontier::new(count, FrontierDirection::Oldest)?;
    work.begin_pass(&source_name)?;
    let progress = reverse_lines(&source.path, work, |line, offset, _, terminal, work| {
        let live_tail = source.thread.is_some() && terminal;
        let at = format!("{source_name}:byte {offset}");
        let marker = decode_reverse_row(source, line, offset, live_tail, work, &at)?.and_then(
            |row| match row {
                DecodedRow::Typed(row) => marker_from_record(&row),
                DecodedRow::Residue(_) | DecodedRow::Ignored => None,
            },
        );
        if let Some(marker) = marker
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            work.admit_retained_text(super::marker_text_bytes(&marker), &at)?;
            frontier.push(marker);
        }
        Ok(frontier.rows.len() < count)
    })?;
    let mut page = frontier.into_page(progress.map(HorizonContinuation::Reverse));
    page.rows.reverse();
    Ok(page)
}

fn read_recent_compaction_markers_forward(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    work: &mut SessionWork,
) -> Result<FrontierPage<Marker, HorizonContinuation>, Refusal> {
    let mut frontier = ResultFrontier::new(count, FrontierDirection::Newest)?;
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
    Ok(frontier.into_page(progress.map(HorizonContinuation::Forward)))
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
    start: &str,
    work: &mut SessionWork,
) -> Result<u64, Refusal> {
    let mut offset = 0_u64;
    let mut context = None;
    let progress = reverse_lines(&source.path, work, |line, current, _, _, work| {
        if let Some(remaining) = context.as_mut() {
            offset = current;
            *remaining -= 1;
            return Ok(*remaining > 0);
        }
        let at = format!("{}:byte {current}", source.path.display());
        let timestamp = decode_reverse_row(source, line, current, false, work, &at)?.and_then(
            |row| match row {
                DecodedRow::Typed(row) => Some(record_timestamp(&row).to_owned()),
                DecodedRow::Residue(_) | DecodedRow::Ignored => None,
            },
        );
        if timestamp.is_some_and(|ts| ts.as_str() < start) {
            offset = current;
            context = Some(HORIZON_CONTEXT_LINES);
        }
        Ok(true)
    })?;
    if let FrontierProgress::Refused { error, .. } = progress {
        return Err(error);
    }
    Ok(offset)
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
    mut visit: F,
) -> Result<FrontierProgress<ReverseContinuation>, Refusal>
where
    F: FnMut(&[u8], u64, u64, bool, &mut SessionWork) -> Result<bool, Refusal>,
{
    let mut cursor = ReverseCursor::new(path)?;
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
            let at = format!("{}:byte {}", path.display(), record.offset);
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
        Ok(Self {
            file,
            path: path.to_owned(),
            position,
            transcript_length: position,
            carry: Vec::new(),
        })
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
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::WindowFold;
    use super::super::stream::stream_records;
    use super::super::work::{ResidueLedger, SessionWorkBudget};
    use super::*;
    use crate::session_command::model::{Dataset, Vendor};

    #[test]
    fn bounded_stream_and_reverse_horizon_share_canonical_complete_records() {
        let source = fixture_source();
        let horizon = recent_compaction_horizon(std::slice::from_ref(&source), 3, None)
            .expect("bounded horizon scans");
        assert_eq!(horizon.start.as_deref(), Some("2026-01-01T00:00:01.000Z"));
        assert_fold_accounting(&source);
        assert_budget_refusals(&source);
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
                "{\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"compacted\",\"payload\":{}}\n",
                "{\"timestamp\":\"2026-01-01T00:00:02Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"context_compacted\"}}\n",
                "{\"timestamp\":\"2026-01-01T00:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"x\"}]}}\n",
                "{\"timestamp\":\"2026-01-01T00:00:04Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"call_id\":\"c\",\"arguments\":\"{\\\"cmd\\\":\\\"\\\"}\"}}\n",
                "{\"timestamp\":\"2026-01-01T00:00:05Z\",\"type\":\"compacted\"",
            ),
        )
        .expect("live transcript is written");
        ResolvedSource {
            path,
            vendor: Vendor::UNKNOWN,
            dialect: Some(listen::Dialect::Codex),
            thread: Some("018f7777-7777-7444-8888-999999999999".to_owned()),
        }
    }

    fn assert_fold_accounting(source: &ResolvedSource) {
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut dataset = Dataset::default();
        let mut markers = Vec::new();
        let mut residues = ResidueLedger::new();
        let mut fold = WindowFold::new(&mut dataset, &mut markers, &mut residues);
        stream_records(source, None, &mut work, |row, work, at| {
            fold.push(row, work, at)
        })
        .expect("forward dataset streams");
        fold.finish(&mut work, "fixture fold")
            .expect("fold finishes");
        let residues = residues
            .finish(&mut work, "residue summary")
            .expect("residue summary fits");
        assert_eq!(residues[0].kind, "live-tail-truncated");
        let mut reverse_work = SessionWork::new(SessionWorkBudget::report());
        reverse_work.begin_pass("reverse").expect("pass starts");
        let mut selected = ResultFrontier::new(8, FrontierDirection::Oldest).unwrap();
        let progress = reverse_lines(
            &source.path,
            &mut reverse_work,
            |line, offset, _, tail, _| {
                selected.push((line.to_vec(), offset, tail));
                Ok(true)
            },
        )
        .expect("reverse windows select");
        let page = selected.into_page(progress);
        let mut reverse = Dataset::default();
        let mut reverse_markers = Vec::new();
        let mut reverse_residues = ResidueLedger::new();
        let mut reverse_fold =
            WindowFold::new(&mut reverse, &mut reverse_markers, &mut reverse_residues);
        let mut dialect = source.dialect;
        for (row, offset, tail) in page.rows.into_iter().rev() {
            let (observed, row) = decode_window_row(
                source,
                offset,
                &row,
                dialect,
                tail,
                &mut reverse_work,
                "row",
            )
            .expect("row decodes");
            dialect = observed.or(dialect);
            if let Some(row) = row {
                reverse_fold
                    .push(row, &mut reverse_work, "row")
                    .expect("row folds");
            }
        }
        reverse_fold
            .finish(&mut reverse_work, "fold")
            .expect("fold finishes");
        let reverse_residues = reverse_residues
            .finish(&mut reverse_work, "residues")
            .unwrap();
        assert!(matches!(page.progress, FrontierProgress::Complete(_)));
        assert_eq!(reverse.turns.len(), dataset.turns.len());
        assert_eq!(reverse.messages.len(), dataset.messages.len());
        assert_eq!(reverse.commands.len(), dataset.commands.len());
        assert_eq!((reverse_markers.len(), reverse_residues.len()), (1, 1));
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
        let retained_budget =
            SessionWorkBudget::new(1_048_576, 16, 16, 1, 1, deadline).expect("positive budget");
        let mut exhausted = SessionWork::new(retained_budget);
        let mut dataset = Dataset::default();
        let mut markers = Vec::new();
        let mut residues = ResidueLedger::new();
        let mut fold = WindowFold::new(&mut dataset, &mut markers, &mut residues);
        let refusal = stream_records(source, None, &mut exhausted, |row, work, at| {
            fold.push(row, work, at)
        })
        .expect_err("the first retained String exceeds the one-byte budget");
        assert_eq!(refusal.reason(), "session::transcript-work-exhausted");
        assert_eq!(exhausted.report().retained_text_bytes, 0);
        assert!(SessionWorkBudget::new(0, 1, 1, 1, 1, deadline).is_err());
    }
}
