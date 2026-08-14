use std::io::{Cursor, Read, Write};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::{FileImage, InstallError};

const FORMAT_VERSION: u16 = 1;
const MAGIC: &[u8] = b"spice-install-rollback\0";
const MAX_UNCOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// A compressed, digest-bound snapshot used only to reverse a version-7
/// migration. Keeping large executable images out of the JSON object graph
/// prevents every progress receipt from expanding each byte into a decimal
/// array (and then duplicating the operation reversal inventory).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RollbackArchive {
    format: u16,
    uncompressed_bytes: u64,
    digest: Digest,
    payload: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RollbackState {
    pub(super) record: FileImage,
    pub(super) operations: Vec<Option<FileImage>>,
    pub(super) retirements: Vec<Option<FileImage>>,
}

impl RollbackArchive {
    pub(super) fn new(state: &RollbackState) -> Result<Self, InstallError> {
        let bytes = encode_state(state)?;
        let uncompressed_bytes = u64::try_from(bytes.len()).map_err(|_| {
            invalid("migration rollback archive is too large for its canonical length")
        })?;
        if uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(invalid(
                "migration rollback archive exceeds the supported uncompressed size",
            ));
        }
        let digest = Digest::compute(Domain::InstallArtifact, &bytes);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&bytes)
            .map_err(|_| invalid("migration rollback archive did not compress"))?;
        let compressed = encoder
            .finish()
            .map_err(|_| invalid("migration rollback archive did not finish compression"))?;
        Ok(Self {
            format: FORMAT_VERSION,
            uncompressed_bytes,
            digest,
            payload: STANDARD.encode(compressed),
        })
    }

    pub(super) fn decode(&self) -> Result<RollbackState, InstallError> {
        if self.format != FORMAT_VERSION {
            return Err(invalid(
                "migration rollback archive has an unsupported format",
            ));
        }
        if self.uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(invalid(
                "migration rollback archive exceeds the supported uncompressed size",
            ));
        }
        let compressed = STANDARD
            .decode(&self.payload)
            .map_err(|_| invalid("migration rollback archive payload is not base64"))?;
        if STANDARD.encode(&compressed) != self.payload {
            return Err(invalid(
                "migration rollback archive payload is not canonical base64",
            ));
        }
        let mut decoder = GzDecoder::new(Cursor::new(compressed));
        let mut bytes = Vec::new();
        Read::by_ref(&mut decoder)
            .take(self.uncompressed_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("migration rollback archive did not decompress"))?;
        if u64::try_from(bytes.len()).ok() != Some(self.uncompressed_bytes) {
            return Err(invalid(
                "migration rollback archive length does not match its manifest",
            ));
        }
        if Digest::compute(Domain::InstallArtifact, &bytes) != self.digest {
            return Err(invalid(
                "migration rollback archive digest does not match its payload",
            ));
        }
        decode_state(&bytes)
    }
}

fn encode_state(state: &RollbackState) -> Result<Vec<u8>, InstallError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    encode_image(&mut bytes, &state.record)?;
    encode_images(&mut bytes, &state.operations)?;
    encode_images(&mut bytes, &state.retirements)?;
    Ok(bytes)
}

fn encode_images(bytes: &mut Vec<u8>, images: &[Option<FileImage>]) -> Result<(), InstallError> {
    let count = u32::try_from(images.len())
        .map_err(|_| invalid("migration rollback archive has too many images"))?;
    bytes.extend_from_slice(&count.to_be_bytes());
    for image in images {
        match image {
            Some(image) => {
                bytes.push(1);
                encode_image(bytes, image)?;
            }
            None => bytes.push(0),
        }
    }
    Ok(())
}

fn encode_image(bytes: &mut Vec<u8>, image: &FileImage) -> Result<(), InstallError> {
    if !image.validate() {
        return Err(invalid(
            "migration rollback archive contains an invalid file image",
        ));
    }
    let length = u64::try_from(image.bytes.len())
        .map_err(|_| invalid("migration rollback archive image is too large"))?;
    bytes.extend_from_slice(&image.mode.to_be_bytes());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&image.bytes);
    Ok(())
}

fn decode_state(bytes: &[u8]) -> Result<RollbackState, InstallError> {
    let mut reader = ArchiveReader::new(bytes);
    if reader.take(MAGIC.len())? != MAGIC {
        return Err(invalid("migration rollback archive has the wrong magic"));
    }
    if reader.u16()? != FORMAT_VERSION {
        return Err(invalid(
            "migration rollback archive body has an unsupported format",
        ));
    }
    let record = reader.image()?;
    let operations = reader.images()?;
    let retirements = reader.images()?;
    if !reader.is_finished() {
        return Err(invalid(
            "migration rollback archive has trailing non-canonical bytes",
        ));
    }
    Ok(RollbackState {
        record,
        operations,
        retirements,
    })
}

struct ArchiveReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ArchiveReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], InstallError> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("migration rollback archive is truncated"))?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16, InstallError> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .expect("the archive reader returned exactly two bytes");
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, InstallError> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .expect("the archive reader returned exactly four bytes");
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, InstallError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .expect("the archive reader returned exactly eight bytes");
        Ok(u64::from_be_bytes(bytes))
    }

    fn image(&mut self) -> Result<FileImage, InstallError> {
        let mode = self.u32()?;
        let length = usize::try_from(self.u64()?)
            .map_err(|_| invalid("migration rollback archive image length does not fit memory"))?;
        let bytes = self.take(length)?.to_vec();
        let image = FileImage::new(mode, bytes);
        if !image.validate() {
            return Err(invalid(
                "migration rollback archive contains an invalid file image",
            ));
        }
        Ok(image)
    }

    fn images(&mut self) -> Result<Vec<Option<FileImage>>, InstallError> {
        let count = usize::try_from(self.u32()?)
            .expect("every u32 image count fits the supported platforms");
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            images.push(match self.take(1)?[0] {
                0 => None,
                1 => Some(self.image()?),
                _ => {
                    return Err(invalid(
                        "migration rollback archive has an invalid image presence tag",
                    ));
                }
            });
        }
        Ok(images)
    }

    const fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn invalid(reason: &str) -> InstallError {
    InstallError::InvalidRecord(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_round_trips_and_rejects_payload_corruption() {
        let state = RollbackState {
            record: FileImage::new(0o600, b"receipt".to_vec()),
            operations: vec![Some(FileImage::new(0o755, vec![b'x'; 1024 * 1024])), None],
            retirements: vec![Some(FileImage::new(0o755, vec![b'y'; 1024 * 1024]))],
        };
        let archive = RollbackArchive::new(&state).expect("archive fixture state");
        assert_eq!(archive.decode().expect("decode archive fixture"), state);
        assert!(archive.payload.len() < 32 * 1024);

        let mut corrupt = archive;
        corrupt.payload.replace_range(0..1, "!");
        assert!(matches!(
            corrupt.decode(),
            Err(InstallError::InvalidRecord(_))
        ));
    }
}
