use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rustix::fs::{self as unix_fs, AtFlags, Dir, Mode, OFlags, RenameFlags};
use rustix::process::geteuid;
use serde::{Deserialize, Serialize};
use sill::{LockFile, LockName, Possession};
use wire::{Digest, Domain};

use super::{FileImage, GeneratedImage, InstallError, Repository};

const FORMAT_VERSION: u16 = 1;
const MAGIC: &[u8] = b"spice-install-rollback\0";
const MAX_UNCOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_STORED_BYTES: u64 = MAX_UNCOMPRESSED_BYTES + 64 * 1024 * 1024;
const MAX_INLINE_PAYLOAD_CHARS: u64 = MAX_STORED_BYTES.div_ceil(3) * 4;
const MAX_ARCHIVE_IMAGE_COUNT: u32 = 4096;
const BLOB_MODE: u32 = 0o600;
const BLOB_DIRECTORY_MODE: u32 = 0o700;
const BLOB_LINK_COUNT: u64 = 1;
const DIGEST_HEX_CHARACTERS: usize = 64;
const BLOB_SUFFIX: &str = ".gz";
const LOCK_NAME: &str = "install-rollback";
const LOCK_FILE: &str = "install-rollback.lock";
const RECLAIM_ENTRY_LIMIT: usize = 4096;
const RECLAIM_CANDIDATE_LIMIT: usize = 32;
const RECLAIM_GRACE: Duration = Duration::from_secs(60 * 60);
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub(super) struct RollbackReclamation {
    pub(super) examined_entries: usize,
    pub(super) omitted_entries: usize,
    pub(super) omitted_candidates: usize,
    pub(super) reclaimable_count: usize,
    pub(super) reclaimable_bytes: u64,
    pub(super) unsafe_candidates: Vec<String>,
}

pub(super) struct RollbackLock {
    _possession: Possession,
}

pub(super) fn acquire_lock(repository: &Repository) -> Result<RollbackLock, InstallError> {
    let state = repository.admit_state()?;
    match fs::create_dir(state.locks()) {
        Ok(()) => {
            fs::set_permissions(
                state.locks(),
                fs::Permissions::from_mode(BLOB_DIRECTORY_MODE),
            )
            .map_err(|source| {
                InstallError::io("set install lock directory mode", state.locks(), source)
            })?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => {
            return Err(InstallError::io(
                "create install lock directory",
                state.locks(),
                source,
            ));
        }
    }
    let path = state.locks().join(LOCK_FILE);
    let name = LockName::new(LOCK_NAME).expect("install rollback is a valid lock name");
    let possession = LockFile::new(name, &path)
        .acquire()
        .map_err(|source| InstallError::io("acquire install rollback lock", path, source))?;
    Ok(RollbackLock {
        _possession: possession,
    })
}

pub(super) fn rollback_image_matches(
    image: Option<&FileImage>,
    superseded: Option<&GeneratedImage>,
) -> bool {
    match (image, superseded) {
        (None, None) => true,
        (Some(image), Some(superseded)) => image.validate() && image.generated() == *superseded,
        _ => false,
    }
}

/// A compressed, digest-bound snapshot used only to reverse a version-7
/// migration. Keeping large executable images out of the JSON object graph
/// prevents every progress receipt from expanding each byte into a decimal
/// array (and then duplicating the operation reversal inventory).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum RollbackArchive {
    Inline(InlineRollbackArchive),
    Blob(RollbackBlob),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InlineRollbackArchive {
    format: u16,
    uncompressed_bytes: u64,
    digest: Digest,
    payload: String,
    #[serde(skip)]
    decoded: OnceLock<Arc<RollbackState>>,
}

impl PartialEq for InlineRollbackArchive {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && self.uncompressed_bytes == other.uncompressed_bytes
            && self.digest == other.digest
            && self.payload == other.payload
    }
}

