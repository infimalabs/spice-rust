//! Stable loci and the reads that resolve them.
//!
//! A locus is a position in captured bytes, not a position in storage, so it
//! survives the member it names being rotated from plaintext into a gzip
//! member. Resolution consults the index first because sealing publishes the
//! gzip member before the index that names it: an index entry therefore proves
//! its member exists, while the absence of one only means the member has not
//! been sealed *yet*.
//!
//! That ordering leaves exactly one race for a reader that takes no lock. A
//! reader can observe an index without member N, and by the time it opens N's
//! plaintext the sealer has published N and removed it. The plaintext is then
//! missing without the member being unknown. Re-reading the index tells the
//! two cases apart: an index that changed means a seal landed underneath and
//! the read retries against the sealed member, while an unchanged index means
//! the member was never issued.

use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

use flate2::read::GzDecoder;

use crate::{Error, Extent, Operation, Refusal, Tape};

/// Times a reader re-resolves while a sealer publishes underneath it.
///
/// One retry is enough for the race this guards, because a member is sealed
/// once and never returns to plaintext. The remaining attempts exist so that a
/// reader racing several sealers in a row still terminates with an answer
/// rather than a refusal.
const ATTEMPTS: usize = 4;

/// A stable position in one tape.
///
/// The member is assigned when the first byte of the range is appended and
/// never changes. Nothing here records whether the member is plaintext or
/// sealed, because a caller holding a locus is not entitled to care.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Locus {
    /// Member holding these bytes.
    pub member: u64,
    /// Offset of the first byte within the member's plaintext.
    pub offset: u64,
    /// Bytes this locus covers.
    pub length: u64,
}

impl Locus {
    /// A locus over one member's byte range.
    #[must_use]
    pub const fn new(member: u64, offset: u64, length: u64) -> Self {
        Self {
            member,
            offset,
            length,
        }
    }

    /// The position just past this locus, unless it overflows.
    const fn end(self) -> Option<u64> {
        self.offset.checked_add(self.length)
    }
}

impl fmt::Display for Locus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "member {} offset {} length {}",
            self.member, self.offset, self.length
        )
    }
}

impl Tape {
    /// Reproduce the exact bytes one locus covers.
    ///
    /// The same locus answers identically whether its member is still
    /// accepting appends or has been sealed.
    ///
    /// # Errors
    ///
    /// Refuses a locus naming a member this tape never issued, and a locus
    /// reaching past the end of its member. Returns an I/O failure when a
    /// member exists but cannot be read.
    pub fn read(&self, locus: Locus) -> Result<Vec<u8>, Error> {
        for _ in 0..ATTEMPTS {
            let index = self.read_index()?;

            if let Some(member) = index.find(locus.member) {
                validate(locus, member.length)?;
                let plaintext = self.read_sealed(locus.member)?;
                return slice(locus, &plaintext);
            }

            if locus.member > index.next_member() {
                return Err(unknown(locus));
            }

            if let Some(bytes) = self.read_active(locus)? {
                return Ok(bytes);
            }

            // The plaintext is gone. Either a seal landed between the two
            // observations, or this member was never issued at all.
            if self.read_index()? == index {
                return Err(unknown(locus));
            }
        }

        Err(Error::Refused(Refusal::MemberVanished {
            member: locus.member,
        }))
    }

    /// Read through one previously admitted extent without rediscovering the
    /// complete tape index.
    ///
    /// The extent is an observation, not caller authority: its member and
    /// range must contain the locus. An active member that sealed after the
    /// observation is read from its immutable replacement.
    ///
    /// # Errors
    ///
    /// Refuses a mismatched or out-of-range locus and returns an I/O failure
    /// when neither the observed storage nor its sealed successor can be read.
    pub fn read_extent(&self, locus: Locus, extent: Extent) -> Result<Vec<u8>, Error> {
        self.read_extent_bounded(locus, extent, u64::MAX)
    }

    /// Read an observed range with a ceiling on plaintext read or decompressed.
    ///
    /// Sealed members require decompression from their beginning. Their full
    /// length must fit the allowance, including when an active member seals
    /// after observation. An understated extent cannot bypass the decoder cap.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::MemberReadLimit`] when the read cannot fit the allowance, or
    /// the same range and storage failures as [`Self::read_extent`].
    pub fn read_extent_bounded(
        &self,
        locus: Locus,
        extent: Extent,
        max_bytes: u64,
    ) -> Result<Vec<u8>, Error> {
        if locus.member != extent.member {
            return Err(unknown(locus));
        }
        validate(locus, extent.length)?;
        let limit = || {
            Error::Refused(Refusal::MemberReadLimit {
                member: locus.member,
                limit: max_bytes,
            })
        };
        if locus.length > max_bytes {
            return Err(limit());
        }
        if !extent.sealed
            && let Some(bytes) = self.read_active(locus)?
        {
            return Ok(bytes);
        }
        if extent.length > max_bytes {
            return Err(limit());
        }
        self.read_sealed_bounded(locus.member, max_bytes)
            .and_then(|plaintext| slice(locus, &plaintext))
    }

