//! The spice rack: everything an executable ships besides its code, as one
//! deterministic ZIP whose members are gzip streams stored with method STORE.
//!
//! Every member path begins with the [`Namespace`] that holds it: `app/` the
//! browser app generation, `skill/` the agent skill, `doc/` documentation, and
//! `tool/` tool members beside their manifests. [`Rack::write`] is the only
//! writer and [`Rack::admit`] the only reader. Every member is one gzip stream
//! under a fixed header, so a browser receives the stored bytes directly under
//! `Content-Encoding: gzip`. Admission inflates only the app shell;
//! [`RackMember::inflate`] yields any other member's original bytes on demand.
//! The archive bytes digest and the executable signature that carries them are
//! the integrity authority, and the ZIP CRC fields are structural evidence
//! compared between headers.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write as _;

use flate2::{Compression, Crc, Decompress, FlushDecompress, Status};

/// Most members one rack carries: every namespace's bound summed.
pub const MAX_MEMBERS: usize = Namespace::App.max_members()
    + Namespace::Doc.max_members()
    + Namespace::Skill.max_members()
    + Namespace::Tool.max_members();
/// Largest uncompressed size one member's gzip trailer may declare.
pub const MAX_MEMBER_BYTES: u32 = 64 * 1024 * 1024;
/// Largest uncompressed size all members may declare together.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// The member beside every tool's other members that describes the tool.
pub const TOOL_MANIFEST: &str = "manifest.json";

/// One rack namespace: the first segment of every member path it holds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Namespace {
    /// The browser app generation: exactly its shell's inventory.
    App,
    /// Tracked documentation.
    Doc,
    /// The agent skill.
    Skill,
    /// Tool members, each tool directory carrying its [`TOOL_MANIFEST`].
    Tool,
}

impl Namespace {
    /// Every namespace, in path order.
    pub const ALL: [Self; 4] = [Self::App, Self::Doc, Self::Skill, Self::Tool];

    /// The path segment naming this namespace.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Doc => "doc",
            Self::Skill => "skill",
            Self::Tool => "tool",
        }
    }

    /// The namespace a path segment names.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|namespace| namespace.name() == name)
    }

    /// Most members this namespace may hold, declared so that no namespace
    /// widens another's share.
    #[must_use]
    pub const fn max_members(self) -> usize {
        match self {
            Self::App => wire::manifest::APP_FILES.len() + wire::manifest::renderer::MAX_FILES,
            Self::Doc => 256,
            Self::Skill => 1,
            Self::Tool => 64,
        }
    }

    /// The member path of `file` within this namespace.
    #[must_use]
    pub fn member(self, file: &str) -> String {
        format!("{}/{file}", self.name())
    }
}

/// The media type a member path declares by its extension, which assembly
/// takes from the tracked source's name. Bytes are never sniffed: an unknown
/// extension declares opaque bytes.
#[must_use]
pub fn media_type(path: &str) -> &'static str {
    let file = path.rsplit('/').next().unwrap_or(path);
    let extension = file.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension.to_ascii_lowercase().as_str() {
        "md" => "text/markdown",
        "txt" => "text/plain",
        "html" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "toml" => "application/toml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Whether a member of `media_type` is read inline as text rather than
/// handed off as bytes.
#[must_use]
pub fn is_text(media_type: &str) -> bool {
    media_type.starts_with("text/") || matches!(media_type, "application/json" | "application/toml")
}

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;
/// The end-of-central-directory record, which carries no comment.
const END_RECORD_BYTES: usize = 22;
/// ZIP 1.0 made on and needing MS-DOS: all that reading STORE requires.
const ZIP_VERSION: u16 = 10;
const METHOD_STORE: u16 = 0;
/// 1980-01-01 00:00:00 as MS-DOS time and date fields.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1;
/// gzip magic, deflate, no flags, mtime 0, no extra flags, unknown OS.
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
/// The gzip trailer: CRC-32 and size of the uncompressed bytes.
const GZIP_TRAILER_BYTES: usize = 8;
const GZIP_CRC_BYTES: usize = 4;
const GZIP_LEVEL: u32 = 9;
/// The shell attribute naming the generation its routes are qualified with.
const SHELL_GENERATION: &str = "data-app-generation=\"";

/// One admitted rack whose members borrow the archive bytes.
#[derive(Debug)]
pub struct Rack<'a> {
    generation: wire::Digest,
    shell: Box<[u8]>,
    members: BTreeMap<&'a str, RackMember<'a>>,
}