impl Eq for InlineRollbackArchive {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RollbackBlob {
    format: u16,
    stored_bytes: u64,
    stored_digest: Digest,
    uncompressed_bytes: u64,
    digest: Digest,
    #[serde(skip)]
    staged: Option<StagedRollbackBlob>,
}

#[derive(Clone, Debug)]
struct StagedRollbackBlob {
    bytes: Arc<[u8]>,
    state: Option<Arc<RollbackState>>,
}

impl PartialEq for RollbackBlob {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && self.stored_bytes == other.stored_bytes
            && self.stored_digest == other.stored_digest
            && self.uncompressed_bytes == other.uncompressed_bytes
            && self.digest == other.digest
    }
}

impl Eq for RollbackBlob {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RollbackState {
    pub(super) record: FileImage,
    pub(super) operations: Vec<Option<FileImage>>,
    pub(super) retirements: Vec<Option<FileImage>>,
}

impl RollbackArchive {
    pub(super) const fn format(&self) -> u16 {
        match self {
            Self::Inline(archive) => archive.format,
            Self::Blob(archive) => archive.format,
        }
    }

    pub(super) const fn uncompressed_bytes(&self) -> u64 {
        match self {
            Self::Inline(archive) => archive.uncompressed_bytes,
            Self::Blob(archive) => archive.uncompressed_bytes,
        }
    }

    pub(super) const fn digest(&self) -> &Digest {
        match self {
            Self::Inline(archive) => &archive.digest,
            Self::Blob(archive) => &archive.digest,
        }
    }

    pub(super) const fn is_inline(&self) -> bool {
        matches!(self, Self::Inline(_))
    }

    pub(super) const fn stored_digest(&self) -> Option<&Digest> {
        match self {
            Self::Inline(_) => None,
            Self::Blob(blob) => Some(&blob.stored_digest),
        }
    }

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
        Self::blob_from_compressed(
            FORMAT_VERSION,
            uncompressed_bytes,
            digest,
            compressed,
            Some(Arc::new(state.clone())),
        )
    }

    fn blob_from_compressed(
        format: u16,
        uncompressed_bytes: u64,
        digest: Digest,
        compressed: Vec<u8>,
        state: Option<Arc<RollbackState>>,
    ) -> Result<Self, InstallError> {
        let stored_bytes = u64::try_from(compressed.len())
            .map_err(|_| invalid("migration rollback blob is too large for its manifest"))?;
        if stored_bytes > MAX_STORED_BYTES {
            return Err(invalid(
                "migration rollback blob exceeds the supported stored size",
            ));
        }
        let stored_digest = Digest::compute(Domain::InstallArtifact, &compressed);
        Ok(Self::Blob(RollbackBlob {
            format,
            stored_bytes,
            stored_digest,
            uncompressed_bytes,
            digest,
            staged: Some(StagedRollbackBlob {
                bytes: Arc::from(compressed),
                state,
            }),
        }))
    }

