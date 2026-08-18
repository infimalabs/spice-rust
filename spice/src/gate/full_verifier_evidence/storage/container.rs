//! Current archive-container framing and bounded decompression.

use std::io::{Cursor, Read as _, Write as _};

use flate2::Compression;
use flate2::bufread::GzDecoder;
use flate2::write::GzEncoder;

const MAGIC: &[u8; 8] = b"SPVARC\0\0";
const VERSION: u32 = 1;
const DIGEST_BYTES: usize = 32;
const HEADER_BYTES: usize = MAGIC.len() + 4 + 8 + 8 + DIGEST_BYTES;
const RAW_GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// The old corpus is roughly 45 MiB per plain bundle. Leave bounded headroom
/// for report growth while refusing decompression bombs before allocation.
pub(super) const MAX_PLAIN_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn is_current(document: &[u8]) -> bool {
    document.starts_with(MAGIC)
}

pub(super) fn encode(plain: &[u8]) -> Result<Vec<u8>, String> {
    if plain.len() > MAX_PLAIN_BYTES {
        return Err(format!(
            "archive document is {} bytes; maximum plain length is {MAX_PLAIN_BYTES}",
            plain.len(),
        ));
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(plain)
        .map_err(|error| format!("compress archive document: {error}"))?;
    let payload = encoder
        .finish()
        .map_err(|error| format!("finish archive compression: {error}"))?;
    let plain_len = u64::try_from(plain.len()).expect("plain archive length fits u64");
    let stored_len = u64::try_from(payload.len()).expect("stored archive length fits u64");
    let mut document = Vec::with_capacity(HEADER_BYTES + payload.len());
    document.extend_from_slice(MAGIC);
    document.extend_from_slice(&VERSION.to_be_bytes());
    document.extend_from_slice(&plain_len.to_be_bytes());
    document.extend_from_slice(&stored_len.to_be_bytes());
    document.extend_from_slice(blake3::hash(plain).as_bytes());
    document.extend_from_slice(&payload);
    Ok(document)
}

pub(super) fn decode(document: &[u8]) -> Result<Vec<u8>, String> {
    if document.len() < HEADER_BYTES || !document.starts_with(MAGIC) {
        return Err("archive document does not use the current container".to_owned());
    }
    let version = u32::from_be_bytes(
        document[MAGIC.len()..MAGIC.len() + 4]
            .try_into()
            .expect("version field has fixed width"),
    );
    if version != VERSION {
        return Err(format!(
            "unsupported archive container version {version}; expected {VERSION}",
        ));
    }
    let plain_start = MAGIC.len() + 4;
    let plain_len = u64::from_be_bytes(
        document[plain_start..plain_start + 8]
            .try_into()
            .expect("plain-length field has fixed width"),
    );
    let stored_start = plain_start + 8;
    let stored_len = u64::from_be_bytes(
        document[stored_start..stored_start + 8]
            .try_into()
            .expect("stored-length field has fixed width"),
    );
    let plain_len = usize::try_from(plain_len)
        .map_err(|_| "archive plain length does not fit this platform".to_owned())?;
    if plain_len > MAX_PLAIN_BYTES {
        return Err(format!(
            "archive plain length {plain_len} exceeds maximum {MAX_PLAIN_BYTES}",
        ));
    }
    let stored_len = usize::try_from(stored_len)
        .map_err(|_| "archive stored length does not fit this platform".to_owned())?;
    let expected = HEADER_BYTES
        .checked_add(stored_len)
        .ok_or_else(|| "archive stored length overflows this platform".to_owned())?;
    if document.len() != expected {
        return Err(format!(
            "archive stored length is {stored_len}, but container length is {}",
            document.len().saturating_sub(HEADER_BYTES),
        ));
    }
    let digest_start = stored_start + 8;
    let digest = &document[digest_start..digest_start + DIGEST_BYTES];
    let payload = &document[HEADER_BYTES..];
    let mut plain = Vec::with_capacity(plain_len);
    let mut decoder = GzDecoder::new(Cursor::new(payload));
    (&mut decoder)
        .take(u64::try_from(plain_len + 1).expect("bounded length fits u64"))
        .read_to_end(&mut plain)
        .map_err(|error| format!("decompress archive document: {error}"))?;
    if plain.len() != plain_len {
        return Err(format!(
            "archive expanded to {} bytes; header names {plain_len}",
            plain.len(),
        ));
    }
    if blake3::hash(&plain).as_bytes() != digest {
        return Err("archive document integrity digest does not match its payload".to_owned());
    }
    let consumed = usize::try_from(decoder.into_inner().position())
        .map_err(|_| "archive gzip position does not fit this platform".to_owned())?;
    if consumed != payload.len() {
        return Err(format!(
            "archive gzip member consumed {consumed} of {} stored bytes",
            payload.len(),
        ));
    }
    Ok(plain)
}

pub(super) fn decode_pre_container(document: &[u8]) -> Result<Vec<u8>, String> {
    if document.starts_with(&RAW_GZIP_MAGIC) {
        let mut plain = Vec::new();
        let mut decoder = GzDecoder::new(Cursor::new(document));
        (&mut decoder)
            .take(u64::try_from(MAX_PLAIN_BYTES + 1).expect("bounded length fits u64"))
            .read_to_end(&mut plain)
            .map_err(|error| format!("decompress raw-gzip archive document: {error}"))?;
        if plain.len() > MAX_PLAIN_BYTES {
            return Err(format!(
                "raw-gzip archive expands beyond {MAX_PLAIN_BYTES} bytes",
            ));
        }
        let consumed = usize::try_from(decoder.into_inner().position())
            .map_err(|_| "raw-gzip archive position does not fit this platform".to_owned())?;
        if consumed != document.len() {
            return Err(format!(
                "raw-gzip archive member consumed {consumed} of {} bytes",
                document.len(),
            ));
        }
        return Ok(plain);
    }
    if document.len() > MAX_PLAIN_BYTES {
        return Err(format!(
            "plain archive is {} bytes; maximum is {MAX_PLAIN_BYTES}",
            document.len(),
        ));
    }
    Ok(document.to_vec())
}