/// One stored gzip member of an admitted rack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RackMember<'a> {
    gzip: &'a [u8],
    size: u32,
}

/// One member's content as [`Rack::write`] stores it.
#[derive(Clone, Copy, Debug)]
pub enum RackInput<'a> {
    /// Original bytes, deflated into the member's gzip stream.
    Bytes(&'a [u8]),
    /// A member of an admitted rack, whose gzip stream is copied unchanged:
    /// deflation is deterministic, so this is the stream its bytes would give.
    Stored(RackMember<'a>),
}

impl<'a> From<&'a [u8]> for RackInput<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a> From<RackMember<'a>> for RackInput<'a> {
    fn from(member: RackMember<'a>) -> Self {
        Self::Stored(member)
    }
}

impl<'a> RackMember<'a> {
    /// The exact stored gzip stream, served under `Content-Encoding: gzip`.
    #[must_use]
    pub const fn gzip(self) -> &'a [u8] {
        self.gzip
    }

    /// Uncompressed size the member's gzip trailer declares.
    #[must_use]
    pub const fn size(self) -> u32 {
        self.size
    }

    /// Inflate this member, stored at `path`, requiring exactly one gzip
    /// stream of its declared size and CRC.
    ///
    /// # Errors
    ///
    /// Refuses, naming `path`, bytes that are not exactly that stream.
    pub fn inflate(self, path: &str) -> Result<Box<[u8]>, RackError> {
        let refused = || member_error(path, "is not exactly one gzip stream");
        let trailer = self.gzip.len() - GZIP_TRAILER_BYTES;
        let body = &self.gzip[GZIP_HEADER.len()..trailer];
        let mut inflated = Vec::with_capacity(self.size as usize);
        let mut inflater = Decompress::new(false);
        let status = inflater
            .decompress_vec(body, &mut inflated, FlushDecompress::Finish)
            .map_err(|_| refused())?;
        let mut crc = Crc::new();
        crc.update(&inflated);
        let declared = u32::from_le_bytes(
            self.gzip[trailer..trailer + GZIP_CRC_BYTES]
                .try_into()
                .map_err(|_| TRUNCATED)?,
        );
        if status != Status::StreamEnd
            || inflater.total_in() != body.len() as u64
            || inflated.len() != self.size as usize
            || crc.sum() != declared
        {
            return Err(refused());
        }
        Ok(inflated.into_boxed_slice())
    }
}

impl<'a> Rack<'a> {
    /// Write the deterministic rack of these members.
    ///
    /// Members are ordered by path bytes whatever the input order, so the same
    /// members always yield the same archive bytes.
    ///
    /// # Errors
    ///
    /// Refuses an invalid or repeated path, a path outside every namespace, a
    /// tool member without its manifest, and a member, namespace, total, or
    /// archive above its bound.
    pub fn write<'f, I: Into<RackInput<'f>>>(
        files: impl IntoIterator<Item = (&'f str, I)>,
    ) -> Result<Vec<u8>, RackError> {
        let mut files = files
            .into_iter()
            .map(|(path, input)| (path, input.into()))
            .collect::<Vec<_>>();
        let count = u16::try_from(files.len())
            .ok()
            .filter(|count| usize::from(*count) <= MAX_MEMBERS)
            .ok_or(RackError::Bound("archive exceeds its member bound"))?;
        files.sort_unstable_by_key(|(path, _)| *path);
        if let Some(pair) = files.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(member_error(pair[0].0, "path is repeated"));
        }
        for (path, _) in &files {
            check_path(path)?;
        }
        check_namespaces(files.iter().map(|(path, _)| *path))?;
        let mut archive = Vec::new();
        let mut directory = Vec::new();
        let mut total = 0_u64;
        for (path, input) in files {
            let (stored, size) = match input {
                RackInput::Bytes(bytes) => {
                    let size = u32::try_from(bytes.len())
                        .ok()
                        .filter(|size| *size <= MAX_MEMBER_BYTES)
                        .ok_or_else(|| member_error(path, "exceeds the member byte bound"))?;
                    (Cow::Owned(gzip(bytes)), size)
                }
                RackInput::Stored(member) => (Cow::Borrowed(member.gzip), member.size),
            };
            total += u64::from(size);
            if total > MAX_TOTAL_BYTES {
                return Err(RackError::Bound("members exceed the total byte bound"));
            }
            let mut crc = Crc::new();
            crc.update(&stored);
            let fields = Fields::stored(path, crc.sum(), &stored)?;
            let offset = archive_offset(archive.len())?;
            put_u32(&mut archive, LOCAL_SIGNATURE);
            fields.put(&mut archive);
            archive.extend_from_slice(path.as_bytes());
            archive.extend_from_slice(&stored);
            put_u32(&mut directory, CENTRAL_SIGNATURE);
            put_u16(&mut directory, ZIP_VERSION);
            fields.put(&mut directory);
            CentralTail::canonical(offset).put(&mut directory);
            directory.extend_from_slice(path.as_bytes());
        }
        let end = EndRecord {
            count,
            size: archive_offset(directory.len())?,
            offset: archive_offset(archive.len())?,
        };
        archive.extend_from_slice(&directory);
        end.put(&mut archive);
        Ok(archive)
    }

