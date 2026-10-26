//! Verbatim vendor capture.
//!
//! Tape commits bytes before anything interprets them. An append is durable
//! before the caller is allowed to decode it, so a decoder that panics, a
//! dialect that cannot classify, and a resident that dies all leave the
//! original bytes intact and re-readable.
//!
//! Tape is not authority. It never decides what bytes mean, never merges two
//! captures, and never rewrites a member it already sealed. The one promise it
//! makes is positional: a [`Locus`] handed back by [`Tape::tee`] resolves to
//! the same bytes forever, whether the member holding it is still plaintext or
//! has since been rotated into an immutable gzip member.
//!
//! A tape directory holds plaintext for the member currently accepting
//! appends, one gzip member per sealed member, and one index naming every
//! sealed member and its plaintext length. Naming the plaintext after its own
//! member number is what makes recovery unambiguous: a plaintext file whose
//! member the index already lists is the residue of a seal that published and
//! then died, and finishing that seal is a deletion rather than a guess.

#![forbid(unsafe_code)]

mod read;
mod seal;
mod tee;

pub use read::Locus;
pub use seal::{BoundedExtents, Extent, Index, Member, Seal};

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sill::{Acquisition, LockFile, LockName, Possession};

/// Name of the index within a tape directory.
const INDEX: &str = "index";

/// Name of the durable source-generation identity.
const SOURCE: &str = "source";

/// Header of the one source identity form tape writes.
const SOURCE_FORMAT: &str = "tape-source 1";

/// Name of the writer lock within a tape directory.
const LOCK: &str = "tape.lock";

/// Mode for every file tape publishes.
#[cfg(unix)]
const PRIVATE: u32 = 0o600;

/// Mode for the tape directory itself.
#[cfg(unix)]
const PRIVATE_DIRECTORY: u32 = 0o700;

/// Distinguishes concurrent stagers within one process.
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// One capture directory.
///
/// A tape is a directory rather than a file because sealing publishes whole
/// members by rename, and rename is only atomic within one directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tape {
    root: PathBuf,
}

/// Opaque identity of one durable tape generation.
///
/// The value changes when a tape directory is replaced, but not when members
/// within one tape are sealed. Its spelling is deliberately opaque to callers;
/// equality and serialization are the complete public contract.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct SourceId(String);

impl SourceId {
    fn fresh() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }

    /// Canonical opaque spelling used in durable checkpoints.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::str::FromStr for SourceId {
    type Err = SourceIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parsed = uuid::Uuid::parse_str(text).map_err(|_| SourceIdError)?;
        let canonical = parsed.hyphenated().to_string();
        if canonical != text {
            return Err(SourceIdError);
        }
        Ok(Self(canonical))
    }
}

impl<'de> serde::Deserialize<'de> for SourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A source identity was not in tape's one current spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceIdError;

impl fmt::Display for SourceIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("source identity is not a canonical UUID")
    }
}

impl std::error::Error for SourceIdError {}

impl Tape {
    /// Open a tape, creating the directory when it does not yet exist.
    ///
    /// # Errors
    ///
    /// Returns an I/O failure when the directory cannot be created or
    /// resolved.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        let requested = root.as_ref();
        create_directory(requested)?;
        let root = fs::canonicalize(requested)
            .map_err(|error| Error::io(Operation::OpenTape, requested, error))?;

