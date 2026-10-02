//! Content authentication for preserved-session history checkpoints.
//!
//! A checkpoint freezes the transcript prefix present when it was captured.
//! A [`FileStamp`] cannot tell a genuine same-inode append from
//! rewrite-plus-append, so history never decodes a prefix byte on metadata
//! alone. The prefix is divided into fixed chunks: the first read of a chunk
//! records its content digest, and every later read must reproduce it, so a
//! rewrite of bytes history already read is refused before anything decodes
//! them. Appends land past the frozen prefix and touch no chunk, so an actively
//! written transcript pages with work bounded by the window it reads rather
//! than by the length of the prefix behind it.

#[cfg(test)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;

/// Prefix bytes one digest covers. A window authenticates the whole chunks it
/// overlaps, so it reads at most one partial chunk beyond each of its edges.
pub(super) const HISTORY_CHUNK_BYTES: u64 = 16 * 1024;

#[derive(Clone)]
pub(in crate::resident::message_surface) struct HistoryCheckpoint {
    frontier: Option<u64>,
    /// Frozen prefix length: history reads only `[0, length)`.
    length: u64,
    authentication: Arc<Mutex<HistoryAuthentication>>,
}

struct HistoryAuthentication {
    /// Source identity captured with the checkpoint.
    stamp: FileStamp,
    /// Digest of every chunk read so far, by chunk index.
    chunks: HashMap<u64, blake3::Hash>,
    /// A refused source stays refused for the checkpoint's generation.
    fault: Option<String>,
}

impl std::fmt::Debug for HistoryCheckpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HistoryCheckpoint")
            .field("next", &self.next())
            .finish_non_exhaustive()
    }
}

impl HistoryCheckpoint {
    pub(in crate::resident::message_surface) fn next(&self) -> u64 {
        self.frontier
            .map_or(self.length, |frontier| frontier.min(self.length))
    }

    pub(in crate::resident::message_surface) fn retreat(&self, next: u64) -> Self {
        Self {
            frontier: Some(self.frontier.map_or(next, |frontier| frontier.min(next))),
            length: self.length,
            authentication: Arc::clone(&self.authentication),
        }
    }
}

pub(in crate::resident::message_surface) fn history_checkpoint(
    path: &PathBuf,
) -> io::Result<HistoryCheckpoint> {
    let stamp = FileStamp::read(&File::open(path)?)?;
    Ok(HistoryCheckpoint {
        frontier: None,
        length: stamp.length,
        authentication: Arc::new(Mutex::new(HistoryAuthentication {
            stamp,
            chunks: HashMap::new(),
            fault: None,
        })),
    })
}

/// Read `[start, end)` of the frozen prefix, proven against its chunk digests.
///
/// Returns the source stamp observed after the read with exactly the
/// authenticated bytes the caller may decode.
pub(super) fn read_authenticated_history(
    path: &PathBuf,
    checkpoint: &HistoryCheckpoint,
    start: u64,
    end: u64,
) -> io::Result<(FileStamp, Vec<u8>)> {
    debug_assert!(start <= end && end <= checkpoint.length);
    let mut authentication = checkpoint.authentication.lock().map_err(|_| {
        io::Error::other("preserved session history authentication lock is poisoned")
    })?;
    if let Some(fault) = &authentication.fault {
        return Err(io::Error::new(io::ErrorKind::InvalidData, fault.clone()));
    }
    authentication.read(path, checkpoint.length, start, end)
}

/// Admit the source as the one the checkpoint froze, possibly grown, without
/// reading a byte: cards decoded earlier stay servable only while it is.
pub(super) fn admit_history_source(
    path: &PathBuf,
    checkpoint: &HistoryCheckpoint,
) -> io::Result<()> {
    let mut authentication = checkpoint.authentication.lock().map_err(|_| {
        io::Error::other("preserved session history authentication lock is poisoned")
    })?;
    if let Some(fault) = &authentication.fault {
        return Err(io::Error::new(io::ErrorKind::InvalidData, fault.clone()));
    }
    let observed = FileStamp::read(&File::open(path)?)?;
    authentication.admit(observed, checkpoint.length).map(drop)
}

impl HistoryAuthentication {
    fn read(
        &mut self,
        path: &PathBuf,
        length: u64,
        start: u64,
        end: u64,
    ) -> io::Result<(FileStamp, Vec<u8>)> {
        let first = start / HISTORY_CHUNK_BYTES;
        let covered_start = first * HISTORY_CHUNK_BYTES;
        let covered_end = end
            .div_ceil(HISTORY_CHUNK_BYTES)
            .saturating_mul(HISTORY_CHUNK_BYTES)
            .min(length);
        let mut file = File::open(path)?;
        let before = self.admit(FileStamp::read(&file)?, length)?;
        let mut bytes = read_range(&mut file, covered_start, covered_end)?;
        #[cfg(test)]
        AFTER_HISTORY_AUTHENTICATION_READ.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let after = self.admit(FileStamp::read(&file)?, length)?;
        let digests = chunk_digests(&bytes, first);
        if digests
            .iter()
            .any(|(index, digest)| self.chunks.get(index).is_some_and(|known| known != digest))
        {
            return Err(self.refuse("after its authenticated prefix was rewritten"));
        }
        // A write landed while unrecorded chunks were read. Appends never touch
        // the frozen prefix, so a second read reproduces those bytes unless the
        // prefix itself was being rewritten underneath the first one.
        if after != before
            && digests
                .iter()
                .any(|(index, _)| !self.chunks.contains_key(index))
            && chunk_digests(&read_range(&mut file, covered_start, covered_end)?, first) != digests
        {
            return Err(self.refuse("while its prefix was being read"));
        }
        self.chunks.extend(digests);
        let offset = usize::try_from(start - covered_start).expect("chunk offset fits usize");
        let window = usize::try_from(end - start).expect("history window fits usize");
        bytes.truncate(offset + window);
        bytes.drain(..offset);
        Ok((after, bytes))
    }

    /// Admit `observed` only as the captured source, possibly grown.
    fn admit(&mut self, observed: FileStamp, length: u64) -> io::Result<FileStamp> {
        if self.stamp.extension(observed, length).is_some() {
            Ok(observed)
        } else {
            Err(self.refuse("after app bootstrap"))
        }
    }

    fn refuse(&mut self, when: &str) -> io::Error {
        let detail = format!("preserved session history source changed {when}");
        self.fault = Some(detail.clone());
        io::Error::new(io::ErrorKind::InvalidData, detail)
    }
}

fn read_range(file: &mut File, start: u64, end: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; usize::try_from(end - start).expect("history range fits usize")];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn chunk_digests(bytes: &[u8], first: u64) -> Vec<(u64, blake3::Hash)> {
    bytes
        .chunks(HISTORY_CHUNK_BYTES as usize)
        .zip(first..)
        .map(|(chunk, index)| (index, blake3::hash(chunk)))
        .collect()
}

#[cfg(test)]
thread_local! {
    pub(super) static AFTER_HISTORY_AUTHENTICATION_READ: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}