    /// Admit one rack, deriving its generation from the archived app shell.
    ///
    /// # Errors
    ///
    /// Refuses every structure, member, namespace, bound, and shell violation
    /// the module documentation and [`RackError`] name.
    pub fn admit(bytes: &'a [u8]) -> Result<Self, RackError> {
        let end = EndRecord::read(bytes)?;
        let mut walk = Walk {
            bytes,
            central: Cursor::at(bytes, end.offset as usize),
            local: 0,
            directory: end.offset as usize,
        };
        let mut members = BTreeMap::new();
        let mut total = 0_u64;
        let mut previous = None;
        for _ in 0..end.count {
            let (path, member) = walk.member()?;
            if previous.is_some_and(|previous| previous >= path) {
                return Err(member_error(path, "repeats or breaks path order"));
            }
            total += u64::from(member.size);
            if total > MAX_TOTAL_BYTES {
                return Err(RackError::Bound("members exceed the total byte bound"));
            }
            previous = Some(path);
            members.insert(path, member);
        }
        if walk.central.at != bytes.len() - END_RECORD_BYTES || walk.local != walk.directory {
            return Err(RackError::Structure(
                "bytes lie outside every member and directory record",
            ));
        }
        check_namespaces(members.keys().copied())?;
        let shell_path = Namespace::App.member(wire::manifest::APP_HTML);
        let shell = members
            .get(shell_path.as_str())
            .copied()
            .ok_or(RackError::Shell("archive carries no app shell"))
            .and_then(|shell| shell.inflate(&shell_path))?;
        let generation = shell_generation(&shell)?;
        check_inventory(&shell, &members)?;
        Ok(Self {
            generation,
            shell,
            members,
        })
    }

    /// Admit one rack whose app shell must name `expected`.
    ///
    /// # Errors
    ///
    /// Refuses everything [`Self::admit`] refuses and a shell naming another
    /// generation.
    pub fn admit_generation(bytes: &'a [u8], expected: &wire::Digest) -> Result<Self, RackError> {
        let archive = Self::admit(bytes)?;
        if archive.generation != *expected {
            return Err(RackError::Generation {
                expected: expected.clone(),
                found: archive.generation,
            });
        }
        Ok(archive)
    }

    /// The generation the archived app shell qualifies its routes with.
    #[must_use]
    pub const fn generation(&self) -> &wire::Digest {
        &self.generation
    }

    /// The inflated app shell, read once at admission.
    #[must_use]
    pub fn shell(&self) -> &[u8] {
        &self.shell
    }

    /// The stored member at one namespaced path.
    #[must_use]
    pub fn member(&self, path: &str) -> Option<RackMember<'a>> {
        self.members.get(path).copied()
    }

    /// Every stored member in path order.
    pub fn members(&self) -> impl Iterator<Item = (&'a str, RackMember<'a>)> + '_ {
        self.members.iter().map(|(path, member)| (*path, *member))
    }

    /// The members of `namespace`, keyed by their path within it, in order.
    pub fn namespace(
        &self,
        namespace: Namespace,
    ) -> impl Iterator<Item = (&'a str, RackMember<'a>)> + '_ {
        let prefix = Namespace::member(namespace, "");
        self.members
            .range::<str, _>((
                std::ops::Bound::Included(prefix.as_str()),
                std::ops::Bound::Unbounded,
            ))
            .map_while(move |(path, member)| {
                path.strip_prefix(prefix.as_str())
                    .map(|file| (file, *member))
            })
    }
}