        let tape = Self { root };
        tape.adopt_source()?;
        Ok(tape)
    }

    /// Open an existing tape without creating or recovering anything.
    ///
    /// This is the observer door. A missing directory is an error rather than
    /// an invitation to create one, so a read-only surface cannot leave an
    /// empty tape behind merely by looking in the wrong place.
    ///
    /// # Errors
    ///
    /// Returns an I/O failure when the path is absent, is not a directory, or
    /// cannot be resolved.
    pub fn open_existing(root: impl AsRef<Path>) -> Result<Self, Error> {
        let requested = root.as_ref();
        let metadata = fs::symlink_metadata(requested)
            .map_err(|error| Error::io(Operation::OpenTape, requested, error))?;
        if !metadata.is_dir() {
            return Err(Error::io(
                Operation::OpenTape,
                requested,
                io::Error::new(io::ErrorKind::InvalidInput, "tape root is not a directory"),
            ));
        }
        let root = fs::canonicalize(requested)
            .map_err(|error| Error::io(Operation::OpenTape, requested, error))?;

        Ok(Self { root })
    }

    /// The directory holding this tape.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Durable identity of this exact tape generation.
    ///
    /// This read never creates or repairs an identity. Producers migrate an
    /// identity-less owned tape through [`Tape::open`]; observers using
    /// [`Tape::open_existing`] receive the missing, unreadable, or malformed
    /// identity as a failure instead of guessing from paths or metadata.
    ///
    /// # Errors
    ///
    /// Returns an I/O failure when the identity cannot be read, or a refusal
    /// when its bytes are not the one form tape publishes.
    pub fn source_id(&self) -> Result<SourceId, Error> {
        let path = self.source_path();
        let text = fs::read_to_string(&path)
            .map_err(|error| Error::io(Operation::ReadSource, &path, error))?;
        parse_source(&text)
    }

    /// Plaintext path for one member.
    fn plain_path(&self, member: u64) -> PathBuf {
        self.root
            .join(format!("{member:0width$}.plain", width = seal::DIGITS))
    }

    /// Sealed gzip path for one member.
    fn sealed_path(&self, member: u64) -> PathBuf {
        self.root
            .join(format!("{member:0width$}.gz", width = seal::DIGITS))
    }

    /// Path of the index.
    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX)
    }

    /// Path of the durable source identity.
    fn source_path(&self) -> PathBuf {
        self.root.join(SOURCE)
    }

    /// Give one owned legacy tape its only forward source-identity migration.
    fn adopt_source(&self) -> Result<SourceId, Error> {
        match self.source_id() {
            Ok(source) => {
                // A prior publication may have written and renamed the source
                // before its directory synchronization failed. Retrying the
                // owning door commits that already-current name before it
                // reports the generation durable.
                self.sync_directory()?;
                return Ok(source);
            }
            Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let _possession = self.possess()?;
        match self.source_id() {
            Ok(source) => Ok(source),
            Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                let source = SourceId::fresh();
                let bytes = format!("{SOURCE_FORMAT}\n{source}\n");
                self.publish(&self.source_path(), bytes.as_bytes())?;
                self.sync_directory()?;
                Ok(source)
            }
            Err(error) => Err(error),
        }
    }

    /// Take the writer lock, refusing rather than blocking when it is held.
    ///
    /// Appending and sealing both mutate the same member, so they serialize
    /// against each other. Refusing a contended lock keeps a stalled peer from
    /// turning into an unbounded wait inside a capture path.
    fn possess(&self) -> Result<Possession, Error> {
        let name = LockName::new("tape").map_err(|_| Error::Refused(Refusal::LockNameRejected))?;
        let path = self.root.join(LOCK);
        let lock = LockFile::new(name, &path);

        match lock
            .try_acquire()
            .map_err(|error| Error::io(Operation::Possess, path, error))?
        {
            Acquisition::Acquired(possession) => Ok(possession),
            Acquisition::Contended => Err(Error::Refused(Refusal::Contended)),
        }
    }

    /// Finish a seal that published its member and died before cleanup.
    ///
    /// A plaintext file whose member the index already names cannot be
    /// accepting appends: the sealed member and the index both exist, so every
    /// locus in it already resolves against the gzip member. Removing it
    /// completes the interrupted seal rather than losing bytes.
    fn recover(&self, index: &Index) -> Result<(), Error> {
        let Some(last) = index.last() else {
            return Ok(());
        };
        let residue = self.plain_path(last.member);
        match fs::remove_file(&residue) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::io(Operation::RemovePlaintext, residue, error)),
        }
    }

    /// Synchronize the tape directory itself.
    ///
    /// Synchronizing a file commits its bytes; it does not commit the name the
    /// bytes answer to. A member created and written but whose directory entry
    /// is still only in the page cache is a member that a power loss can leave
    /// absent while every byte of it survives, which would strand a locus that
    /// tape had already promised.
    fn sync_directory(&self) -> Result<(), Error> {
        sill::authority::Directory::open(&self.root)
            .and_then(|directory| directory.sync())
            .map_err(|error| Error::io(Operation::SyncDirectory, &self.root, error))
    }

    /// Publish bytes into this tape's directory at owner-only mode.
    ///
    /// The bytes are staged here and then handed to [`atomic`] for
    /// replacement, rather than written through `Destination::publish`,
    /// because staging here is what lets the mode be set before any byte is
    /// visible under the final name.
    fn publish(&self, destination: &Path, bytes: &[u8]) -> Result<(), Error> {
        let staged = self.root.join(temporary_name());
        // The handle closes where this block ends, which is ahead of both the
        // unlink below and the rename [`atomic`] performs: neither may run
        // while this process still holds the staged bytes open.
        let written = {
            let mut file = create_private(&staged)
                .map_err(|error| Error::io(Operation::StagePublication, &staged, error))?;
            file.write_all(bytes).and_then(|()| file.sync_all())
        };
        if let Err(error) = written {
            let _ = fs::remove_file(&staged);
            return Err(Error::io(Operation::StagePublication, staged, error));
        }

        let target = atomic::Destination::new(destination)
            .map_err(|error| Error::Publish(Box::new(error)))?;
        if let Err(error) = target.replace_from(&staged) {
            let _ = fs::remove_file(&staged);
            return Err(Error::Publish(Box::new(error)));
        }

        Ok(())
    }
}

