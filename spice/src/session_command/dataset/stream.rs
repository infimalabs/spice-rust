//! Bounded forward decoding into one canonical session fold.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::GzDecoder;
use listen::{Decoded, RecordDecodeError, ResidueKind};
use wire::Refusal;

use super::aggregate::{admit_sources, merge_compactions, source_view};
use super::horizon::{
    last_assistant_from_record, start_offset_after_prior_assistant,
    start_offset_after_prior_timestamp,
};
use super::work::{ResidueLedger, SessionWork, SessionWorkBudget, decoded_row_events};
use super::{
    CompactionHorizon, CompactionTracker, DecodedRow, FrontierDirection, MAX_LINE_BYTES, Residue,
    ResolvedSource, Vendor, decode_row, read_residue, record_refusal, record_timestamp, refusal,
};
use crate::session_command::model::{CompactionRecord, SourceView};

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

pub(crate) struct CompactionScan {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) compactions: Vec<CompactionRecord>,
    pub(crate) residues: Vec<Residue>,
}

impl CompactionScan {
    pub(crate) fn scan(
        sources: &[ResolvedSource],
        horizon: CompactionHorizon,
        limit: usize,
    ) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        debug_assert_eq!(sources.len(), horizon.sources.len());
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut residues = ResidueLedger::new();
        let mut source_views = Vec::with_capacity(sources.len());
        let mut source_rows = Vec::with_capacity(sources.len());
        for (source, boundary) in sources.iter().zip(horizon.sources) {
            let mut vendor = boundary.vendor;
            let rows = if boundary.has_compaction {
                let mut cursor = if compressed(&source.path) {
                    ForwardCursor::new(source, None, &mut work)?
                } else {
                    ForwardCursor::at_offset(
                        source,
                        boundary
                            .start_offset
                            .expect("uncompressed compaction horizon has an exact offset"),
                        &mut work,
                    )?
                };
                let mut tracker = CompactionTracker::new(
                    limit,
                    FrontierDirection::Newest,
                    horizon.start.as_deref(),
                    None,
                )?;
                tracker.seed_assistant(boundary.assistant_before);
                scan_compaction_rows(source, &mut cursor, &mut tracker, &mut residues, &mut work)?;
                vendor = cursor.vendor();
                tracker.finish(&mut work, &source.path.display().to_string())?
            } else {
                Vec::new()
            };
            let view = source_view(source, vendor);
            work.admit_retained_text(super::source_view_text_bytes(&view), &view.path)?;
            source_views.push(view);
            source_rows.push(VecDeque::from(rows));
        }
        Ok(Self {
            sources: source_views,
            compactions: merge_compactions(source_rows, limit, FrontierDirection::Newest)?,
            residues: residues.finish(&mut work, "compaction residue summary")?,
        })
    }
}

fn scan_compaction_rows(
    source: &ResolvedSource,
    cursor: &mut ForwardCursor<'_>,
    tracker: &mut CompactionTracker,
    residues: &mut ResidueLedger,
    work: &mut SessionWork,
) -> Result<(), Refusal> {
    loop {
        let window = cursor.advance(work)?;
        if let Some(row) = window.row {
            let at = format!(
                "{}:record {}",
                source.path.display(),
                cursor.continuation().line
            );
            match row {
                DecodedRow::Typed(record) => tracker.observe_record(&record, work, &at)?,
                DecodedRow::Residue(residue) => residues.push(residue, work, &at)?,
                DecodedRow::Ignored => {}
            }
        }
        if matches!(window.progress, ForwardProgress::Eof(_)) {
            return Ok(());
        }
    }
}