/// Why a rack cannot be written or admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RackError {
    /// The end record or central directory disagrees with the archive bytes.
    Structure(&'static str),
    /// One member violates the format; `path` is its stored spelling.
    Member { path: String, reason: &'static str },
    /// The members exceed a declared bound.
    Bound(&'static str),
    /// The archived app shell does not declare an admissible generation.
    Shell(&'static str),
    /// The shell qualifies its routes with another generation.
    Generation {
        expected: wire::Digest,
        found: wire::Digest,
    },
}

impl fmt::Display for RackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Structure(reason) | Self::Bound(reason) | Self::Shell(reason) => {
                write!(formatter, "spice rack refused: {reason}")
            }
            Self::Member { path, reason } => {
                write!(formatter, "spice rack member {path:?} refused: {reason}")
            }
            Self::Generation { expected, found } => write!(
                formatter,
                "spice rack app shell names generation {found}, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for RackError {}

fn member_error(path: &str, reason: &'static str) -> RackError {
    RackError::Member {
        path: path.to_owned(),
        reason,
    }
}

const TRUNCATED: RackError = RackError::Structure("an archive record is truncated");

/// The header run local and central entries share, in field order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fields {
    needed: u16,
    flags: u16,
    method: u16,
    time: u16,
    date: u16,
    crc: u32,
    stored: u32,
    size: u32,
    name: u16,
    extra: u16,
}

impl Fields {
    fn stored(path: &str, crc: u32, stored: &[u8]) -> Result<Self, RackError> {
        let length = u32::try_from(stored.len())
            .map_err(|_| member_error(path, "exceeds the archive offset range"))?;
        Ok(Self {
            needed: ZIP_VERSION,
            flags: 0,
            method: METHOD_STORE,
            time: DOS_TIME,
            date: DOS_DATE,
            crc,
            stored: length,
            size: length,
            name: u16::try_from(path.len())
                .map_err(|_| member_error(path, "path exceeds the header length range"))?,
            extra: 0,
        })
    }

    fn read(cursor: &mut Cursor<'_>) -> Result<Self, RackError> {
        Ok(Self {
            needed: cursor.u16()?,
            flags: cursor.u16()?,
            method: cursor.u16()?,
            time: cursor.u16()?,
            date: cursor.u16()?,
            crc: cursor.u32()?,
            stored: cursor.u32()?,
            size: cursor.u32()?,
            name: cursor.u16()?,
            extra: cursor.u16()?,
        })
    }

    fn put(self, out: &mut Vec<u8>) {
        for value in [self.needed, self.flags, self.method, self.time, self.date] {
            put_u16(out, value);
        }
        for value in [self.crc, self.stored, self.size] {
            put_u32(out, value);
        }
        put_u16(out, self.name);
        put_u16(out, self.extra);
    }

    const fn canonical(self) -> bool {
        self.needed == ZIP_VERSION
            && self.flags == 0
            && self.time == DOS_TIME
            && self.date == DOS_DATE
            && self.size == self.stored
            && self.extra == 0
    }
}

/// The central entry fields that follow the shared run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CentralTail {
    comment: u16,
    disk: u16,
    internal: u16,
    external: u32,
    offset: u32,
}

impl CentralTail {
    const fn canonical(offset: u32) -> Self {
        Self {
            comment: 0,
            disk: 0,
            internal: 0,
            external: 0,
            offset,
        }
    }

    fn read(cursor: &mut Cursor<'_>) -> Result<Self, RackError> {
        Ok(Self {
            comment: cursor.u16()?,
            disk: cursor.u16()?,
            internal: cursor.u16()?,
            external: cursor.u32()?,
            offset: cursor.u32()?,
        })
    }

    fn put(self, out: &mut Vec<u8>) {
        for value in [self.comment, self.disk, self.internal] {
            put_u16(out, value);
        }
        put_u32(out, self.external);
        put_u32(out, self.offset);
    }
}

/// The end-of-central-directory record of a single-disk archive.
struct EndRecord {
    count: u16,
    size: u32,
    offset: u32,
}

impl EndRecord {
    fn read(bytes: &[u8]) -> Result<Self, RackError> {
        let start = bytes
            .len()
            .checked_sub(END_RECORD_BYTES)
            .ok_or(RackError::Structure(
                "archive is shorter than its end record",
            ))?;
        let mut cursor = Cursor::at(bytes, start);
        let signature = cursor.u32()?;
        let disks = [cursor.u16()?, cursor.u16()?];
        let (here, count) = (cursor.u16()?, cursor.u16()?);
        let end = Self {
            count,
            size: cursor.u32()?,
            offset: cursor.u32()?,
        };
        let comment = cursor.u16()?;
        if signature != END_SIGNATURE || disks != [0, 0] || here != count || comment != 0 {
            return Err(RackError::Structure(
                "end record is absent or not a canonical single-disk record",
            ));
        }
        if end.offset as usize + end.size as usize != start {
            return Err(RackError::Structure(
                "central directory does not end at the end record",
            ));
        }
        if usize::from(end.count) > MAX_MEMBERS {
            return Err(RackError::Bound("archive exceeds its member bound"));
        }
        Ok(end)
    }

    fn put(&self, out: &mut Vec<u8>) {
        put_u32(out, END_SIGNATURE);
        for value in [0, 0, self.count, self.count] {
            put_u16(out, value);
        }
        put_u32(out, self.size);
        put_u32(out, self.offset);
        put_u16(out, 0);
    }
}

/// Central directory entries walked against contiguous local members.
struct Walk<'a> {
    bytes: &'a [u8],
    central: Cursor<'a>,
    local: usize,
    directory: usize,
}

impl<'a> Walk<'a> {
    fn member(&mut self) -> Result<(&'a str, RackMember<'a>), RackError> {
        if self.central.u32()? != CENTRAL_SIGNATURE {
            return Err(RackError::Structure(
                "a central directory entry has no signature",
            ));
        }
        let made_by = self.central.u16()?;
        let fields = Fields::read(&mut self.central)?;
        let tail = CentralTail::read(&mut self.central)?;
        let name = self.central.take(usize::from(fields.name))?;
        let path = admitted_path(name)?;
        if made_by != ZIP_VERSION
            || !fields.canonical()
            || tail != CentralTail::canonical(tail.offset)
        {
            return Err(member_error(
                path,
                "directory entry carries noncanonical fields",
            ));
        }
        if fields.method != METHOD_STORE {
            return Err(member_error(path, "method is not STORE"));
        }
        if tail.offset as usize != self.local {
            return Err(member_error(
                path,
                "member is not contiguous with its predecessor",
            ));
        }
        let mut local = Cursor::at(self.bytes, self.local);
        let signature = local.u32()?;
        let local_fields = Fields::read(&mut local)?;
        if signature != LOCAL_SIGNATURE
            || local_fields != fields
            || local.take(usize::from(fields.name))? != name
        {
            return Err(member_error(path, "local and central headers disagree"));
        }
        let gzip = local.take(fields.stored as usize)?;
        if local.at > self.directory {
            return Err(member_error(
                path,
                "stored bytes overlap the central directory",
            ));
        }
        self.local = local.at;
        let size = gzip_size(path, gzip)?;
        Ok((path, RackMember { gzip, size }))
    }
}

/// A little-endian reader that refuses reads past the archive.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn at(bytes: &'a [u8], at: usize) -> Self {
        Self { bytes, at }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], RackError> {
        let end = self.at.checked_add(count).ok_or(TRUNCATED)?;
        let slice = self.bytes.get(self.at..end).ok_or(TRUNCATED)?;
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, RackError> {
        let bytes = self.take(size_of::<u16>())?;
        Ok(u16::from_le_bytes(bytes.try_into().map_err(|_| TRUNCATED)?))
    }

    fn u32(&mut self) -> Result<u32, RackError> {
        let bytes = self.take(size_of::<u32>())?;
        Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| TRUNCATED)?))
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn archive_offset(length: usize) -> Result<u32, RackError> {
    u32::try_from(length).map_err(|_| RackError::Bound("archive exceeds its offset range"))
}

