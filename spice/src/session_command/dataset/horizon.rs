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

fn reverse_lines<F>(path: &Path, work: &mut SessionWork, mut visit: F) -> Result<(), Refusal>
where
    F: FnMut(&[u8], u64, u64, bool, &mut SessionWork) -> Result<bool, Refusal>,
{
    let mut file = File::open(path).map_err(|error| {
        refusal(
            "session::transcript-unreadable",
            Some(format!("{}: {error}", path.display())),
            "restore read access to the transcript and retry",
        )
    })?;
    let mut position = file
        .metadata()
        .map_err(|error| {
            refusal(
                "session::transcript-unreadable",
                Some(format!("{}: {error}", path.display())),
                "restore read access to the transcript and retry",
            )
        })?
        .len();
    let transcript_length = position;
    let mut carry = Vec::new();
    while position > 0 {
        let at = format!("{}:byte {position}", path.display());
        work.check_deadline(&at)?;
        let read = position.min(BACKWARD_CHUNK_BYTES);
        work.admit_source_bytes(read, &at)?;
        position -= read;
        file.seek(SeekFrom::Start(position)).map_err(|error| {
            refusal(
                "session::transcript-unreadable",
                Some(format!("{}: {error}", path.display())),
                "restore read access to the transcript and retry",
            )
        })?;
        let mut block = vec![0_u8; read as usize];
        file.read_exact(&mut block).map_err(|error| {
            refusal(
                "session::transcript-unreadable",
                Some(format!("{}: {error}", path.display())),
                "restore read access to the transcript and retry",
            )
        })?;
        block.extend_from_slice(&carry);
        let parse_start = if position == 0 {
            0
        } else if let Some(split) = block.iter().position(|byte| *byte == b'\n') {
            carry = block[..split].to_vec();
            split + 1
        } else {
            carry = block;
            continue;
        };
        let mut ranges = Vec::new();
        let mut line_start = parse_start;
        for (index, byte) in block.iter().enumerate().skip(parse_start) {
            if *byte == b'\n' {
                ranges.push((line_start, index, index + 1));
                line_start = index + 1;
            }
        }
        if line_start < block.len() {
            ranges.push((line_start, block.len(), block.len()));
        }
        for (start, mut end, next) in ranges.into_iter().rev() {
            while end > start && block[end - 1] == b'\r' {
                end -= 1;
            }
            if start == end {
                continue;
            }
            let next = position.saturating_add(next as u64);
            let at = format!(
                "{}:byte {}",
                path.display(),
                position.saturating_add(start as u64)
            );
            work.admit_record(&at)?;
            if !visit(
                &block[start..end],
                position.saturating_add(start as u64),
                next,
                next == transcript_length,
                work,
            )? {
                return Ok(());
            }
        }
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
    use crate::session_command::model::Dataset;
    use crate::session_command::model::Vendor;

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
                "{\"timestamp\":\"2026-01-01T00:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"working\"}]}}\n",
                "{\"timestamp\":\"2026-01-01T00:00:04Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"exec_command\",\"call_id\":\"command-one\",\"arguments\":\"{\\\"cmd\\\":\\\"cargo test\\\"}\"}}\n",
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
            .saturating_add(text_bytes(["exec_command", "command-one"]));
        finish_fold(&mut dataset, fold, &mut work, "fixture fold").expect("fold finishes");
        let residues = residues
            .finish(&mut work, "residue summary")
            .expect("residue summary fits");
        assert_eq!(
            markers.len(),
            1,
            "the forward decoder sees the canonical marker"
        );
        assert_eq!(dataset.messages.len(), 1);
        assert_eq!(dataset.turns[0].messages, dataset.messages);
        assert_eq!(dataset.commands.len(), 1);
        assert_eq!(residues.len(), 1);
        assert_eq!(residues[0].kind, "live-tail-truncated");
        let report = work.report();
        assert_eq!(report.records, 5);
        assert!(report.source_bytes > 0);
        assert!(report.events > 0);
        assert!(report.retained_text_bytes >= derived_floor);
        assert_eq!(report.passes, 1);
    }

    fn assert_budget_refusals(source: &ResolvedSource) {
        let budget = SessionWorkBudget::new(1, 1, 1, 1, 1, std::time::Duration::from_secs(1))
            .expect("tiny positive budget is admitted");
        let mut exhausted = SessionWork::new(budget);
        let refusal = stream_records(source, None, &mut exhausted, |_, _, _| Ok(()))
            .expect_err("source bytes stop before the first record");
        assert_eq!(refusal.reason(), "session::transcript-work-exhausted");
        assert_eq!(exhausted.report().source_bytes, 0);

        let retained_budget = SessionWorkBudget::new(
            1_024 * 1_024,
            16,
            16,
            1,
            1,
            std::time::Duration::from_secs(1),
        )
        .expect("retained-text boundary is positive");
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

        assert!(SessionWorkBudget::new(0, 1, 1, 1, 1, std::time::Duration::from_secs(1),).is_err());
    }
}
