//! Bounded forward decoding into one canonical session fold.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use listen::{Decoded, RecordDecodeError, ResidueKind};
use wire::Refusal;

use super::horizon::start_offset_after_prior_timestamp;
use super::work::{SessionWork, decoded_row_work};
use super::{
    DecodedRow, MAX_LINE_BYTES, Residue, ResolvedSource, Vendor, decode_row, read_residue,
    record_refusal, record_timestamp, refusal,
};

enum RecordRead {
    Eof,
    Read { bytes: usize, terminated: bool },
    Interrupted(std::io::Error),
}

pub(super) fn stream_records(
    source: &ResolvedSource,
    start: Option<&str>,
    work: &mut SessionWork,
    mut accept: impl FnMut(DecodedRow) -> Result<(), Refusal>,
) -> Result<Vendor, Refusal> {
    let mut reader = transcript_reader(source, start, work)?;
    work.begin_pass(&source.path.display().to_string())?;
    let mut dialect = source.dialect;
    let mut buffer = Vec::new();
    let mut line = 0_u64;
    let mut within_horizon = start.is_none();
    loop {
        buffer.clear();
        let at = format!(
            "{}:record {}",
            source.path.display(),
            line.saturating_add(1)
        );
        let read = match read_record(reader.as_mut(), &mut buffer, work, &at)? {
            RecordRead::Read { bytes, terminated } => {
                if !terminated
                    && let Some((observed, residue)) = live_tail_truncated_residue(
                        source,
                        line.saturating_add(1),
                        &buffer,
                        dialect,
                    )?
                {
                    dialect = Some(observed);
                    let row = DecodedRow::Residue(residue);
                    let (events, retained) = decoded_row_work(&row);
                    work.admit_decoded(events, retained, &at)?;
                    accept(row)?;
                    line = line.saturating_add(1);
                    continue;
                }
                bytes
            }
            RecordRead::Interrupted(error) => {
                let row =
                    DecodedRow::Residue(read_residue(&source.path, line.saturating_add(1), &error));
                let (events, retained) = decoded_row_work(&row);
                work.admit_decoded(events, retained, &at)?;
                accept(row)?;
                break;
            }
            RecordRead::Eof => break,
        };
        line = line.saturating_add(1);
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        debug_assert!(read > 0);
        let (observed, row) = decode_row(source, line, &buffer, dialect)?;
        dialect = observed.or(dialect);
        let (events, retained) = decoded_row_work(&row);
        work.admit_decoded(events, retained, &at)?;
        match row {
            DecodedRow::Typed(record) => {
                if !within_horizon && start.is_some_and(|start| record_timestamp(&record) >= start)
                {
                    within_horizon = true;
                }
                if within_horizon {
                    accept(DecodedRow::Typed(record))?;
                }
            }
            DecodedRow::Residue(residue) => accept(DecodedRow::Residue(residue))?,
            DecodedRow::Ignored => accept(DecodedRow::Ignored)?,
        }
    }
    Ok(if source.vendor.is_unknown() {
        dialect.map_or(source.vendor, Vendor::inferred)
    } else {
        source.vendor
    })
}

fn read_record(
    reader: &mut dyn BufRead,
    buffer: &mut Vec<u8>,
    work: &mut SessionWork,
    at: &str,
) -> Result<RecordRead, Refusal> {
    loop {
        work.check_deadline(at)?;
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) => return Ok(RecordRead::Interrupted(error)),
        };
        if available.is_empty() {
            return if buffer.is_empty() {
                Ok(RecordRead::Eof)
            } else {
                work.admit_record(at)?;
                Ok(RecordRead::Read {
                    bytes: buffer.len(),
                    terminated: false,
                })
            };
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |newline| newline + 1);
        if buffer.is_empty() {
            work.ensure_record(at)?;
        }
        work.admit_source_bytes(count as u64, at)?;
        if buffer.len().saturating_add(count) > MAX_LINE_BYTES.saturating_add(2) {
            return Err(refusal(
                "session::transcript-record-oversized",
                Some(format!("{at} exceeds {MAX_LINE_BYTES} bytes")),
                "restore a bounded transcript record or select an earlier intact copy",
            ));
        }
        buffer.extend_from_slice(&available[..count]);
        let terminated = available[count - 1] == b'\n';
        reader.consume(count);
        if terminated {
            work.admit_record(at)?;
            return Ok(RecordRead::Read {
                bytes: buffer.len(),
                terminated,
            });
        }
    }
}

fn live_tail_truncated_residue(
    source: &ResolvedSource,
    line: u64,
    buffer: &[u8],
    dialect: Option<listen::Dialect>,
) -> Result<Option<(listen::Dialect, Residue)>, Refusal> {
    if source.thread.is_none() || compressed(&source.path) {
        return Ok(None);
    }
    let Some(expected) = dialect else {
        return Ok(None);
    };
    let locus = listen::Locus::new(line, 0, buffer.len() as u64);
    let truncated = match listen::decode_record(Some(expected), buffer, locus) {
        Ok(detected) => match detected.decoded {
            Decoded::Residue(residue) => matches!(residue.kind, ResidueKind::Truncated),
            Decoded::Events { .. } => false,
        },
        Err(RecordDecodeError::Invalid {
            kind: ResidueKind::Truncated,
        }) => true,
        Err(error) => return Err(record_refusal(&source.path, line, &error)),
    };
    Ok(truncated.then(|| {
        (
            expected,
            Residue {
                source: source.path.display().to_string(),
                line,
                kind: "live-tail-truncated".to_owned(),
                detail: "final live transcript row ended before a complete JSON record".to_owned(),
                recovery: "retry after the vendor finishes appending the current transcript record"
                    .to_owned(),
            },
        )
    }))
}

fn transcript_reader(
    source: &ResolvedSource,
    start: Option<&str>,
    work: &mut SessionWork,
) -> Result<Box<dyn BufRead>, Refusal> {
    let file = open_transcript(source)?;
    if compressed(&source.path) {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        let mut file = file;
        if let Some(start) = start {
            work.begin_pass(&source.path.display().to_string())?;
            let offset = start_offset_after_prior_timestamp(source, start, work)?;
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

pub(super) fn compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".gz")
}