    /// Stream an observed range through `buffer` without holding it.
    ///
    /// `visit` receives consecutive chunks of at most `buffer.len()` bytes
    /// covering exactly the range. Active plaintext is read in place. A sealed
    /// member, including one that sealed after observation, is decompressed
    /// forward with the bytes before the range discarded as they decode, so
    /// memory stays independent of the member's size.
    ///
    /// # Errors
    ///
    /// Refuses a mismatched or out-of-range locus, returns an I/O failure when
    /// the storage cannot be read or ends short of the range, and returns the
    /// first failure `visit` reports.
    ///
    /// # Panics
    ///
    /// Panics when `buffer` is empty, which could never make progress.
    pub fn stream_extent<E: From<Error>>(
        &self,
        locus: Locus,
        extent: Extent,
        buffer: &mut [u8],
        mut visit: impl FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        assert!(!buffer.is_empty(), "a streamed read needs buffer space");
        if locus.member != extent.member {
            return Err(unknown(locus).into());
        }
        validate(locus, extent.length)?;
        let (operation, path, mut reader) = self.open_range(locus, extent)?;
        let mut remaining = locus.length;
        while remaining != 0 {
            let want = buffer
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let read = match reader.read(&mut buffer[..want]) {
                Ok(0) => {
                    let short = io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("range ended {remaining} bytes short"),
                    );
                    return Err(Error::io(operation, path, short).into());
                }
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(Error::io(operation, path, error).into()),
            };
            visit(&buffer[..read])?;
            remaining -= read as u64;
        }
        Ok(())
    }

    /// Open a reader positioned at the first byte of an observed range.
    fn open_range(
        &self,
        locus: Locus,
        extent: Extent,
    ) -> Result<(Operation, PathBuf, Box<dyn Read>), Error> {
        if !extent.sealed {
            let path = self.plain_path(locus.member);
            match File::open(&path) {
                Ok(mut file) => {
                    let length = file
                        .metadata()
                        .map_err(|error| Error::io(Operation::ReadPlaintext, &path, error))?
                        .len();
                    validate(locus, length)?;
                    file.seek(SeekFrom::Start(locus.offset))
                        .map_err(|error| Error::io(Operation::ReadPlaintext, &path, error))?;
                    return Ok((Operation::ReadPlaintext, path, Box::new(file)));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::io(Operation::ReadPlaintext, path, error)),
            }
        }
        let path = self.sealed_path(locus.member);
        let file =
            File::open(&path).map_err(|error| Error::io(Operation::ReadSealed, &path, error))?;
        let mut decoder = GzDecoder::new(file);
        let skipped = io::copy(&mut (&mut decoder).take(locus.offset), &mut io::sink())
            .map_err(|error| Error::io(Operation::ReadSealed, &path, error))?;
        if skipped != locus.offset {
            return Err(Error::Refused(Refusal::OutOfRange {
                locus,
                length: skipped,
            }));
        }
        Ok((Operation::ReadSealed, path, Box::new(decoder)))
    }

    /// Read a range from the member still accepting appends.
    ///
    /// Reports `None` rather than an error when the plaintext is absent, so
    /// the caller can decide whether that means sealed or never issued.
    fn read_active(&self, locus: Locus) -> Result<Option<Vec<u8>>, Error> {
        let path = self.plain_path(locus.member);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::io(Operation::ReadPlaintext, path, error)),
        };

        let length = file
            .metadata()
            .map_err(|error| Error::io(Operation::ReadPlaintext, &path, error))?
            .len();
        validate(locus, length)?;

        let mut bytes = vec![0; span(locus, length)?];
        file.seek(SeekFrom::Start(locus.offset))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|error| Error::io(Operation::ReadPlaintext, path, error))?;

        Ok(Some(bytes))
    }

    /// Decompress one sealed member back to its captured plaintext.
    fn read_sealed(&self, member: u64) -> Result<Vec<u8>, Error> {
        self.read_sealed_bounded(member, u64::MAX)
    }

    fn read_sealed_bounded(&self, member: u64, max_bytes: u64) -> Result<Vec<u8>, Error> {
        let path = self.sealed_path(member);
        let file =
            File::open(&path).map_err(|error| Error::io(Operation::ReadSealed, &path, error))?;

        let mut plaintext = Vec::new();
        GzDecoder::new(file)
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut plaintext)
            .map_err(|error| Error::io(Operation::ReadSealed, path, error))?;
        if plaintext.len() as u64 > max_bytes {
            return Err(Error::Refused(Refusal::MemberReadLimit {
                member,
                limit: max_bytes,
            }));
        }
        Ok(plaintext)
    }
}

/// Refuse a locus whose member this tape never issued.
fn unknown(locus: Locus) -> Error {
    Error::Refused(Refusal::UnknownMember {
        member: locus.member,
    })
}

/// Refuse a locus that reaches past the bytes its member holds.
fn validate(locus: Locus, length: u64) -> Result<(), Error> {
    match locus.end() {
        Some(end) if end <= length => Ok(()),
        _ => Err(Error::Refused(Refusal::OutOfRange { locus, length })),
    }
}

/// This locus's length as a buffer size, refusing one this target cannot hold.
fn span(locus: Locus, length: u64) -> Result<usize, Error> {
    usize::try_from(locus.length).map_err(|_| Error::Refused(Refusal::OutOfRange { locus, length }))
}

/// Take a locus's range out of a member's plaintext.
fn slice(locus: Locus, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
    let held = plaintext.len() as u64;
    validate(locus, held)?;

    let start = span(Locus::new(locus.member, 0, locus.offset), held)?;
    let length = span(locus, held)?;

    Ok(plaintext[start..start + length].to_vec())
}
