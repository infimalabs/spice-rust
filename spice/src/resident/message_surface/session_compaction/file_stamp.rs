use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileStamp {
    pub(super) length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}
impl FileStamp {
    pub(super) fn read(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }

    pub(super) fn extension(&self, observed: Self, prefix_length: u64) -> Option<Self> {
        if observed.length < prefix_length {
            return None;
        }
        #[cfg(unix)]
        if observed.device != self.device || observed.inode != self.inode {
            return None;
        }
        if observed == *self {
            return Some(*self);
        }
        if observed.length <= self.length {
            return None;
        }
        Some(observed)
    }
}

pub(super) fn record_guard_hash(bytes: &[u8]) -> u64 {
    let witness = bytes.len().min(RECORD_GUARD_BYTES);
    let hash = hash_bytes(0xcbf2_9ce4_8422_2325, &bytes.len().to_le_bytes());
    let hash = hash_bytes(hash, &bytes[..witness]);
    if bytes.len() > witness {
        hash_bytes(hash, &bytes[bytes.len() - witness..])
    } else {
        hash
    }
}

pub(super) fn record_guard_hash_with_length(bytes: &[u8], length: usize) -> u64 {
    let hash = hash_bytes(0xcbf2_9ce4_8422_2325, &length.to_le_bytes());
    hash_bytes(hash, bytes)
}

fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