/// Create a file readable and writable only by its owner.
#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE)
        .open(path)
}

/// Create a file, on a target without Unix permission bits.
#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Open a member's plaintext for appending, creating it privately if absent.
fn open_member(path: &Path) -> io::Result<File> {
    match create_private_append(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_private_append(path),
        Err(error) => Err(error),
    }
}

/// Create a member's plaintext for appending, failing if it exists.
#[cfg(unix)]
fn create_private_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(PRIVATE)
        .open(path)
}

/// Create a member's plaintext for appending, without Unix permission bits.
#[cfg(not(unix))]
fn create_private_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().append(true).create_new(true).open(path)
}

/// Open a member's plaintext for appending, creating it privately.
#[cfg(unix)]
fn open_private_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(PRIVATE)
        .open(path)
}

/// Open a member's plaintext for appending, without Unix permission bits.
#[cfg(not(unix))]
fn open_private_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().append(true).create(true).open(path)
}

/// Create the tape directory, private to its owner, if it is absent.
fn create_directory(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::DirBuilderExt;

        fs::DirBuilder::new()
            .recursive(true)
            .mode(PRIVATE_DIRECTORY)
            .create(path)
    };
    #[cfg(not(unix))]
    let result = fs::DirBuilder::new().recursive(true).create(path);

    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(Error::io(Operation::OpenTape, path, error)),
    }
}

/// A staging name that cannot collide with a member, an index, or a lock.
fn temporary_name() -> String {
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    format!(".tape-staged-{}-{sequence}.tmp", std::process::id())
}

/// Parse the one framed source identity spelling tape publishes.
fn parse_source(text: &str) -> Result<SourceId, Error> {
    let Some(body) = text.strip_suffix('\n') else {
        return Err(Error::Refused(Refusal::MalformedSource {
            line: text.lines().next_back().unwrap_or("").to_owned(),
        }));
    };
    let mut lines = body.split('\n');
    if lines.next() != Some(SOURCE_FORMAT) {
        return Err(Error::Refused(Refusal::MalformedSource {
            line: body.lines().next().unwrap_or("").to_owned(),
        }));
    }
    let identity = lines.next().ok_or_else(|| {
        Error::Refused(Refusal::MalformedSource {
            line: String::new(),
        })
    })?;
    if let Some(extra) = lines.next() {
        return Err(Error::Refused(Refusal::MalformedSource {
            line: extra.to_owned(),
        }));
    }
    identity.parse().map_err(|_| {
        Error::Refused(Refusal::MalformedSource {
            line: identity.to_owned(),
        })
    })
}