/// A normalized relative logical path: slash-separated, no empty, dot,
/// dot-dot, backslash, or NUL segment.
fn check_path(path: &str) -> Result<(), RackError> {
    if path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(member_error(
            path,
            "path is not a normalized relative logical path",
        ));
    }
    Ok(())
}

fn admitted_path(name: &[u8]) -> Result<&str, RackError> {
    let path = std::str::from_utf8(name)
        .map_err(|_| member_error(&String::from_utf8_lossy(name), "path is not UTF-8"))?;
    check_path(path)?;
    Ok(path)
}

/// One deterministic gzip stream: the fixed header, raw deflate, and trailer.
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::from(GZIP_HEADER), Compression::new(GZIP_LEVEL));
    encoder
        .write_all(bytes)
        .expect("deflating into memory cannot fail");
    let mut stream = encoder.finish().expect("deflating into memory cannot fail");
    let mut crc = Crc::new();
    crc.update(bytes);
    put_u32(&mut stream, crc.sum());
    put_u32(&mut stream, crc.amount());
    stream
}

/// The declared size of a stored gzip member under the fixed framing.
fn gzip_size(path: &str, gzip: &[u8]) -> Result<u32, RackError> {
    let trailer = gzip
        .len()
        .checked_sub(GZIP_TRAILER_BYTES)
        .filter(|trailer| *trailer > GZIP_HEADER.len())
        .ok_or_else(|| member_error(path, "stored bytes are too short to be one gzip stream"))?;
    if gzip[..GZIP_HEADER.len()] != GZIP_HEADER {
        return Err(member_error(path, "gzip header is not the fixed header"));
    }
    let size = u32::from_le_bytes(
        gzip[trailer + GZIP_CRC_BYTES..]
            .try_into()
            .map_err(|_| TRUNCATED)?,
    );
    if size > MAX_MEMBER_BYTES {
        return Err(member_error(
            path,
            "declares more than the member byte bound",
        ));
    }
    Ok(size)
}