    pub(super) fn to_current(&self) -> Result<Self, InstallError> {
        match self {
            Self::Blob(blob) => Ok(Self::Blob(blob.clone())),
            Self::Inline(inline) => {
                let state = inline.cached_state()?;
                let compressed = inline.compressed()?;
                Self::blob_from_compressed(
                    inline.format,
                    inline.uncompressed_bytes,
                    inline.digest.clone(),
                    compressed,
                    Some(state),
                )
            }
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn to_inline(&self, repository: &Repository) -> Result<Self, InstallError> {
        match self {
            Self::Inline(inline) => Ok(Self::Inline(inline.clone())),
            Self::Blob(blob) => {
                let compressed = match &blob.staged {
                    Some(staged) => staged.bytes.to_vec(),
                    None => blob.read_and_validate(repository)?,
                };
                blob.validate_compressed(&compressed)?;
                Ok(Self::Inline(InlineRollbackArchive {
                    format: blob.format,
                    uncompressed_bytes: blob.uncompressed_bytes,
                    digest: blob.digest.clone(),
                    payload: STANDARD.encode(compressed),
                    decoded: OnceLock::new(),
                }))
            }
        }
    }

    pub(super) fn validate_structure(&self) -> Result<(), InstallError> {
        match self {
            Self::Inline(inline) => inline.validate_structure(),
            Self::Blob(blob) => blob.validate_structure(),
        }
    }

    pub(super) fn staged_state(&self) -> Result<Option<RollbackState>, InstallError> {
        match self {
            Self::Inline(inline) => inline.decode().map(Some),
            Self::Blob(blob) => match &blob.staged {
                Some(staged) => staged
                    .state
                    .as_deref()
                    .cloned()
                    .map_or_else(|| blob.decode_compressed(&staged.bytes), Ok)
                    .map(Some),
                None => Ok(None),
            },
        }
    }

    pub(super) fn decode(&self, repository: &Repository) -> Result<RollbackState, InstallError> {
        match self {
            Self::Inline(inline) => inline.decode(),
            Self::Blob(blob) => {
                if let Some(staged) = &blob.staged {
                    staged
                        .state
                        .as_deref()
                        .cloned()
                        .map_or_else(|| blob.decode_compressed(&staged.bytes), Ok)
                } else {
                    let compressed = blob.read_and_validate(repository)?;
                    blob.decode_compressed(&compressed)
                }
            }
        }
    }

    pub(super) fn validate_stored(&self, repository: &Repository) -> Result<(), InstallError> {
        match self {
            Self::Inline(_) => Ok(()),
            Self::Blob(blob) => match &blob.staged {
                Some(staged) => blob.validate_compressed(&staged.bytes),
                None => blob.read_and_validate(repository).map(|_| ()),
            },
        }
    }

    pub(super) fn publish_staged(&self, repository: &Repository) -> Result<(), InstallError> {
        let Self::Blob(blob) = self else {
            return Ok(());
        };
        let Some(staged) = &blob.staged else {
            return blob.read_and_validate(repository).map(|_| ());
        };
        blob.validate_compressed(&staged.bytes)?;
        publish_blob(repository, blob, &staged.bytes)
    }
}

impl InlineRollbackArchive {
    fn validate_structure(&self) -> Result<(), InstallError> {
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
        Ok(())
    }

    fn compressed(&self) -> Result<Vec<u8>, InstallError> {
        self.validate_structure()?;
        if u64::try_from(self.payload.len())
            .ok()
            .is_none_or(|length| length > MAX_INLINE_PAYLOAD_CHARS)
        {
            return Err(invalid(
                "migration rollback archive payload exceeds the supported stored size",
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
        if u64::try_from(compressed.len())
            .ok()
            .is_none_or(|size| size > MAX_STORED_BYTES)
        {
            return Err(invalid(
                "migration rollback archive exceeds the supported stored size",
            ));
        }
        Ok(compressed)
    }

    fn decode(&self) -> Result<RollbackState, InstallError> {
        Ok(self.cached_state()?.as_ref().clone())
    }

    fn cached_state(&self) -> Result<Arc<RollbackState>, InstallError> {
        if let Some(state) = self.decoded.get() {
            return Ok(Arc::clone(state));
        }
        let compressed = self.compressed()?;
        let state = Arc::new(decode_compressed(
            self.format,
            self.uncompressed_bytes,
            &self.digest,
            &compressed,
        )?);
        let _ = self.decoded.set(Arc::clone(&state));
        Ok(state)
    }
}

impl RollbackBlob {
    fn validate_structure(&self) -> Result<(), InstallError> {
        if self.format != FORMAT_VERSION {
            return Err(invalid("migration rollback blob has an unsupported format"));
        }
        if self.stored_bytes > MAX_STORED_BYTES {
            return Err(invalid(
                "migration rollback blob exceeds the supported stored size",
            ));
        }
        if self.uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(invalid(
                "migration rollback blob exceeds the supported uncompressed size",
            ));
        }
        Ok(())
    }

    fn validate_compressed(&self, compressed: &[u8]) -> Result<(), InstallError> {
        self.validate_structure()?;
        if u64::try_from(compressed.len()).ok() != Some(self.stored_bytes) {
            return Err(invalid(
                "migration rollback blob length does not match its manifest",
            ));
        }
        if Digest::compute(Domain::InstallArtifact, compressed) != self.stored_digest {
            return Err(invalid(
                "migration rollback blob digest does not match its manifest",
            ));
        }
        Ok(())
    }

    fn decode_compressed(&self, compressed: &[u8]) -> Result<RollbackState, InstallError> {
        self.validate_compressed(compressed)?;
        decode_compressed(
            self.format,
            self.uncompressed_bytes,
            &self.digest,
            compressed,
        )
    }

    fn read_and_validate(&self, repository: &Repository) -> Result<Vec<u8>, InstallError> {
        self.validate_structure()?;
        let directory = open_blob_directory(repository, false)?
            .ok_or_else(|| invalid("migration rollback blob directory is absent"))?;
        let name = blob_name(&self.stored_digest);
        let path = blob_path(repository, &name)?;
        let descriptor = unix_fs::openat(
            &directory,
            OsStr::new(&name),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|source| InstallError::io("open migration rollback blob", &path, source.into()))?;
        self.read_file_and_validate(File::from(descriptor), &path)
    }

    fn read_file_and_validate(&self, mut file: File, path: &Path) -> Result<Vec<u8>, InstallError> {
        validate_blob_file(&file, path, self.stored_bytes)?;
        let capacity = usize::try_from(self.stored_bytes)
            .map_err(|_| invalid("migration rollback blob length does not fit memory"))?;
        let mut compressed = Vec::with_capacity(capacity);
        Read::by_ref(&mut file)
            .take(self.stored_bytes.saturating_add(1))
            .read_to_end(&mut compressed)
            .map_err(|source| InstallError::io("read migration rollback blob", path, source))?;
        self.validate_compressed(&compressed)?;
        Ok(compressed)
    }
}

fn decode_compressed(
    format: u16,
    uncompressed_bytes: u64,
    digest: &Digest,
    compressed: &[u8],
) -> Result<RollbackState, InstallError> {
    if format != FORMAT_VERSION {
        return Err(invalid(
            "migration rollback archive has an unsupported format",
        ));
    }
    if uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
        return Err(invalid(
            "migration rollback archive exceeds the supported uncompressed size",
        ));
    }
    let mut decoder = GzDecoder::new(Cursor::new(compressed));
    let mut bytes = Vec::new();
    Read::by_ref(&mut decoder)
        .take(uncompressed_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("migration rollback archive did not decompress"))?;
    if u64::try_from(bytes.len()).ok() != Some(uncompressed_bytes) {
        return Err(invalid(
            "migration rollback archive length does not match its manifest",
        ));
    }
    if Digest::compute(Domain::InstallArtifact, &bytes) != *digest {
        return Err(invalid(
            "migration rollback archive digest does not match its payload",
        ));
    }
    decode_state(&bytes)
}

fn blob_name(digest: &Digest) -> String {
    format!("{}{BLOB_SUFFIX}", digest.as_str())
}

pub(super) fn reclaim_unreferenced(
    repository: &Repository,
    protected: Option<&Digest>,
    now: SystemTime,
) -> Result<RollbackReclamation, InstallError> {
    scan_unreferenced(repository, protected, now, true)
}

pub(super) fn inspect_unreferenced(
    repository: &Repository,
    protected: Option<&Digest>,
    now: SystemTime,
) -> Result<RollbackReclamation, InstallError> {
    scan_unreferenced(repository, protected, now, false)
}

fn scan_unreferenced(
    repository: &Repository,
    protected: Option<&Digest>,
    now: SystemTime,
    reclaim: bool,
) -> Result<RollbackReclamation, InstallError> {
    let Some(directory) = open_blob_directory(repository, false)? else {
        return Ok(RollbackReclamation::default());
    };
    let path = repository
        .admit_state()?
        .install_rollback_blobs()
        .to_path_buf();
    let mut report = RollbackReclamation::default();
    let entries = reclamation_entries(
        &directory,
        &path,
        protected.map(blob_name).as_deref(),
        &mut report,
    )?;
    let mut deleted = 0_usize;
    for name in entries {
        let Some((file, metadata)) =
            reclamation_candidate(&directory, &path, &name, now, &mut report)?
        else {
            continue;
        };
        report.reclaimable_count += 1;
        report.reclaimable_bytes = report
            .reclaimable_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| invalid("rollback reclaimable byte total overflowed"))?;
        if reclaim
            && remove_reclamation_candidate(
                &directory,
                &path,
                &name,
                &file,
                &metadata,
                &mut report,
            )?
        {
            deleted += 1;
        }
    }
    if deleted != 0 {
        directory.sync_all().map_err(|source| {
            InstallError::io("synchronize migration rollback directory", &path, source)
        })?;
    }
    Ok(report)
}

fn reclamation_entries(
    directory: &File,
    path: &Path,
    protected: Option<&str>,
    report: &mut RollbackReclamation,
) -> Result<Vec<String>, InstallError> {
    let mut entries = Vec::new();
    let mut reader = Dir::read_from(directory).map_err(|source| {
        InstallError::io("read migration rollback directory", path, source.into())
    })?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|source| {
            InstallError::io("read migration rollback directory", path, source.into())
        })?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        if report.examined_entries == RECLAIM_ENTRY_LIMIT {
            report.omitted_entries = 1;
            break;
        }
        report.examined_entries += 1;
        let Some(name) = std::str::from_utf8(bytes).ok() else {
            report
                .unsafe_candidates
                .push("install-rollback/<non-UTF-8>: unsafe name".to_owned());
            continue;
        };
        if protected == Some(name) {
            continue;
        }
        if is_blob_name(name) || is_staged_blob_name(name) {
            entries.push(name.to_owned());
        } else {
            report
                .unsafe_candidates
                .push(format!("install-rollback/{name}: non-canonical name"));
        }
    }
    entries.sort_unstable();
    report.omitted_candidates = entries.len().saturating_sub(RECLAIM_CANDIDATE_LIMIT);
    entries.truncate(RECLAIM_CANDIDATE_LIMIT);
    Ok(entries)
}

