//! Bounded forward decoding into one canonical session fold.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use listen::{Decoded, RecordDecodeError, ResidueKind};
use wire::Refusal;

use super::horizon::start_offset_after_prior_timestamp;
use super::work::{SessionWork, decoded_row_events};
use super::{
    DecodedRow, MAX_LINE_BYTES, Residue, ResolvedSource, Vendor, decode_row, read_residue,
    record_refusal, record_timestamp, refusal,
};

enum RecordRead {
    Eof,
    Read { bytes: usize, terminated: bool },
    Interrupted(std::io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ForwardContinuation {
    pub(crate) line: u64,
    pub(super) dialect: Option<listen::Dialect>,
    pub(super) live_tail: bool,
}

pub(crate) enum ForwardProgress {
    More(ForwardContinuation),
    Eof(ForwardContinuation),
}

pub(crate) struct ForwardWindow {
    pub(crate) row: Option<DecodedRow>,
    pub(crate) progress: ForwardProgress,
}

pub(crate) struct ForwardCursor<'a> {
    source: &'a ResolvedSource,
    reader: Box<dyn BufRead>,
    start: Option<String>,
    dialect: Option<listen::Dialect>,
    line: u64,
    within_horizon: bool,
    live_tail: bool,
    buffer: Vec<u8>,
}

pub(super) fn stream_records(
    source: &ResolvedSource,
    start: Option<&str>,
    work: &mut SessionWork,
    mut accept: impl FnMut(DecodedRow, &mut SessionWork, &str) -> Result<(), Refusal>,
) -> Result<Vendor, Refusal> {
    let mut cursor = ForwardCursor::new(source, start, work)?;
    loop {
        let window = cursor.advance(work)?;
        if let Some(row) = window.row {
            let at = cursor.record_locus();
            accept(row, work, &at)?;
        }
        match window.progress {
            ForwardProgress::More(next) => debug_assert!(cursor.is_at(next)),
            ForwardProgress::Eof(end) => {
                debug_assert!(cursor.is_at(end));
                break;
            }
        }
    }
    Ok(cursor.vendor())
}

impl<'a> ForwardCursor<'a> {
    pub(crate) fn new(
        source: &'a ResolvedSource,
        start: Option<&str>,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let reader = transcript_reader(source, start, work)?;
        work.begin_pass(&source.path.display().to_string())?;
        Ok(Self {
            source,
            reader,
            start: start.map(str::to_owned),
            dialect: source.dialect,
            line: 0,
            within_horizon: start.is_none(),
            live_tail: false,
            buffer: Vec::new(),
        })
    }

    pub(crate) fn at_offset(
        source: &'a ResolvedSource,
        offset: u64,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let mut file = open_transcript(source)?;
        file.seek(SeekFrom::Start(offset)).map_err(|error| {
            refusal(
                "session::transcript-unreadable",
                Some(format!("{}: {error}", source.path.display())),
                "restore read access to the transcript and retry",
            )
        })?;
        work.begin_pass(&source.path.display().to_string())?;
        Ok(Self {
            source,
            reader: Box::new(BufReader::new(file)),
            start: None,
            dialect: source.dialect,
            line: 0,
            within_horizon: true,
            live_tail: false,
            buffer: Vec::new(),
        })
    }

    pub(crate) fn advance(&mut self, work: &mut SessionWork) -> Result<ForwardWindow, Refusal> {
        self.buffer.clear();
        let at = self.next_record_locus();
        let (read, terminated) =
            match read_record(self.reader.as_mut(), &mut self.buffer, work, &at)? {
                RecordRead::Eof => return Ok(self.window(None, false)),
                RecordRead::Interrupted(error) => {
                    self.line = self.line.saturating_add(1);
                    let row =
                        DecodedRow::Residue(read_residue(&self.source.path, self.line, &error));
                    work.admit_decoded(decoded_row_events(&row), &at)?;
                    return Ok(self.window(Some(row), false));
                }
                RecordRead::Read { bytes, terminated } => (bytes, terminated),
            };
        self.line = self.line.saturating_add(1);
        self.live_tail = !terminated && self.source.thread.is_some();
        debug_assert!(read > 0 && read <= MAX_LINE_BYTES.saturating_add(2));
        let (observed, row) = decode_window_row(
            self.source,
            self.line,
            &self.buffer,
            self.dialect,
            self.live_tail,
            work,
            &at,
        )?;
        self.dialect = observed.or(self.dialect);
        let row = match row {
            None => None,
            Some(DecodedRow::Typed(record)) => {
                if !self.within_horizon
                    && self
                        .start
                        .as_deref()
                        .is_some_and(|start| record_timestamp(&record) >= start)
                {
                    self.within_horizon = true;
                }
                self.within_horizon.then_some(DecodedRow::Typed(record))
            }
            Some(row) => Some(row),
        };
        Ok(self.window(row, terminated))
    }

    fn window(&self, row: Option<DecodedRow>, more: bool) -> ForwardWindow {
        let continuation = self.continuation();
        let progress = if more {
            ForwardProgress::More(continuation)
        } else {
            ForwardProgress::Eof(continuation)
        };
        ForwardWindow { row, progress }
    }

    fn is_at(&self, continuation: ForwardContinuation) -> bool {
        self.line == continuation.line
            && self.dialect == continuation.dialect
            && self.live_tail == continuation.live_tail
    }

    pub(crate) fn continuation(&self) -> ForwardContinuation {
        ForwardContinuation {
            line: self.line,
            dialect: self.dialect,
            live_tail: self.live_tail,
        }
    }

    fn next_record_locus(&self) -> String {
        format!(
            "{}:record {}",
            self.source.path.display(),
            self.line.saturating_add(1)
        )
    }

    fn record_locus(&self) -> String {
        format!("{}:record {}", self.source.path.display(), self.line)
    }

    pub(crate) fn vendor(&self) -> Vendor {
        if self.source.vendor.is_unknown() {
            self.dialect.map_or(self.source.vendor, Vendor::inferred)
        } else {
            self.source.vendor
        }
    }
}

pub(super) fn decode_window_row(
    source: &ResolvedSource,
    line: u64,
    buffer: &[u8],
    dialect: Option<listen::Dialect>,
    live_tail: bool,
    work: &mut SessionWork,
    at: &str,
) -> Result<(Option<listen::Dialect>, Option<DecodedRow>), Refusal> {
    if live_tail
        && let Some((observed, residue)) =
            live_tail_truncated_residue(source, line, buffer, dialect)?
    {
        let row = DecodedRow::Residue(residue);
        work.admit_decoded(decoded_row_events(&row), at)?;
        return Ok((Some(observed), Some(row)));
    }
    let end = buffer
        .iter()
        .rposition(|byte| !matches!(byte, b'\n' | b'\r'))
        .map_or(0, |last| last + 1);
    if end == 0 {
        return Ok((dialect, None));
    }
    let (observed, row) = decode_row(source, line, &buffer[..end], dialect)?;
    work.admit_decoded(decoded_row_events(&row), at)?;
    Ok((observed, Some(row)))
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

pub(super) fn live_tail_truncated_residue(
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

pub(crate) fn compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".gz")
}