fn shell_generation(shell: &[u8]) -> Result<wire::Digest, RackError> {
    let html = std::str::from_utf8(shell).map_err(|_| RackError::Shell("shell is not UTF-8"))?;
    let mut spans = html.split(SHELL_GENERATION).skip(1);
    let value = spans
        .next()
        .ok_or(RackError::Shell("shell names no generation"))?;
    if spans.next().is_some() {
        return Err(RackError::Shell("shell names more than one generation"));
    }
    value
        .split_once('"')
        .and_then(|(generation, _)| generation.parse().ok())
        .ok_or(RackError::Shell("shell generation is not a digest"))
}

/// Every path lies inside one namespace and within its member bound, and every
/// tool directory carries its manifest.
fn check_namespaces<'p>(paths: impl IntoIterator<Item = &'p str>) -> Result<(), RackError> {
    let mut counts = BTreeMap::<Namespace, usize>::new();
    let mut tools = BTreeMap::<&str, bool>::new();
    for path in paths {
        let (namespace, file) = path
            .split_once('/')
            .and_then(|(name, file)| Some((Namespace::parse(name)?, file)))
            .ok_or_else(|| member_error(path, "lies outside every rack namespace"))?;
        let count = counts.entry(namespace).or_default();
        *count += 1;
        if *count > namespace.max_members() {
            return Err(member_error(path, "exceeds its namespace member bound"));
        }
        if namespace == Namespace::Tool {
            let (tool, rest) = file
                .split_once('/')
                .ok_or_else(|| member_error(path, "lies outside every tool directory"))?;
            *tools.entry(tool).or_default() |= rest == TOOL_MANIFEST;
        }
    }
    match tools.into_iter().find(|(_, manifest)| !manifest) {
        Some((tool, _)) => Err(member_error(
            &Namespace::Tool.member(tool),
            "carries no tool manifest",
        )),
        None => Ok(()),
    }
}

/// The app members are exactly the shell inventory and its declared renderers.
fn check_inventory(
    shell: &[u8],
    members: &BTreeMap<&str, RackMember<'_>>,
) -> Result<(), RackError> {
    let declared = wire::manifest::renderer::from_html(shell).map_err(RackError::Shell)?;
    let expected = wire::manifest::renderer::filenames(&declared)
        .map(|file| Namespace::App.member(&file))
        .collect::<BTreeSet<String>>();
    let prefix = Namespace::App.member("");
    if let Some(path) = members
        .keys()
        .filter(|path| path.starts_with(prefix.as_str()))
        .find(|path| !expected.contains(**path))
    {
        return Err(member_error(path, "is outside the shell inventory"));
    }
    if let Some(path) = expected
        .iter()
        .find(|path| !members.contains_key(path.as_str()))
    {
        return Err(member_error(path, "is declared by the shell but absent"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