fn reclamation_candidate(
    directory: &File,
    path: &Path,
    name: &str,
    now: SystemTime,
    report: &mut RollbackReclamation,
) -> Result<Option<(File, fs::Metadata)>, InstallError> {
    let candidate_path = path.join(name);
    let descriptor = match unix_fs::openat(
        directory,
        OsStr::new(name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => {
            report
                .unsafe_candidates
                .push(format!("install-rollback/{name}: linked name"));
            return Ok(None);
        }
        Err(source) => {
            return Err(InstallError::io(
                "open rollback reclamation candidate",
                candidate_path,
                source.into(),
            ));
        }
    };
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|source| {
        InstallError::io(
            "inspect rollback reclamation candidate",
            &candidate_path,
            source,
        )
    })?;
    if !safe_reclamation_metadata(&metadata) {
        report.unsafe_candidates.push(format!(
            "install-rollback/{name}: ownership, mode, type, link count, or length is unsafe"
        ));
        return Ok(None);
    }
    match reclaim_age(&metadata, now) {
        ReclaimAge::Old => Ok(Some((file, metadata))),
        ReclaimAge::Recent => Ok(None),
        ReclaimAge::Unsafe => {
            report.unsafe_candidates.push(format!(
                "install-rollback/{name}: timestamps are unrepresentable or in the future"
            ));
            Ok(None)
        }
    }
}