pub(crate) struct ForwardCursor<'a> {
    source: &'a ResolvedSource,
    reader: Box<dyn BufRead>,
    start: Option<String>,
    dialect: Option<listen::Dialect>,
    line: u64,
    within_horizon: bool,
    retain_preceding_assistant: bool,
    preceding_assistant: Option<String>,
    preceding_assistant_ready: bool,
    generation: Option<TranscriptGeneration>,
    expected_bytes: Option<u64>,
    observed_bytes: u64,
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
        Self::from_start(source, start, false, work)
    }

    pub(crate) fn with_preceding_assistant(
        source: &'a ResolvedSource,
        start: Option<&str>,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        Self::from_start(source, start, true, work)
    }

    fn from_start(
        source: &'a ResolvedSource,
        start: Option<&str>,
        retain_preceding_assistant: bool,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let file = open_transcript(source)?;
        Self::from_file(source, file, start, retain_preceding_assistant, work)
    }

    fn from_file(
        source: &'a ResolvedSource,
        file: File,
        start: Option<&str>,
        retain_preceding_assistant: bool,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        let captured = transcript_reader(source, file, start, retain_preceding_assistant, work)?;
        work.begin_pass(&source.path.display().to_string())?;
        Ok(Self {
            source,
            reader: captured.reader,
            start: start.map(str::to_owned),
            dialect: source.dialect,
            line: 0,
            within_horizon: start.is_none(),
            retain_preceding_assistant,
            preceding_assistant: captured.preceding_assistant,
            preceding_assistant_ready: false,
            generation: captured.generation,
            expected_bytes: captured.expected_bytes,
            observed_bytes: 0,
            live_tail: false,
            buffer: Vec::new(),
        })
    }

    #[cfg(test)]
    pub(super) fn with_preceding_assistant_from_file(
        source: &'a ResolvedSource,
        file: File,
        start: &str,
        work: &mut SessionWork,
    ) -> Result<Self, Refusal> {
        Self::from_file(source, file, Some(start), true, work)
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
            retain_preceding_assistant: false,
            preceding_assistant: None,
            preceding_assistant_ready: false,
            generation: None,
            expected_bytes: None,
            observed_bytes: 0,
            live_tail: false,
            buffer: Vec::new(),
        })
    }

    pub(crate) fn advance(&mut self, work: &mut SessionWork) -> Result<ForwardWindow, Refusal> {
        self.buffer.clear();
        let at = self.next_record_locus();
        let (read, terminated) =
            match read_record(self.reader.as_mut(), &mut self.buffer, work, &at)? {
                RecordRead::Eof => {
                    self.require_complete_generation()?;
                    return Ok(self.window(None, false));
                }
                RecordRead::Interrupted(error) => {
                    if self.generation_changed()? {
                        return Err(generation_changed(self.source, &error.to_string()));
                    }
                    self.line = self.line.saturating_add(1);
                    let row =
                        DecodedRow::Residue(read_residue(&self.source.path, self.line, &error));
                    work.admit_decoded(decoded_row_events(&row), &at)?;
                    return Ok(self.window(Some(row), false));
                }
                RecordRead::Read { bytes, terminated } => (bytes, terminated),
            };
        self.observed_bytes = self.observed_bytes.saturating_add(read as u64);
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
                if !self.within_horizon {
                    if self
                        .start
                        .as_deref()
                        .is_some_and(|start| record_timestamp(&record) >= start)
                    {
                        self.within_horizon = true;
                        if self.retain_preceding_assistant {
                            if let Some(assistant) = self.preceding_assistant.as_deref() {
                                work.admit_retained_text(super::text_bytes([assistant]), &at)?;
                            }
                            self.preceding_assistant_ready = true;
                        }
                    } else if self.retain_preceding_assistant
                        && let Some(assistant) = last_assistant_from_record(&record)
                    {
                        self.preceding_assistant = Some(assistant);
                    }
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

    pub(crate) fn take_preceding_assistant(&mut self) -> Option<String> {
        if !self.preceding_assistant_ready {
            return None;
        }
        self.preceding_assistant_ready = false;
        self.preceding_assistant.take()
    }

    fn require_complete_generation(&self) -> Result<(), Refusal> {
        if self
            .expected_bytes
            .is_some_and(|expected| self.observed_bytes != expected)
        {
            Err(generation_changed(
                self.source,
                "captured transcript generation ended before its admitted length",
            ))
        } else {
            Ok(())
        }
    }

    fn generation_changed(&self) -> Result<bool, Refusal> {
        let Some(generation) = &self.generation else {
            return Ok(false);
        };
        generation
            .file
            .metadata()
            .map(|metadata| metadata.len() != generation.bytes)
            .map_err(|error| source_unreadable(self.source, &error))
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

struct TranscriptReader {
    reader: Box<dyn BufRead>,
    preceding_assistant: Option<String>,
    generation: Option<TranscriptGeneration>,
    expected_bytes: Option<u64>,
}

struct TranscriptGeneration {
    file: File,
    bytes: u64,
}

fn transcript_reader(
    source: &ResolvedSource,
    mut file: File,
    start: Option<&str>,
    retain_preceding_assistant: bool,
    work: &mut SessionWork,
) -> Result<TranscriptReader, Refusal> {
    let exact_generation = retain_preceding_assistant && start.is_some();
    let transcript_length = file
        .metadata()
        .map_err(|error| source_unreadable(source, &error))?
        .len();
    let generation = exact_generation
        .then(|| {
            file.try_clone()
                .map(|file| TranscriptGeneration {
                    file,
                    bytes: transcript_length,
                })
                .map_err(|error| source_unreadable(source, &error))
        })
        .transpose()?;
    if compressed(&source.path) {
        let reader: Box<dyn BufRead> = if exact_generation {
            Box::new(BufReader::new(GzDecoder::new(file.take(transcript_length))))
        } else {
            Box::new(BufReader::new(GzDecoder::new(file)))
        };
        Ok(TranscriptReader {
            reader,
            preceding_assistant: None,
            generation,
            expected_bytes: None,
        })
    } else {
        let mut preceding_assistant = None;
        let mut offset = 0;
        if let Some(start) = start {
            work.begin_pass(&source.path.display().to_string())?;
            let locator = file
                .try_clone()
                .map_err(|error| source_unreadable(source, &error))?;
            offset = if retain_preceding_assistant {
                let (offset, assistant) = start_offset_after_prior_assistant(
                    source,
                    locator,
                    transcript_length,
                    start,
                    work,
                )?;
                preceding_assistant = assistant;
                offset
            } else {
                start_offset_after_prior_timestamp(source, locator, transcript_length, start, work)?
            };
            file.seek(SeekFrom::Start(offset)).map_err(|error| {
                refusal(
                    "session::transcript-unreadable",
                    Some(format!("{}: {error}", source.path.display())),
                    "restore read access to the transcript and retry",
                )
            })?;
        }
        let expected_bytes = exact_generation.then_some(transcript_length.saturating_sub(offset));
        let reader: Box<dyn BufRead> = if let Some(expected) = expected_bytes {
            Box::new(BufReader::new(file.take(expected)))
        } else {
            Box::new(BufReader::new(file))
        };
        Ok(TranscriptReader {
            reader,
            preceding_assistant,
            generation,
            expected_bytes,
        })
    }
}

fn open_transcript(source: &ResolvedSource) -> Result<File, Refusal> {
    File::open(&source.path).map_err(|error| source_unreadable(source, &error))
}

fn source_unreadable(source: &ResolvedSource, error: &std::io::Error) -> Refusal {
    refusal(
        "session::transcript-unreadable",
        Some(format!("{}: {error}", source.path.display())),
        "restore read access to the transcript and retry",
    )
}

fn generation_changed(source: &ResolvedSource, detail: &str) -> Refusal {
    refusal(
        "session::transcript-generation-changed",
        Some(format!("{}: {detail}", source.path.display())),
        "retry against one stable transcript generation",
    )
}

pub(crate) fn compressed(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".gz")
}
