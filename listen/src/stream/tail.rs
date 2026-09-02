//! Bounded newest-record windows over a generation-authenticated tape.

use std::path::{Path, PathBuf};

use tape::{Extent, Locus, Tape};

use super::{Checkpoint, Position, Scan, ScanError, scan, scan_extents, validate_position};
use crate::Dialect;

/// Bytes inspected at once while locating the preceding complete-record boundary.
const BOUNDARY_CHUNK_BYTES: u64 = 64 * 1024;

/// One newest-record scan and the complete-record boundary where it began.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TailScan {
    /// Safe boundary selected at or before the requested byte window.
    pub start: Checkpoint,
    /// Complete records and resumable end checkpoint after that boundary.
    pub scan: Scan,
}

/// Scan the newest bounded byte window without beginning inside a record.
///
/// The returned window may exceed `bytes` by one record: its start retreats to
/// the nearest preceding newline so the normal scanner never interprets a
/// record suffix as independent vendor evidence. The end checkpoint remains
/// before an unterminated tail, so a later forward scan cannot miss the suffix
/// that completes it.
///
/// # Errors
///
/// Returns [`ScanError`] when tape identity, extents, boundary bytes, or the
/// resulting generation-authenticated scan cannot be read.
pub fn scan_tail(
    root: impl AsRef<Path>,
    dialect: Dialect,
    bytes: u64,
) -> Result<TailScan, ScanError> {
    let root = root.as_ref();
    let tape = Tape::open_existing(root).map_err(|source| tape_error(root, source))?;
    let source = tape
        .source_id()
        .map_err(|source| tape_error(root, source))?;
    let extents = tape.extents().map_err(|source| tape_error(root, source))?;
    let candidate = tail_candidate(&extents, bytes.max(1));
    let next = preceding_boundary(&tape, &extents, candidate)
        .map_err(|source| tape_error(root, source))?;
    let mut start = Checkpoint { source, next };
    let scan = scan(root, dialect, Some(&start))?;
    if start.next == Position::start() {
        // Byte zero is transferable to a concurrently replaced generation;
        // report the exact source the resulting scan actually admitted.
        start.source = scan.checkpoint.source.clone();
    }
    Ok(TailScan { start, scan })
}

/// Scan a bounded complete-record window immediately before an authenticated boundary.
///
/// Unlike [`scan_tail`], this does not read records after `before`, so adjacent
/// history pages can move backward without rescanning a page already served.
///
/// # Errors
///
/// Returns [`ScanError`] when the source identity or boundary no longer belongs
/// to the observed tape generation, or when its bounded records cannot be read.
pub fn scan_before(
    root: impl AsRef<Path>,
    dialect: Dialect,
    before: &Checkpoint,
    bytes: u64,
) -> Result<TailScan, ScanError> {
    let root = root.as_ref();
    let tape = Tape::open_existing(root).map_err(|source| tape_error(root, source))?;
    let source = tape
        .source_id()
        .map_err(|source| tape_error(root, source))?;
    if before.source != source {
        return Err(ScanError::SourceChanged {
            expected: before.source.clone(),
            observed: source,
            next: before.next,
        });
    }
    let extents = tape.extents().map_err(|source| tape_error(root, source))?;
    let end = validate_position(before.next, &extents)?;
    let candidate = window_candidate(&extents, end, bytes.max(1));
    let next = preceding_boundary(&tape, &extents, candidate)
        .map_err(|source| tape_error(root, source))?;
    let start = Checkpoint {
        source: before.source.clone(),
        next,
    };
    let state = scan_extents(&tape, dialect, &before.source, &extents, next, Some(end))?;
    let observed = tape
        .source_id()
        .map_err(|source| tape_error(root, source))?;
    if observed != before.source {
        return Err(ScanError::SourceChanged {
            expected: before.source.clone(),
            observed,
            next: state.position,
        });
    }
    Ok(TailScan {
        start,
        scan: Scan {
            records: state.records,
            checkpoint: Checkpoint {
                source: before.source.clone(),
                next: state.position,
            },
            pending: state.spans,
            extents_empty: extents.is_empty(),
        },
    })
}

fn tail_candidate(extents: &[Extent], bytes: u64) -> Position {
    let mut remaining = bytes;
    for extent in extents.iter().rev() {
        if remaining >= extent.length {
            remaining -= extent.length;
            continue;
        }
        return Position {
            member: extent.member,
            offset: extent.length - remaining,
        };
    }
    Position::start()
}

fn window_candidate(extents: &[Extent], end: Position, bytes: u64) -> Position {
    let mut remaining = bytes;
    for extent in extents.iter().rev() {
        if extent.member > end.member {
            continue;
        }
        let length = if extent.member == end.member {
            end.offset.min(extent.length)
        } else {
            extent.length
        };
        if length == 0 {
            continue;
        }
        if remaining < length {
            return Position {
                member: extent.member,
                offset: length - remaining,
            };
        }
        remaining -= length;
    }
    Position::start()
}

fn preceding_boundary(
    tape: &Tape,
    extents: &[Extent],
    candidate: Position,
) -> Result<Position, tape::Error> {
    if candidate == Position::start() {
        return Ok(candidate);
    }
    let candidate_index = extents
        .iter()
        .position(|extent| extent.member == candidate.member)
        .expect("a tail candidate names one observed extent");
    for index in (0..=candidate_index).rev() {
        let extent = extents[index];
        let mut end = if index == candidate_index {
            candidate.offset
        } else {
            extent.length
        };
        while end > 0 {
            let start = end.saturating_sub(BOUNDARY_CHUNK_BYTES);
            let fragment = tape.read(Locus::new(extent.member, start, end - start))?;
            if let Some(newline) = fragment.iter().rposition(|byte| *byte == b'\n') {
                let offset = start + newline as u64 + 1;
                return Ok(if extent.sealed && offset == extent.length {
                    Position {
                        member: extent.member + 1,
                        offset: 0,
                    }
                } else {
                    Position {
                        member: extent.member,
                        offset,
                    }
                });
            }
            end = start;
        }
    }
    Ok(Position::start())
}

fn tape_error(path: &Path, source: tape::Error) -> ScanError {
    ScanError::Tape {
        path: PathBuf::from(path),
        source,
    }
}