fn remove_reclamation_candidate(
    directory: &File,
    path: &Path,
    name: &str,
    file: &File,
    metadata: &fs::Metadata,
    report: &mut RollbackReclamation,
) -> Result<bool, InstallError> {
    let candidate_path = path.join(name);
    let named = match unix_fs::statat(directory, OsStr::new(name), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(named) => named,
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(source) => {
            return Err(InstallError::io(
                "reinspect rollback reclamation candidate",
                candidate_path,
                source.into(),
            ));
        }
    };
    #[cfg(target_vendor = "apple")]
    let named_device = named.st_dev as u64;
    #[cfg(not(target_vendor = "apple"))]
    let named_device = named.st_dev;
    let observed = file.metadata().map_err(|source| {
        InstallError::io(
            "reinspect held rollback reclamation candidate",
            &candidate_path,
            source,
        )
    })?;
    if named_device != metadata.dev()
        || named.st_ino != metadata.ino()
        || !same_reclamation_metadata(&observed, metadata)
    {
        report.unsafe_candidates.push(format!(
            "install-rollback/{name}: identity or metadata changed before removal"
        ));
        return Ok(false);
    }
    match unix_fs::unlinkat(directory, OsStr::new(name), AtFlags::empty()) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(source) => Err(InstallError::io(
            "remove unreferenced rollback blob",
            candidate_path,
            source.into(),
        )),
    }
}

