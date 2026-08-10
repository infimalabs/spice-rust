use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use listen::{Decoded, Event};
use wire::Refusal;

use super::{
    CompactionHorizon, DecodedRow, MAX_LINE_BYTES, Marker, Residue, ResolvedSource, TypedRecord,
    Vendor, decode_row, dialect_vendor, read_residue, record_refusal, record_timestamp, refusal,
    vendor_dialect,
};

const BACKWARD_CHUNK_BYTES: u64 = 64 * 1024;

pub(crate) fn recent_compaction_horizon(
    sources: &[ResolvedSource],
    count: usize,
    end: Option<&str>,
) -> Result<CompactionHorizon, Refusal> {
    let mut markers = Vec::new();
    for source in sources {
        markers.extend(read_recent_compaction_markers(source, count, end)?);
    }
    markers.sort_by(|left, right| (&left.ts, &left.source).cmp(&(&right.ts, &right.source)));
    let first = markers.len().saturating_sub(count);
    let selected = markers.into_iter().skip(first).collect::<Vec<_>>();
    Ok(CompactionHorizon {
        start: selected.first().map(|marker| marker.ts.clone()),
    })
}

pub(super) fn read_records(
    source: &ResolvedSource,
    start: Option<&str>,
) -> Result<(Vec<TypedRecord>, Vec<Residue>, Vendor), Refusal> {
    let mut reader = transcript_reader(source, start)?;
    let mut dialect = vendor_dialect(source.vendor);
    let mut rows = Vec::new();
    let mut residues = Vec::new();
    let mut buffer = Vec::new();
    let mut line = 0_u64;
    loop {
        buffer.clear();
        let read = match reader.read_until(b'\n', &mut buffer) {
            Ok(read) => read,
            Err(error) => {
                residues.push(read_residue(&source.path, line.saturating_add(1), &error));
                break;
            }
        };
        if read == 0 {
            break;
        }
        line = line.saturating_add(1);
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        let (observed, row) = decode_row(source, line, &buffer, dialect)?;
        dialect = Some(observed);
        match row {
            DecodedRow::Typed(row) if start.is_some_and(|start| record_timestamp(&row) < start) => {
            }
            DecodedRow::Typed(row) => rows.push(row),
            DecodedRow::Residue(residue) => residues.push(residue),
            DecodedRow::Ignored => {}
        }
    }
    let vendor = dialect.map_or(source.vendor, dialect_vendor);
    Ok((rows, residues, vendor))
}

pub(super) fn transcript_reader(
    source: &ResolvedSource,
    start: Option<&str>,
) -> Result<Box<dyn BufRead>, Refusal> {
    let file = open_transcript(source)?;
    if compressed(&source.path) {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        let mut file = file;
        if let Some(start) = start {
            let offset = start_offset_after_prior_timestamp(source, start)?;
            file.seek(SeekFrom::Start(offset)).map_err(|error| {
                refusal(
                    "session::transcript-unreadable",
                    Some(format!("{}: {error}", source.path.display())),
                    "restore read access to the transcript and retry",
                )
            })?;
        }
        Ok(Box::new(BufReader::new(file)))
    }
}

fn open_transcript(source: &ResolvedSource) -> Result<File, Refusal> {
    File::open(&source.path).map_err(|error| {
        refusal(
            "session::transcript-unreadable",
            Some(format!("{}: {error}", source.path.display())),
            "restore read access to the transcript and retry",
        )
    })
}

fn compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".gz")
}

fn read_recent_compaction_markers(
    source: &ResolvedSource,
    count: usize,
    end: Option<&str>,
) -> Result<Vec<Marker>, Refusal> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if compressed(&source.path) {
        return read_recent_compaction_markers_forward(source, count, end);
    }
    let source_name = source.path.display().to_string();
    let mut markers = Vec::new();
    reverse_lines(&source.path, |line, offset, _| {
        if let Some(marker) = marker_from_line(source, &source_name, line, offset)?
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
) -> Result<Vec<Marker>, Refusal> {
    let mut reader = transcript_reader(source, None)?;
    let mut dialect = vendor_dialect(source.vendor);
    let mut markers = Vec::new();
    let mut buffer = Vec::new();
    let mut line = 0_u64;
    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).map_err(|error| {
            refusal(
                "session::transcript-unreadable",
                Some(format!("{}: {error}", source.path.display())),
                "restore read access to the transcript and retry",
            )
        })?;
        if read == 0 {
            break;
        }
        line = line.saturating_add(1);
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        let (observed, row) = decode_row(source, line, &buffer, dialect)?;
        dialect = Some(observed);
        if let DecodedRow::Typed(row) = row
            && let Some(marker) = marker_from_record(&row)
            && end.is_none_or(|end| marker.ts.as_str() <= end)
        {
            markers.push(marker);
            trim_oldest(&mut markers, count);
        }
    }
    Ok(markers)
}

fn start_offset_after_prior_timestamp(
    source: &ResolvedSource,
    start: &str,
) -> Result<u64, Refusal> {
    let mut offset = 0_u64;
    reverse_lines(&source.path, |line, _, next| {
        if let Some(ts) = timestamp_from_line(source, line)
            && ts.as_str() < start
        {
            offset = next;
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(offset)
}

fn reverse_lines<F>(path: &Path, mut visit: F) -> Result<(), Refusal>
where
    F: FnMut(&[u8], u64, u64) -> Result<bool, Refusal>,
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
    let mut carry = Vec::new();
    while position > 0 {
        let read = position.min(BACKWARD_CHUNK_BYTES);
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
        for index in parse_start..block.len() {
            if block[index] == b'\n' {
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
            if !visit(
                &block[start..end],
                position.saturating_add(start as u64),
                position.saturating_add(next as u64),
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
    let detected = listen::decode_record(vendor_dialect(source.vendor), line, locus)
        .map_err(|error| record_refusal(&source.path, 1, &error))?;
    let Decoded::Events { context, events } = detected.decoded else {
        return Ok(None);
    };
    let Some(raw_ts) = context.timestamp.as_deref() else {
        return Ok(None);
    };
    let Some(timestamp) = spice::session_meter::normalize_timestamp(raw_ts) else {
        return Ok(None);
    };
    Ok(events.into_iter().find_map(|event| {
        if let Event::Compaction(compaction) = event {
            Some(Marker {
                source: source_name.to_owned(),
                ts: timestamp.to_string(),
                summary: compaction.summary.unwrap_or_default(),
            })
        } else {
            None
        }
    }))
}

fn timestamp_from_line(source: &ResolvedSource, line: &[u8]) -> Option<String> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    let locus = listen::Locus::new(1, 0, line.len() as u64);
    let detected = listen::decode_record(vendor_dialect(source.vendor), line, locus).ok()?;
    let Decoded::Events { context, .. } = detected.decoded else {
        return None;
    };
    context
        .timestamp
        .as_deref()
        .and_then(spice::session_meter::normalize_timestamp)
        .map(|timestamp| timestamp.to_string())
}

fn trim_oldest<T>(rows: &mut Vec<T>, count: usize) {
    if rows.len() > count {
        rows.drain(..rows.len() - count);
    }
}
