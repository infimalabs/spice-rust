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
use std::fs::{self, File};
use std::io::{self, Read, Write};

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

/// Observable bounds of one durable tape member.
///
/// A sealed extent cannot grow. An active extent may be longer on the next
/// observation, which is the cursor comparison a daemonless listener uses to
/// heal notifications it never received.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Extent {
    /// Member number.
    pub member: u64,
    /// Plaintext bytes currently readable from the member.
    pub length: u64,
    /// Whether the member has crossed the immutable index boundary.
    pub sealed: bool,
}

/// Complete readable extents admitted by caller-owned discovery bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedExtents {
    /// Readable members in ascending order.
    pub extents: Vec<Extent>,
    /// Exact index bytes read and parsed.
    pub index_bytes: u64,
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
        let index = usize::try_from(member.checked_sub(1)?).ok()?;
        self.members
            .get(index)
            .copied()
            .filter(|candidate| candidate.member == member)
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
    /// This admits exactly what [`Index::render`] emits. The index is the map
    /// from a locus to the bytes it names, so a reading that is not the one
    /// writing produced is a reading of something else: every spelling this
    /// accepts is a spelling some writer could have meant, and only one writer
    /// exists. Anything else is refused by the line it appeared on rather than
    /// repaired, because tape cannot know which of two meanings was intended.
    ///
    /// # Errors
    ///
    /// Returns a refusal naming the first line that is not current.
    pub fn parse(text: &str) -> Result<Self, Error> {
        // Every rendered line is terminated, so text that stops without a
        // newline is truncated rather than current. `str::lines` yields the
        // same segments either way and would admit the truncation silently.
        let Some(body) = text.strip_suffix('\n') else {
            return Err(malformed(text.lines().next_back().unwrap_or("")));
        };

        let mut lines = body.split('\n');
        match lines.next() {
            Some(FORMAT) => {}
            Some(other) => return Err(malformed(other)),
            None => return Err(malformed("")),
        }

        let mut members: Vec<Member> = Vec::new();
        for line in lines {
            let member = parse_member(line)?;

            // Members are sealed one after another starting at one, so the
            // index is a contiguous run and its position states its number.
            // A gap would name a member whose bytes no locus can reach, and a
            // repeat or a descent would give one number two lengths.
            let expected = members.len() as u64 + 1;
            if member.member != expected {
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

    if number.len() != DIGITS || !is_digits(number) || !is_length(length) {
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

/// True when the text is the one spelling [`Index::render`] gives a length.
///
/// Lengths are rendered plainly, so a leading zero is a second spelling of a
/// number that already has one, and two spellings of one index would give the
/// same tape two byte-stable forms. Zero is refused outright rather than as a
/// spelling: [`Tape::seal`] reports an empty member as [`Seal::Empty`] and
/// seals nothing, so a sealed member of no bytes never existed to be named.
fn is_length(text: &str) -> bool {
    is_digits(text) && !text.starts_with('0')
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

    /// Snapshot every currently readable member bound in ascending order.
    ///
    /// The snapshot reads only metadata and the index. A seal racing this read
    /// may make the active member disappear after an older index was read; in
    /// that case the next snapshot observes the published index and the
    /// caller's durable cursor heals the missed transition.
    ///
    /// # Errors
    ///
    /// Returns a refusal when the index is not current, or an I/O failure when
    /// the index or active member metadata cannot be read.
    pub fn extents(&self) -> Result<Vec<Extent>, Error> {
        let index = self.read_index()?;
        let mut extents = index
            .members()
            .iter()
            .map(|member| Extent {
                member: member.member,
                length: member.length,
                sealed: true,
            })
            .collect::<Vec<_>>();

        let active = index.next_member();
        let path = self.plain_path(active);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => extents.push(Extent {
                member: active,
                length: metadata.len(),
                sealed: false,
            }),
            Ok(_) => {
                return Err(Error::io(
                    Operation::InspectExtents,
                    path,
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "active tape member is not a regular file",
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(Operation::InspectExtents, path, error)),
        }

        Ok(extents)
    }

    /// Snapshot readable members without reading an oversized index or
    /// materializing more extents than the caller admitted.
    ///
    /// # Errors
    ///
    /// Returns a typed limit refusal or the same index and metadata failures
    /// as [`Self::extents`].
    pub fn extents_bounded(
        &self,
        max_index_bytes: u64,
        max_extents: usize,
    ) -> Result<BoundedExtents, Error> {
        let path = self.index_path();
        let (index, index_bytes) = match File::open(&path) {
            Ok(file) => read_bounded_index(file, &path, max_index_bytes, max_extents)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Index::default(), 0),
            Err(error) => return Err(Error::io(Operation::ReadIndex, path, error)),
        };
        let mut extents = index
            .members()
            .iter()
            .map(|member| Extent {
                member: member.member,
                length: member.length,
                sealed: true,
            })
            .collect::<Vec<_>>();
        let active = index.next_member();
        let active_path = self.plain_path(active);
        match fs::metadata(&active_path) {
            Ok(metadata) if metadata.is_file() => {
                if extents.len() == max_extents {
                    return Err(Error::Refused(Refusal::ExtentReadLimit {
                        limit: max_extents,
                        observed: max_extents.saturating_add(1),
                        index_bytes,
                    }));
                }
                extents.push(Extent {
                    member: active,
                    length: metadata.len(),
                    sealed: false,
                });
            }
            Ok(_) => {
                return Err(Error::io(
                    Operation::InspectExtents,
                    active_path,
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "active tape member is not a regular file",
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io(Operation::InspectExtents, active_path, error)),
        }
        Ok(BoundedExtents {
            extents,
            index_bytes,
        })
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

fn read_bounded_index(
    file: File,
    path: &std::path::Path,
    max_bytes: u64,
    max_extents: usize,
) -> Result<(Index, u64), Error> {
    let observed = file
        .metadata()
        .map_err(|error| Error::io(Operation::ReadIndex, path, error))?
        .len();
    if observed > max_bytes {
        return Err(Error::Refused(Refusal::IndexReadLimit {
            limit: max_bytes,
            observed,
        }));
    }
    let mut text = String::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)
        .map_err(|error| Error::io(Operation::ReadIndex, path, error))?;
    let index_bytes = u64::try_from(text.len()).expect("a string length fits u64");
    if index_bytes > max_bytes {
        return Err(Error::Refused(Refusal::IndexReadLimit {
            limit: max_bytes,
            observed: index_bytes,
        }));
    }
    let observed_extents = text.lines().count().saturating_sub(1);
    if observed_extents > max_extents {
        return Err(Error::Refused(Refusal::ExtentReadLimit {
            limit: max_extents,
            observed: max_extents.saturating_add(1),
            index_bytes,
        }));
    }
    Ok((Index::parse(&text)?, index_bytes))
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