/// The tape operation an I/O failure was attempting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Opening or creating the tape directory.
    OpenTape,
    /// Taking the writer lock.
    Possess,
    /// Appending verbatim bytes.
    Append,
    /// Reading plaintext for a locus or a seal.
    ReadPlaintext,
    /// Removing plaintext whose member is sealed.
    RemovePlaintext,
    /// Reading a sealed gzip member.
    ReadSealed,
    /// Compressing a member.
    Compress,
    /// Staging bytes for publication.
    StagePublication,
    /// Reading the index.
    ReadIndex,
    /// Inspecting the current member extents.
    InspectExtents,
    /// Reading the durable source-generation identity.
    ReadSource,
    /// Synchronizing the tape directory.
    SyncDirectory,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OpenTape => "open tape directory",
            Self::Possess => "take the tape writer lock",
            Self::Append => "append verbatim bytes",
            Self::ReadPlaintext => "read plaintext",
            Self::RemovePlaintext => "remove sealed plaintext",
            Self::ReadSealed => "read sealed member",
            Self::Compress => "compress member",
            Self::StagePublication => "stage publication",
            Self::ReadIndex => "read index",
            Self::InspectExtents => "inspect tape member extents",
            Self::ReadSource => "read tape source identity",
            Self::SyncDirectory => "synchronize the tape directory",
        })
    }
}

/// A tape refusal: an answer about something tape declined to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// Another writer holds the tape.
    Contended,
    /// The writer lock name was rejected by the lock floor.
    LockNameRejected,
    /// The locus names a member this tape has never issued.
    UnknownMember {
        /// Member the locus named.
        member: u64,
    },
    /// The locus reaches past the end of its member.
    OutOfRange {
        /// Locus as presented.
        locus: Locus,
        /// Bytes the member actually holds.
        length: u64,
    },
    /// The index does not parse as the current format.
    MalformedIndex {
        /// The line that did not parse.
        line: String,
    },
    /// A bounded reader refused to load a larger index generation.
    IndexReadLimit {
        /// Maximum index bytes admitted by the caller.
        limit: u64,
        /// Bytes observed before reading the index.
        observed: u64,
    },
    /// A bounded reader refused to materialize another extent.
    ExtentReadLimit {
        /// Maximum extents admitted by the caller.
        limit: usize,
        /// First extent count outside that maximum.
        observed: usize,
        /// Exact index bytes read before the count was known.
        index_bytes: u64,
    },
    /// The source-generation identity is not the one form tape writes.
    MalformedSource {
        /// First non-current line, or the final line when framing is truncated.
        line: String,
    },
    /// A member was neither plaintext nor sealed across a full retry.
    MemberVanished {
        /// Member that could not be resolved.
        member: u64,
    },
    /// A range or its sealed member exceeds the admitted plaintext work.
    MemberReadLimit {
        /// Member whose read was refused.
        member: u64,
        /// Maximum plaintext bytes admitted for this read.
        limit: u64,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contended => formatter.write_str("another writer holds this tape"),
            Self::LockNameRejected => {
                formatter.write_str("the lock floor rejected the tape lock name")
            }
            Self::UnknownMember { member } => {
                write!(formatter, "this tape has not issued member {member}")
            }
            Self::OutOfRange { locus, length } => write!(
                formatter,
                "{locus} reaches past the {length} byte(s) member {} holds",
                locus.member
            ),
            Self::MalformedIndex { line } => {
                write!(
                    formatter,
                    "the index line {line:?} is not the current format"
                )
            }
            Self::IndexReadLimit { limit, observed } => write!(
                formatter,
                "the {observed}-byte tape index exceeds the {limit}-byte read limit"
            ),
            Self::ExtentReadLimit {
                limit,
                observed,
                index_bytes,
            } => write!(
                formatter,
                "tape extent {observed} exceeds the {limit}-extent read limit after {index_bytes} index bytes"
            ),
            Self::MalformedSource { line } => write!(
                formatter,
                "the source identity line {line:?} is not the current format"
            ),
            Self::MemberVanished { member } => write!(
                formatter,
                "member {member} was neither plaintext nor sealed after a full retry"
            ),
            Self::MemberReadLimit { member, limit } => write!(
                formatter,
                "reading tape member {member} exceeds the {limit}-byte plaintext limit"
            ),
        }
    }
}

/// A typed tape failure.
#[derive(Debug)]
pub enum Error {
    /// Tape declined; nothing was captured, sealed, or removed.
    Refused(Refusal),
    /// Publication through the filesystem primitive failed.
    Publish(Box<atomic::Error>),
    /// An I/O failure, named by the operation attempting it.
    Io {
        /// What tape was doing.
        operation: Operation,
        /// Path involved.
        path: PathBuf,
        /// Underlying failure.
        source: io::Error,
    },
}

