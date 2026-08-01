//! Rotation of plaintext into indexed, immutable gzip members.
//!
//! Sealing publishes in one order and that order is the crash contract: the
//! gzip member first, then the index that names it, then the removal of the
//! plaintext it was made from. Every prefix of that order is recoverable.
//! Dying before the member is published leaves the plaintext untouched and the
//! seal simply has not happened. Dying between the member and the index leaves
//! an unreferenced member that the next seal overwrites with byte-identical
//! bytes, because the plaintext it derives from has not moved. Dying between
//! the index and the removal leaves plaintext whose member the index already
//! names, which is exactly the residue [`Tape::recover`] deletes.
//!
//! Plaintext is therefore never removed before the bytes it holds are
//! readable from somewhere else.

use std::fmt;
use std::fs;
use std::io::{self, Write};

use flate2::Compression;
use flate2::write::GzEncoder;

use crate::{Error, Operation, Refusal, Tape};

/// First line of the index, naming the one format tape writes.
const FORMAT: &str = "tape 1";

/// Digits in a rendered member number.
///
/// Member numbers are zero-padded so that a directory listing sorts members
/// into the order they were captured in.
pub(crate) const DIGITS: usize = 12;

/// One sealed member.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Member {
    /// Member number, assigned when its first byte was appended.
    pub member: u64,
    /// Plaintext bytes the member carries, before compression.
    pub length: u64,
}

/// Every sealed member of one tape, in ascending member order.
///
/// The index records plaintext lengths rather than compressed lengths because
/// a locus is a position in the bytes that were captured, not in their
/// storage. Compression is a storage decision and must not be visible to a
/// reader resolving a position.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Index {
    members: Vec<Member>,
}

impl Index {
    /// Every sealed member, ascending.
    #[must_use]
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The highest sealed member, if any member has been sealed.
    #[must_use]
    pub fn last(&self) -> Option<Member> {
        self.members.last().copied()
    }

    /// The sealed member with this number, if it is sealed.
    #[must_use]
    pub fn find(&self, member: u64) -> Option<Member> {
        self.members.iter().copied().find(|m| m.member == member)
    }

    /// The member currently accepting appends.
    ///
    /// Members are numbered from one, so a tape that has sealed nothing is
    /// appending to member one.
    #[must_use]
    pub fn next_member(&self) -> u64 {
        self.last().map_or(1, |member| member.member + 1)
    }

    /// Render the one byte-stable form of this index.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::from(FORMAT);
        text.push('\n');
        for member in &self.members {
            text.push_str(&format!(
                "member {:0width$} {}\n",
                member.member,
                member.length,
                width = DIGITS
            ));
        }
        text
    }

    /// Parse an index, refusing anything that is not the current form.
    ///
    /// # Errors
    ///
    /// Returns a refusal naming the first line that is not current.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut lines = text.lines();
        match lines.next() {
            Some(FORMAT) => {}
            Some(other) => return Err(malformed(other)),
            None => return Err(malformed("")),
        }

        let mut members: Vec<Member> = Vec::new();
        for line in lines {
            let member = parse_member(line)?;
            if let Some(last) = members.last()
                && member.member <= last.member
            {
                return Err(malformed(line));
            }
            members.push(member);
        }

        Ok(Self { members })
    }
}

/// Parse one `member` line.
fn parse_member(line: &str) -> Result<Member, Error> {
    let rest = line
        .strip_prefix("member ")
        .ok_or_else(|| malformed(line))?;
    let (number, length) = rest.split_once(' ').ok_or_else(|| malformed(line))?;

    if number.len() != DIGITS || !is_digits(number) || !is_digits(length) {
        return Err(malformed(line));
    }
    let member = number.parse().map_err(|_| malformed(line))?;
    let length = length.parse().map_err(|_| malformed(line))?;

    Ok(Member { member, length })
}

/// True when every byte is an ASCII digit and there is at least one.
fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A refusal naming the line that is not the current index form.
fn malformed(line: &str) -> Error {
    Error::Refused(Refusal::MalformedIndex {
        line: line.to_owned(),
    })
}

/// What one seal did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Seal {
    /// No member was pending, so nothing rotated.
    Empty,
    /// One member rotated into an immutable gzip member.
    Sealed {
        /// The member that rotated.
        member: u64,
        /// Plaintext bytes it carried.
        plaintext: u64,
        /// Bytes the gzip member occupies.
        compressed: u64,
    },
}

impl fmt::Display for Seal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("nothing to seal"),
            Self::Sealed {
                member,
                plaintext,
                compressed,
            } => write!(
                formatter,
                "sealed member {member}: {plaintext} plaintext byte(s) into {compressed}"
            ),
        }
    }
}

impl Tape {
    /// Every sealed member of this tape.
    ///
    /// # Errors
    ///
    /// Returns a refusal when the index on disk is not the current form, or an
    /// I/O failure when it cannot be read.
    pub fn index(&self) -> Result<Index, Error> {
        self.read_index()
    }

    /// Read the index, treating an absent one as an empty tape.
    pub(crate) fn read_index(&self) -> Result<Index, Error> {
        let path = self.index_path();
        match fs::read_to_string(&path) {
            Ok(text) => Index::parse(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Index::default()),
            Err(error) => Err(Error::io(Operation::ReadIndex, path, error)),
        }
    }

    /// Rotate the member currently accepting appends into a gzip member.
    ///
    /// Sealing an empty or absent member is not a failure; it reports
    /// [`Seal::Empty`] and leaves the tape alone. A sealed member is never
    /// rewritten afterwards, so every locus already issued against it keeps
    /// resolving to the same bytes.
    ///
    /// # Errors
    ///
    /// Refuses when another writer holds the tape. Returns an I/O or
    /// publication failure when the member or index cannot be published; in
    /// every such case the plaintext is left in place.
    pub fn seal(&self) -> Result<Seal, Error> {
        let _possession = self.possess()?;
        let index = self.read_index()?;
        self.recover(&index)?;

        let member = index.next_member();
        let plain = self.plain_path(member);
        let bytes = match fs::read(&plain) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Seal::Empty),
            Err(error) => return Err(Error::io(Operation::ReadPlaintext, plain, error)),
        };
        if bytes.is_empty() {
            return Ok(Seal::Empty);
        }

        let compressed = compress(&bytes, &plain)?;
        self.publish(&self.sealed_path(member), &compressed)?;

        let mut sealed = index;
        sealed.members.push(Member {
            member,
            length: bytes.len() as u64,
        });
        self.publish(&self.index_path(), sealed.render().as_bytes())?;

        fs::remove_file(&plain)
            .map_err(|error| Error::io(Operation::RemovePlaintext, plain, error))?;

        Ok(Seal::Sealed {
            member,
            plaintext: bytes.len() as u64,
            compressed: compressed.len() as u64,
        })
    }
}

/// Compress plaintext into one gzip member.
///
/// The encoder is built with the default header, whose modification time is
/// zero, so compressing the same plaintext twice produces the same bytes. That
/// determinism is what makes re-publishing an unreferenced member after a
/// crash a no-op rather than a rewrite.
fn compress(bytes: &[u8], plain: &std::path::Path) -> Result<Vec<u8>, Error> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .and_then(|()| encoder.finish())
        .map_err(|error| Error::io(Operation::Compress, plain, error))
}
