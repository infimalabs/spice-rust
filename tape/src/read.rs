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

use flate2::read::GzDecoder;

use crate::{Error, Operation, Refusal, Tape};

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
        let path = self.sealed_path(member);
        let file =
            File::open(&path).map_err(|error| Error::io(Operation::ReadSealed, &path, error))?;

        let mut plaintext = Vec::new();
        GzDecoder::new(file)
            .read_to_end(&mut plaintext)
            .map_err(|error| Error::io(Operation::ReadSealed, path, error))?;

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