impl Error {
    fn io(operation: Operation, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    /// The refusal this error carries, when it refused.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            Self::Publish(_) | Self::Io { .. } => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "tape refused: {refusal}"),
            Self::Publish(error) => write!(formatter, "tape publication failed: {error}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(_) => None,
            Self::Publish(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_ATTEMPTS: usize = 128;

    fn create_scratch_root(candidates: impl Iterator<Item = PathBuf>) -> PathBuf {
        for root in candidates.take(CREATE_ATTEMPTS) {
            match fs::create_dir(&root) {
                Ok(()) => return root,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create a scratch root at {}: {error}", root.display()),
            }
        }
        panic!("no scratch root was free within {CREATE_ATTEMPTS} attempts");
    }

    struct Scratch {
        root: PathBuf,
        removed: bool,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir();
            let process = std::process::id();
            let root = create_scratch_root(std::iter::repeat_with(|| {
                let unique = NEXT.fetch_add(1, Ordering::Relaxed);
                directory.join(format!("spice-tape-{label}-{process}-{unique}"))
            }));
            Self {
                root,
                removed: false,
            }
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn remove(mut self) {
            fs::remove_dir_all(&self.root).unwrap_or_else(|error| {
                panic!(
                    "remove the scratch root at {}: {error}",
                    self.root.display()
                )
            });
            self.assert_gone();
            self.removed = true;
        }

        fn assert_removed(mut self) {
            self.assert_gone();
            self.removed = true;
        }

        fn assert_gone(&self) {
            assert!(
                !self.root.exists(),
                "the scratch root outlived its removal at {}",
                self.root.display()
            );
        }
    }

    impl Drop for Scratch {
        /// Attempt cleanup during unwinding without replacing the first panic.
        fn drop(&mut self) {
            if !self.removed {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    /// Contention is tested here rather than beside the other fixtures because
    /// holding the writer lock without appending requires [`Tape::possess`],
    /// which is deliberately not part of the public surface.
    #[test]
    fn a_second_writer_is_refused_rather_than_left_to_wait() {
        let scratch = Scratch::new("contended");
        let tape = Tape::open(scratch.path()).expect("tape opens");

        let held = tape.possess().expect("the first writer takes the lock");

        assert_eq!(
            Tape::open(tape.root()).expect("a current tape reopens while its writer is active"),
            tape,
            "reopening a current generation does not compete with capture"
        );

        for refused in [
            tape.tee(b"blocked"),
            tape.seal().map(|_| Locus::new(0, 0, 0)),
        ] {
            let error = refused.expect_err("a second writer is refused");
            assert!(
                matches!(error.refusal(), Some(Refusal::Contended)),
                "got {error}"
            );
        }

        drop(held);
        tape.tee(b"admitted")
            .expect("the lock is released with its possession");

        scratch.remove();
    }

    /// A tee that cannot commit the member's name has not captured anything a
    /// locus could promise, so the failure is returned rather than assumed.
    #[test]
    fn a_directory_synchronization_failure_is_surfaced() {
        let scratch = Scratch::new("unsyncable");
        let tape = Tape::open(scratch.path()).expect("tape opens");
        tape.tee(b"captured").expect("the first append is captured");
        tape.sync_directory()
            .expect("an intact directory synchronizes");

        fs::remove_dir_all(scratch.path()).expect("the directory is removed underneath the tape");

        let error = tape
            .sync_directory()
            .expect_err("a directory that cannot be opened is not a success");
        assert!(
            matches!(
                error,
                Error::Io {
                    operation: Operation::SyncDirectory,
                    ..
                }
            ),
            "got {error}",
        );
        scratch.assert_removed();
    }

    #[test]
    fn every_staging_name_is_distinct_and_cannot_be_mistaken_for_a_member() {
        let first = temporary_name();
        let second = temporary_name();

        assert_ne!(first, second, "concurrent stagers must not collide");
        for name in [first, second] {
            assert!(name.starts_with('.'), "{name} is not hidden");
            assert!(name.ends_with(".tmp"), "{name} is not marked temporary");
            assert!(
                !name.ends_with(".gz") && !name.ends_with(".plain") && name != INDEX,
                "{name} could be mistaken for tape's own content"
            );
        }
    }
}