fn is_blob_name(name: &str) -> bool {
    name.strip_suffix(BLOB_SUFFIX).is_some_and(|stem| {
        stem.len() == DIGEST_HEX_CHARACTERS
            && stem.bytes().all(|byte| {
                byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
            })
    })
}

fn is_staged_blob_name(name: &str) -> bool {
    let Some(body) = name
        .strip_prefix(".spice-install-rollback-")
        .and_then(|name| name.strip_suffix(".tmp"))
    else {
        return false;
    };
    let mut parts = body.split('-');
    matches!((parts.next(), parts.next(), parts.next()), (Some(pid), Some(counter), None)
        if canonical_decimal(pid) && canonical_decimal(counter))
}

fn canonical_decimal(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn safe_reclamation_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.uid() == geteuid().as_raw()
        && metadata.mode() & 0o7777 == BLOB_MODE
        && metadata.nlink() == BLOB_LINK_COUNT
        && metadata.len() <= MAX_STORED_BYTES
}

fn same_reclamation_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

enum ReclaimAge {
    Old,
    Recent,
    Unsafe,
}

fn reclaim_age(metadata: &fs::Metadata, now: SystemTime) -> ReclaimAge {
    let Some(modified) = timestamp(metadata.mtime(), metadata.mtime_nsec()) else {
        return ReclaimAge::Unsafe;
    };
    let Some(changed) = timestamp(metadata.ctime(), metadata.ctime_nsec()) else {
        return ReclaimAge::Unsafe;
    };
    let (Ok(modified_age), Ok(changed_age)) =
        (now.duration_since(modified), now.duration_since(changed))
    else {
        return ReclaimAge::Unsafe;
    };
    if modified_age >= RECLAIM_GRACE && changed_age >= RECLAIM_GRACE {
        ReclaimAge::Old
    } else {
        ReclaimAge::Recent
    }
}

fn timestamp(seconds: i64, nanoseconds: i64) -> Option<SystemTime> {
    if !(0..1_000_000_000).contains(&nanoseconds) {
        return None;
    }
    let total = i128::from(seconds)
        .checked_mul(1_000_000_000)?
        .checked_add(i128::from(nanoseconds))?;
    let magnitude = total.unsigned_abs();
    let seconds = u64::try_from(magnitude / 1_000_000_000).ok()?;
    let nanoseconds = u32::try_from(magnitude % 1_000_000_000).ok()?;
    let duration = Duration::new(seconds, nanoseconds);
    if total >= 0 {
        UNIX_EPOCH.checked_add(duration)
    } else {
        UNIX_EPOCH.checked_sub(duration)
    }
}

fn blob_path(repository: &Repository, name: &str) -> Result<std::path::PathBuf, InstallError> {
    Ok(repository
        .admit_state()?
        .install_rollback_blobs()
        .join(name))
}

fn open_blob_directory(
    repository: &Repository,
    create: bool,
) -> Result<Option<File>, InstallError> {
    let state = repository.admit_state()?;
    let path = state.install_rollback_blobs();
    let Some(runtime) = open_runtime_directory(state.runtime(), create)? else {
        return Ok(None);
    };
    let created = create && create_blob_directory(&runtime, path)?;
    let Some(directory) = open_named_blob_directory(&runtime, path, create)? else {
        return Ok(None);
    };
    if created {
        initialize_blob_directory(&runtime, &directory, state.runtime(), path)?;
    }
    validate_blob_directory(&directory, path)?;
    Ok(Some(directory))
}

fn open_runtime_directory(path: &Path, create: bool) -> Result<Option<File>, InstallError> {
    match unix_fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(descriptor) => Ok(Some(File::from(descriptor))),
        Err(rustix::io::Errno::NOENT) if !create => Ok(None),
        Err(source) => Err(InstallError::io(
            "open install runtime directory",
            path,
            source.into(),
        )),
    }
}

