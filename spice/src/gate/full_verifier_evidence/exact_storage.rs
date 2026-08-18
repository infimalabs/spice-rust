//! Deterministic current container for worktree-exact verifier evidence.

use std::io::{Cursor, Read as _, Write as _};

use flate2::Compression;
use flate2::bufread::GzDecoder;
use flate2::write::GzEncoder;

const MAGIC: [u8; 2] = [0x1f, 0x8b];
/// The measured exact document is about 45 MiB. Keep bounded headroom for
/// report growth while refusing unbounded plain inputs and expansion.
const MAX_PLAIN_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn is_current(stored: &[u8]) -> bool {
    stored.starts_with(&MAGIC)
}

pub(super) fn encode(document: &[u8]) -> Result<Vec<u8>, String> {
    if document.len() > MAX_PLAIN_BYTES {
        return Err(format!(
            "exact evidence is {} bytes; maximum plain length is {MAX_PLAIN_BYTES}",
            document.len(),
        ));
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(document)
        .map_err(|error| format!("compress exact evidence: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("finish exact-evidence compression: {error}"))
}

/// Expand the sole current representation and reject every other format.
pub(super) fn decode_current(stored: &[u8]) -> Result<Vec<u8>, String> {
    if !is_current(stored) {
        return Err("exact evidence does not use the current gzip container".to_owned());
    }
    let mut document = Vec::new();
    let mut decoder = GzDecoder::new(Cursor::new(stored));
    (&mut decoder)
        .take(u64::try_from(MAX_PLAIN_BYTES + 1).expect("bounded exact length fits u64"))
        .read_to_end(&mut document)
        .map_err(|error| format!("decompress exact evidence: {error}"))?;
    if document.len() > MAX_PLAIN_BYTES {
        return Err(format!(
            "exact evidence expands beyond {MAX_PLAIN_BYTES} bytes",
        ));
    }
    let consumed = usize::try_from(decoder.into_inner().position())
        .map_err(|_| "exact-evidence gzip position does not fit this platform".to_owned())?;
    if consumed != stored.len() {
        return Err(format!(
            "exact-evidence gzip member consumed {consumed} of {} bytes",
            stored.len(),
        ));
    }
    Ok(document)
}

/// Admit one validated previous-generation plain document for migration.
pub(super) fn decode_plain_migration(stored: &[u8]) -> Result<Vec<u8>, String> {
    if stored.len() > MAX_PLAIN_BYTES {
        return Err(format!(
            "plain exact evidence is {} bytes; maximum is {MAX_PLAIN_BYTES}",
            stored.len(),
        ));
    }
    Ok(stored.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_container_is_deterministic_bounded_and_exact() {
        let document = b"exact evidence".repeat(4096);
        let stored = encode(&document).expect("encode exact evidence");
        assert!(is_current(&stored));
        assert_eq!(
            encode(&document).expect("repeat deterministic encoding"),
            stored,
        );
        assert_eq!(decode_current(&stored).expect("decode current"), document);
        assert!(decode_current(&document).is_err());

        let mut trailing = stored;
        trailing.push(0);
        assert!(
            decode_current(&trailing)
                .expect_err("trailing bytes refuse")
                .contains("consumed")
        );
    }
}
