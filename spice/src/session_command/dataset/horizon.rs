use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use listen::{Decoded, Event, RecordDecodeError, ResidueKind};
use wire::Refusal;

use super::stream::{compressed, stream_records};
use super::work::SessionWork;
use super::{
    CompactionHorizon, DecodedRow, MAX_LINE_BYTES, Marker, ResolvedSource, TypedRecord,
    record_refusal, record_timestamp, refusal,
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
    let mut markers = Vec::new();
    let mut work = SessionWork::new(super::work::SessionWorkBudget::report());
    for source in sources {
        markers.extend(read_recent_compaction_markers(
            source, count, end, &mut work,
        )?);
    }
    markers.sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    let first = markers.len().saturating_sub(count);
    Ok(CompactionHorizon {
        start: markers.into_iter().nth(first).map(|marker| marker.ts),
    })
}

fn read_recent_compaction_markers(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    work: &mut SessionWork,
) -> Result<Vec<Marker>, Refusal> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if compressed(&source.path) {
        return read_recent_compaction_markers_forward(source, count, end, work);
    }
    let source_name = source.path.display().to_string();
    let mut markers = Vec::new();
    work.begin_pass(&source_name)?;
    reverse_lines(&source.path, work, |line, offset, _, terminal, work| {
        let live_tail = source.thread.is_some() && terminal;
        if let Some(marker) = marker_from_line(source, &source_name, line, offset, live_tail, work)?
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            markers.push(marker);
            if markers.len() == count {
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    markers.reverse();
    Ok(markers)
}

fn read_recent_compaction_markers_forward(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
    work: &mut SessionWork,
) -> Result<Vec<Marker>, Refusal> {
    let mut markers = Vec::new();
    stream_records(source, None, work, |row, work, at| {
        if let DecodedRow::Typed(row) = row
            && let Some(marker) = marker_from_record(&row)
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            work.admit_retained_text(super::marker_text_bytes(&marker), at)?;
            markers.push(marker);
            trim_oldest(&mut markers, count);
        }
        Ok(())
    })?;
    Ok(markers)
}

pub(super) fn start_offset_after_prior_timestamp(
    source: &ResolvedSource,
    start: &str,
    work: &mut SessionWork,
) -> Result<u64, Refusal> {
    let mut offset = 0_u64;
    let mut context = None;
    reverse_lines(&source.path, work, |line, current, _, _, work| {
        if let Some(remaining) = context.as_mut() {
            offset = current;
            *remaining -= 1;
            return Ok(*remaining > 0);
        }
        if timestamp_from_line(source, line, current, work)?.is_some_and(|ts| ts.as_str() < start) {
            offset = current;
            context = Some(HORIZON_CONTEXT_LINES);
        }
        Ok(true)
    })?;
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

fn reverse_lines<F>(path: &Path, work: &mut SessionWork, mut visit: F) -> Result<(), Refusal>
where
    F: FnMut(&[u8], u64, u64, bool, &mut SessionWork) -> Result<bool, Refusal>,
{
    let mut cursor = ReverseCursor::new(path)?;
    loop {
        let ReverseWindow {
            bytes,
            records,
            progress,
        } = cursor.advance(work)?;
        for record in records.into_iter().rev() {
            let at = format!("{}:byte {}", path.display(), record.offset);
            work.admit_record(&at)?;
            if !visit(
                &bytes[record.start..record.end],
                record.offset,
                record.next,
                record.terminal,
                work,
            )? {
                return Ok(());
            }
        }
        match progress {
            ReverseProgress::More(next) => debug_assert!(cursor.is_at(next)),
            ReverseProgress::Eof(end) => {
                debug_assert!(cursor.is_at(end));
                return Ok(());
            }
        }
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
        self.position -= read;
        self.file
            .seek(SeekFrom::Start(self.position))
            .map_err(|error| unreadable(&self.path, &error))?;
        let mut block = vec![0_u8; read as usize];
        self.file
            .read_exact(&mut block)
            .map_err(|error| unreadable(&self.path, &error))?;
        let carry = std::mem::take(&mut self.carry);
        admit_reverse_carry(&block, &carry, &self.path, self.position)?;
        block.extend_from_slice(&carry);
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
        let continuation = ReverseContinuation {
            position: self.position,
            carry_bytes: self.carry.len(),
        };
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

fn marker_from_line(
    source: &ResolvedSource,
    source_name: &str,
    line: &[u8],
    offset: u64,
    live_tail: bool,
    work: &mut SessionWork,
) -> Result<Option<Marker>, Refusal> {
    if line.len() > MAX_LINE_BYTES {
        return Err(refusal(
            "session::transcript-record-oversized",
            Some(format!(
                "{}:byte {offset} exceeds {MAX_LINE_BYTES} bytes",
                source.path.display()
            )),
            "restore a bounded transcript record or select an earlier intact copy",
        ));
    }
    let locus = listen::Locus::new(1, 0, line.len() as u64);
    let detected = match listen::decode_record(source.dialect, line, locus) {
        Ok(detected) => detected,
        Err(RecordDecodeError::Invalid {
            kind: ResidueKind::Truncated,
        }) if live_tail => return Ok(None),
        Err(error) => return Err(record_refusal(&source.path, 1, &error)),
    };
    let Decoded::Events { context, events } = detected.decoded else {
        return Ok(None);
    };
    let at = format!("{source_name}:byte {offset}");
    work.admit_decoded(events.len(), &at)?;
    let Some(raw_ts) = context.timestamp.as_deref() else {
        return Ok(None);
    };
    let Some(timestamp) = spice::session_meter::normalize_timestamp(raw_ts) else {
        return Ok(None);
    };
    let marker = events.into_iter().find_map(|event| {
        if let Event::Compaction(compaction) = event {
            Some(Marker {
                source: source_name.to_owned(),
                position: offset,
                ts: timestamp.to_string(),
                summary: compaction.summary.unwrap_or_default(),
            })
        } else {
            None
        }
    });
    if let Some(marker) = &marker {
        work.admit_retained_text(
            super::work::text_bytes([
                marker.source.as_str(),
                marker.ts.as_str(),
                marker.summary.as_str(),
            ]),
            &at,
        )?;
    }
    Ok(marker)
}

fn timestamp_from_line(
    source: &ResolvedSource,
    line: &[u8],
    offset: u64,
    work: &mut SessionWork,
) -> Result<Option<String>, Refusal> {
    if line.len() > MAX_LINE_BYTES {
        return Ok(None);
    }
    let locus = listen::Locus::new(1, 0, line.len() as u64);
    let Ok(detected) = listen::decode_record(source.dialect, line, locus) else {
        return Ok(None);
    };
    let Decoded::Events { context, events } = detected.decoded else {
        return Ok(None);
    };
    work.admit_decoded(
        events.len(),
        &format!("{}:byte {offset}", source.path.display()),
    )?;
    Ok(context
        .timestamp
        .as_deref()
        .and_then(spice::session_meter::normalize_timestamp)
        .map(|timestamp| timestamp.to_string()))
}

fn trim_oldest<T>(rows: &mut Vec<T>, count: usize) {
    if rows.len() > count {
        rows.drain(..rows.len() - count);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::work::{ResidueLedger, SessionWorkBudget, text_bytes};
    use super::super::{
        FoldState, command_text_bytes, finish_fold, message_text_bytes, reduce_record,
    };
    use super::*;
    use crate::session_command::model::{Dataset, Vendor};

    #[test]
    fn bounded_stream_and_reverse_horizon_share_canonical_complete_records() {
        let source = fixture_source();
        let horizon = recent_compaction_horizon(std::slice::from_ref(&source), 3, None)
            .expect("bounded horizon scans");
        assert_eq!(horizon.start.as_deref(), Some("2026-01-01T00:00:01.000Z"));
        assert_reverse_record_boundary(&source);
        assert_fold_accounting(&source);
        assert_budget_refusals(&source);
        fs::remove_file(source.path).expect("fixture transcript is removed");
    }

    fn assert_reverse_record_boundary(source: &ResolvedSource) {
        let path = source.path.with_extension("boundary");
        let mut work = SessionWork::new(SessionWorkBudget::report());
        for ending in [b"\n".as_slice(), b"\r\n".as_slice()] {
            let mut record = vec![b'x'; MAX_LINE_BYTES];
            record.extend_from_slice(ending);
            fs::write(&path, &record).expect("exact-limit record is written");
            reverse_lines(&path, &mut work, |line, offset, _, _, _| {
                assert_eq!((line.len(), offset), (MAX_LINE_BYTES, 0));
                Ok(true)
            })
            .expect("exact-limit LF or CRLF record is admitted");
        }
        let mut record = vec![b'x'; MAX_LINE_BYTES + 1];
        record.push(b'\n');
        fs::write(&path, record).expect("oversized record is written");
        let refusal = reverse_lines(&path, &mut work, |_, _, _, _, _| Ok(true))
            .expect_err("one logical byte over the limit is refused before append");
        assert_eq!(refusal.reason(), "session::transcript-record-oversized");
        fs::remove_file(path).expect("boundary transcript is removed");
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
        let mut fold = FoldState::default();
        stream_records(source, None, &mut work, |row, work, at| {
            match row {
                DecodedRow::Typed(record) => {
                    let mut storage = super::super::FoldStorage {
                        dataset: &mut dataset,
                        markers: &mut markers,
                        residues: &mut residues,
                        state: &mut fold,
                    };
                    reduce_record(&record, &mut storage, work, at)?;
                }
                DecodedRow::Residue(residue) => residues.push(residue, work, at)?,
                DecodedRow::Ignored => {}
            }
            Ok(())
        })
        .expect("forward dataset streams");
        let derived_floor = message_text_bytes(&dataset.messages[0])
            .saturating_mul(2)
            .saturating_add(command_text_bytes(&dataset.commands[0]))
            .saturating_add(text_bytes(["exec_command", "c"]));
        finish_fold(&mut dataset, fold, &mut work, "fixture fold").expect("fold finishes");
        let residues = residues
            .finish(&mut work, "residue summary")
            .expect("residue summary fits");
        assert_eq!(markers.len(), 1);
        assert_eq!(dataset.messages.len(), 1);
        assert_eq!(dataset.turns[0].messages, dataset.messages);
        assert_eq!(dataset.commands.len(), 1);
        assert_eq!(residues.len(), 1);
        assert_eq!(residues[0].kind, "live-tail-truncated");
        let report = work.report();
        assert_eq!((report.records, report.passes), (5, 1));
        assert!(
            report.source_bytes > 0
                && report.events > 0
                && report.retained_text_bytes >= derived_floor
        );
    }

    fn assert_budget_refusals(source: &ResolvedSource) {
        let deadline = std::time::Duration::from_secs(1);
        let budget = SessionWorkBudget::new(1, 1, 1, 1, 1, deadline).expect("positive budget");
        let mut exhausted = SessionWork::new(budget);
        let refusal = stream_records(source, None, &mut exhausted, |_, _, _| Ok(()))
            .expect_err("source bytes stop before the first record");
        assert_eq!(refusal.reason(), "session::transcript-work-exhausted");
        assert_eq!(exhausted.report().source_bytes, 0);
        let retained_budget =
            SessionWorkBudget::new(1_048_576, 16, 16, 1, 1, deadline).expect("positive budget");
        let mut exhausted = SessionWork::new(retained_budget);
        let mut dataset = Dataset::default();
        let mut markers = Vec::new();
        let mut residues = ResidueLedger::new();
        let mut fold = FoldState::default();
        let refusal = stream_records(source, None, &mut exhausted, |row, work, at| match row {
            DecodedRow::Typed(record) => {
                let mut storage = super::super::FoldStorage {
                    dataset: &mut dataset,
                    markers: &mut markers,
                    residues: &mut residues,
                    state: &mut fold,
                };
                reduce_record(&record, &mut storage, work, at)
            }
            DecodedRow::Residue(residue) => residues.push(residue, work, at),
            DecodedRow::Ignored => Ok(()),
        })
        .expect_err("the first retained String exceeds the one-byte budget");
        assert_eq!(refusal.reason(), "session::transcript-work-exhausted");
        assert_eq!(exhausted.report().retained_text_bytes, 0);
        assert!(SessionWorkBudget::new(0, 1, 1, 1, 1, deadline).is_err());
    }
}