fn create_blob_directory(runtime: &File, path: &Path) -> Result<bool, InstallError> {
    match unix_fs::mkdirat(
        runtime,
        OsStr::new("install-rollback"),
        Mode::from_raw_mode(BLOB_DIRECTORY_MODE as _),
    ) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::EXIST) => Ok(false),
        Err(source) => Err(InstallError::io(
            "create migration rollback directory",
            path,
            source.into(),
        )),
    }
}

fn open_named_blob_directory(
    runtime: &File,
    path: &Path,
    create: bool,
) -> Result<Option<File>, InstallError> {
    match unix_fs::openat(
        runtime,
        OsStr::new("install-rollback"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(descriptor) => Ok(Some(File::from(descriptor))),
        Err(rustix::io::Errno::NOENT) if !create => Ok(None),
        Err(source) => Err(InstallError::io(
            "open migration rollback directory",
            path,
            source.into(),
        )),
    }
}

fn initialize_blob_directory(
    runtime: &File,
    directory: &File,
    runtime_path: &Path,
    path: &Path,
) -> Result<(), InstallError> {
    directory
        .set_permissions(fs::Permissions::from_mode(BLOB_DIRECTORY_MODE))
        .map_err(|source| {
            InstallError::io("set migration rollback directory mode", path, source)
        })?;
    runtime.sync_all().map_err(|source| {
        InstallError::io(
            "synchronize install runtime directory",
            runtime_path,
            source,
        )
    })
}

fn validate_blob_directory(directory: &File, path: &Path) -> Result<(), InstallError> {
    let metadata = directory
        .metadata()
        .map_err(|source| InstallError::io("inspect migration rollback directory", path, source))?;
    if !metadata.is_dir()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o7777 != BLOB_DIRECTORY_MODE
    {
        return Err(invalid(
            "migration rollback directory ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn publish_blob(
    repository: &Repository,
    blob: &RollbackBlob,
    bytes: &[u8],
) -> Result<(), InstallError> {
    let directory = open_blob_directory(repository, true)?
        .expect("creating the rollback directory returns a descriptor");
    let name = blob_name(&blob.stored_digest);
    let path = blob_path(repository, &name)?;
    match unix_fs::openat(
        &directory,
        OsStr::new(&name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    ) {
        Ok(descriptor) => {
            blob.read_file_and_validate(File::from(descriptor), &path)?;
            directory.sync_all().map_err(|source| {
                InstallError::io("synchronize migration rollback directory", &path, source)
            })?;
            return Ok(());
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(source) => {
            return Err(InstallError::io(
                "open existing migration rollback blob",
                &path,
                source.into(),
            ));
        }
    }
    let temporary = temporary_name();
    let descriptor = unix_fs::openat(
        &directory,
        &temporary,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(BLOB_MODE as _),
    )
    .map_err(|source| {
        InstallError::io(
            "create staged migration rollback blob",
            &path,
            source.into(),
        )
    })?;
    let mut staged = File::from(descriptor);
    let result = (|| {
        staged
            .set_permissions(fs::Permissions::from_mode(BLOB_MODE))
            .map_err(|source| {
                InstallError::io("set staged migration rollback blob mode", &path, source)
            })?;
        staged.write_all(bytes).map_err(|source| {
            InstallError::io("write staged migration rollback blob", &path, source)
        })?;
        validate_blob_file(&staged, &path, blob.stored_bytes)?;
        staged.sync_all().map_err(|source| {
            InstallError::io("synchronize staged migration rollback blob", &path, source)
        })?;
        match unix_fs::renameat_with(
            &directory,
            &temporary,
            &directory,
            OsStr::new(&name),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => {
                unix_fs::unlinkat(&directory, &temporary, AtFlags::empty()).map_err(|source| {
                    InstallError::io(
                        "remove duplicate staged migration rollback blob",
                        &path,
                        source.into(),
                    )
                })?;
                blob.read_and_validate(repository)?;
            }
            Err(source) => {
                return Err(InstallError::io(
                    "publish migration rollback blob",
                    &path,
                    source.into(),
                ));
            }
        }
        directory.sync_all().map_err(|source| {
            InstallError::io("synchronize migration rollback directory", &path, source)
        })
    })();
    if result.is_err() {
        let _ = unix_fs::unlinkat(&directory, &temporary, AtFlags::empty());
    }
    result
}

fn temporary_name() -> OsString {
    OsString::from(format!(
        ".spice-install-rollback-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed),
    ))
}

fn validate_blob_file(
    file: &File,
    path: &std::path::Path,
    expected: u64,
) -> Result<(), InstallError> {
    let metadata = file
        .metadata()
        .map_err(|source| InstallError::io("inspect migration rollback blob", path, source))?;
    validate_blob_metadata(&metadata, expected, geteuid().as_raw())
}

fn validate_blob_metadata(
    metadata: &fs::Metadata,
    expected: u64,
    expected_owner: u32,
) -> Result<(), InstallError> {
    if !metadata.is_file()
        || metadata.uid() != expected_owner
        || metadata.mode() & 0o7777 != BLOB_MODE
        || metadata.nlink() != BLOB_LINK_COUNT
    {
        return Err(invalid(
            "migration rollback blob ownership, mode, type, or link count is unsafe",
        ));
    }
    if metadata.len() != expected || metadata.len() > MAX_STORED_BYTES {
        return Err(invalid(
            "migration rollback blob length does not match its manifest",
        ));
    }
    Ok(())
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
        let encoded_count = self.u32()?;
        if encoded_count > MAX_ARCHIVE_IMAGE_COUNT {
            return Err(invalid(
                "migration rollback archive has too many image entries",
            ));
        }
        let count = usize::try_from(encoded_count)
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
    fn blob_metadata_refuses_a_non_owner_even_when_every_other_field_is_safe() {
        let path = std::env::temp_dir().join(format!(
            "spice-rollback-owner-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed),
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create private ownership fixture");
        file.set_permissions(fs::Permissions::from_mode(BLOB_MODE))
            .expect("make ownership fixture private");
        let metadata = file.metadata().expect("inspect ownership fixture");
        assert!(validate_blob_metadata(&metadata, 0, metadata.uid()).is_ok());
        let foreign = metadata.uid().wrapping_add(1);
        assert!(matches!(
            validate_blob_metadata(&metadata, 0, foreign),
            Err(InstallError::InvalidRecord(_))
        ));
        drop(file);
        fs::remove_file(path).expect("remove ownership fixture");
    }

    #[test]
    fn archive_image_count_is_bounded_before_vector_allocation() {
        let encoded = u32::MAX.to_be_bytes();
        let mut reader = ArchiveReader::new(&encoded);
        assert!(matches!(
            reader.images(),
            Err(InstallError::InvalidRecord(reason))
                if reason == "migration rollback archive has too many image entries"
        ));
    }

    #[test]
    fn archive_round_trips_and_rejects_payload_corruption() {
        let state = RollbackState {
            record: FileImage::new(0o600, b"receipt".to_vec()),
            operations: vec![Some(FileImage::new(0o755, vec![b'x'; 1024 * 1024])), None],
            retirements: vec![Some(FileImage::new(0o755, vec![b'y'; 1024 * 1024]))],
        };
        let archive = RollbackArchive::new(&state).expect("archive fixture state");
        assert_eq!(
            archive
                .staged_state()
                .expect("decode archive fixture")
                .expect("new archive retains staged bytes"),
            state,
        );
        let RollbackArchive::Blob(blob) = &archive else {
            panic!("new archives use rollback blobs");
        };
        assert!(blob.stored_bytes < 32 * 1024);

        let mut corrupt = archive;
        let RollbackArchive::Blob(blob) = &mut corrupt else {
            panic!("new archives use rollback blobs");
        };
        let staged = blob.staged.as_ref().unwrap();
        let mut bytes = staged.bytes.to_vec();
        bytes[0] ^= 0xff;
        blob.staged = Some(StagedRollbackBlob {
            bytes: Arc::from(bytes),
            state: None,
        });
        assert!(matches!(
            corrupt.staged_state(),
            Err(InstallError::InvalidRecord(_))
        ));
    }
}
