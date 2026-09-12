use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};

use wire::manifest::JournalBackupManifest;
use wire::{Digest, Domain, RestoreVerification, TaskStamp, WireVersion};

use crate::{JournalReader, OpenError, VerifyError};

const MANIFEST: &str = "manifest.json";

mod manifest;
use manifest::admit_manifest_generation;

/// Authenticates one offline journal backup bundle without writer capability.
///
/// # Errors
///
/// Returns a typed bundle refusal or filesystem/store failure. The bundle is
/// never repaired, migrated, or otherwise mutated.
pub fn verify_restore_bundle(
    root: impl AsRef<Path>,
) -> Result<RestoreVerification, BackupVerifyError> {
    capture_restore_bundle(root).map(RestoreCapture::into_verification)
}

/// One fully authenticated restore bundle captured into process-owned bytes.
#[derive(Debug)]
pub struct RestoreCapture {
    verification: RestoreVerification,
    authority: Vec<u8>,
    authority_path: String,
    members: Vec<RestoreMember>,
    tasks: Vec<TaskStamp>,
}

/// One authenticated regular file captured from a restore bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreMember {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

impl RestoreMember {
    /// Canonical slash-separated member path relative to the bundle root.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Exact POSIX mode authenticated by the bundle manifest.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact authenticated member bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl RestoreCapture {
    /// Identities and replay counts proved for the captured generation.
    #[must_use]
    pub const fn verification(&self) -> &RestoreVerification {
        &self.verification
    }

    /// Exact authenticated SQLite bytes named by the bundle manifest.
    #[must_use]
    pub fn authority_bytes(&self) -> &[u8] {
        &self.authority
    }

    /// Canonical member path carrying the SQLite authority.
    #[must_use]
    pub fn authority_path(&self) -> &str {
        &self.authority_path
    }

    /// Every authenticated bundle member in canonical manifest order.
    #[must_use]
    pub fn members(&self) -> &[RestoreMember] {
        &self.members
    }

    /// Every task in the captured journal head set, ordered by birth.
    #[must_use]
    pub fn tasks(&self) -> &[TaskStamp] {
        &self.tasks
    }

    /// Consumes the capture and returns its public verification receipt.
    #[must_use]
    pub fn into_verification(self) -> RestoreVerification {
        self.verification
    }
}

/// Authenticates and captures one bundle before any caller mutates authority.
pub fn capture_restore_bundle(root: impl AsRef<Path>) -> Result<RestoreCapture, BackupVerifyError> {
    capture_restore_bundle_with_hooks(root.as_ref(), |_| {}, |_| {})
}

#[cfg(test)]
fn verify_restore_bundle_after_capture<F>(
    root: &Path,
    after_capture: F,
) -> Result<RestoreVerification, BackupVerifyError>
where
    F: FnOnce(&Path),
{
    capture_restore_bundle_with_hooks(root, |_| {}, after_capture)
        .map(RestoreCapture::into_verification)
}

#[cfg(test)]
fn verify_restore_bundle_after_inventory<F>(
    root: &Path,
    after_inventory: F,
) -> Result<RestoreVerification, BackupVerifyError>
where
    F: FnOnce(&Path),
{
    capture_restore_bundle_with_hooks(root, after_inventory, |_| {})
        .map(RestoreCapture::into_verification)
}

fn capture_restore_bundle_with_hooks<I, C>(
    root: &Path,
    after_inventory: I,
    after_capture: C,
) -> Result<RestoreCapture, BackupVerifyError>
where
    I: FnOnce(&Path),
    C: FnOnce(&Path),
{
    let bundle = BundleAuthority::open(root)?;
    let (manifest, manifest_bytes) = read_manifest(&bundle)?;
    let manifest = admit_manifest_generation(manifest)?;
    let authority = admitted_member_path(&manifest.authority)?;
    let (authority_capture, members) =
        authenticate_members(&bundle, &manifest, &authority, after_inventory)?;
    after_capture(&root.join(&authority));
    let (verified, tasks) = verify_authority(&authority_capture.bytes, manifest.journal_schema)?;

    Ok(RestoreCapture {
        verification: RestoreVerification {
            bundle: Digest::compute(Domain::JournalBackupManifest, &manifest_bytes),
            wire: manifest.wire,
            journal_schema: manifest.journal_schema,
            authority: authority_capture.digest,
            members: count(manifest.members.len(), "members")?,
            facts: count(verified.facts, "facts")?,
            receipts: count(verified.receipts, "receipts")?,
            heads: count(verified.heads, "heads")?,
            threads: count(verified.threads, "threads")?,
        },
        authority: authority_capture.bytes,
        authority_path: manifest.authority,
        members,
        tasks,
    })
}

struct BundleAuthority {
    root: PathBuf,
    #[cfg(unix)]
    directory: File,
}

impl BundleAuthority {
    fn open(root: &Path) -> Result<Self, BackupVerifyError> {
        #[cfg(unix)]
        {
            let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW;
            let directory = rustix::fs::open(root, flags, Mode::empty())
                .map(File::from)
                .map_err(|source| confined_error(root, source.into()))?;
            Ok(Self {
                root: root.to_path_buf(),
                directory,
            })
        }
        #[cfg(not(unix))]
        {
            Err(BackupVerifyError::Invalid {
                subject: root.to_path_buf(),
                detail: "directory-authority restore verification requires a Unix target"
                    .to_owned(),
            })
        }
    }

    #[cfg(unix)]
    fn root_directory(&self) -> Result<File, BackupVerifyError> {
        self.directory
            .try_clone()
            .map_err(|source| BackupVerifyError::Io {
                path: self.root.clone(),
                source,
            })
    }

    fn open_regular(&self, relative: &Path) -> Result<File, BackupVerifyError> {
        #[cfg(unix)]
        {
            let name = relative
                .file_name()
                .ok_or_else(|| BackupVerifyError::Invalid {
                    subject: self.root.join(relative),
                    detail: "backup member path has no file name".to_owned(),
                })?;
            let mut directory = self.root_directory()?;
            let mut traversed = PathBuf::new();
            if let Some(parent) = relative.parent() {
                for component in parent.components() {
                    let Component::Normal(component) = component else {
                        return Err(BackupVerifyError::Invalid {
                            subject: self.root.join(relative),
                            detail: "backup member path left its captured directory authority"
                                .to_owned(),
                        });
                    };
                    traversed.push(component);
                    directory = open_directory_at(&directory, component)
                        .map_err(|source| confined_error(&self.root.join(&traversed), source))?;
                }
            }
            open_file_at(&directory, name)
                .map_err(|source| confined_error(&self.root.join(relative), source))
        }
        #[cfg(not(unix))]
        {
            Err(BackupVerifyError::Invalid {
                subject: self.root.join(relative),
                detail: "directory-authority restore verification requires a Unix target"
                    .to_owned(),
            })
        }
    }
}

fn read_manifest(
    bundle: &BundleAuthority,
) -> Result<(JournalBackupManifest, Vec<u8>), BackupVerifyError> {
    let path = bundle.root.join(MANIFEST);
    let bytes = capture_regular_file(bundle, Path::new(MANIFEST))?.bytes;
    let manifest = wire::decode::<JournalBackupManifest>(&bytes).map_err(|error| {
        BackupVerifyError::Invalid {
            subject: path.clone(),
            detail: format!("backup manifest does not decode: {error}"),
        }
    })?;
    let canonical = wire::encode(&manifest).map_err(|error| BackupVerifyError::Invalid {
        subject: path.clone(),
        detail: format!("backup manifest does not encode: {error}"),
    })?;
    if canonical == bytes {
        Ok((manifest, bytes))
    } else {
        Err(BackupVerifyError::Invalid {
            subject: path,
            detail: "backup manifest is not canonical current wire JSON".to_owned(),
        })
    }
}

fn authenticate_members<F>(
    bundle: &BundleAuthority,
    manifest: &JournalBackupManifest,
    authority: &Path,
    after_inventory: F,
) -> Result<(CapturedMember, Vec<RestoreMember>), BackupVerifyError>
where
    F: FnOnce(&Path),
{
    admit_inventory(bundle, manifest)?;
    after_inventory(&bundle.root);
    let mut previous: Option<&str> = None;
    let mut authority_capture = None;
    let mut captures = Vec::with_capacity(manifest.members.len());
    for member in &manifest.members {
        let relative = admitted_member_path(&member.path)?;
        admit_member_order(&bundle.root, previous, &member.path)?;
        previous = Some(&member.path);
        let observed =
            authenticate_member(bundle, &relative, member.mode, member.bytes, &member.digest)?;
        if relative == authority {
            authority_capture = Some(CapturedMember {
                digest: observed.digest.clone(),
                bytes: observed.bytes.clone(),
            });
        }
        captures.push(RestoreMember {
            path: member.path.clone(),
            mode: member.mode,
            bytes: observed.bytes,
        });
    }
    authority_capture
        .map(|authority| (authority, captures))
        .ok_or_else(|| BackupVerifyError::Incomplete {
            detail: format!(
                "authority member {:?} is absent from the manifest inventory",
                manifest.authority
            ),
        })
}

fn admit_inventory(
    bundle: &BundleAuthority,
    manifest: &JournalBackupManifest,
) -> Result<(), BackupVerifyError> {
    let inventory = bundle_inventory(bundle)?;
    let declared = manifest
        .members
        .iter()
        .map(|member| member.path.clone())
        .collect::<Vec<_>>();
    if declared == inventory {
        Ok(())
    } else {
        Err(BackupVerifyError::Incomplete {
            detail: format!(
                "manifest members {declared:?} do not equal bundle inventory {inventory:?}"
            ),
        })
    }
}

fn admit_member_order(
    root: &Path,
    previous: Option<&str>,
    current: &str,
) -> Result<(), BackupVerifyError> {
    if previous.is_none_or(|previous| previous < current) {
        Ok(())
    } else {
        Err(BackupVerifyError::Invalid {
            subject: root.join(MANIFEST),
            detail: "backup manifest members are not strictly ordered by path".to_owned(),
        })
    }
}

fn authenticate_member(
    bundle: &BundleAuthority,
    relative: &Path,
    declared_mode: u32,
    declared_len: u64,
    declared_digest: &Digest,
) -> Result<CapturedMember, BackupVerifyError> {
    let path = bundle.root.join(relative);
    if declared_mode & !0o7777 != 0 {
        return Err(BackupVerifyError::Invalid {
            subject: bundle.root.join(MANIFEST),
            detail: format!("member mode {declared_mode:#o} is outside the POSIX 0o7777 mask"),
        });
    }
    let captured = capture_regular_file(bundle, relative)?;
    if declared_mode != captured.mode {
        return Err(BackupVerifyError::Invalid {
            subject: path,
            detail: format!(
                "member mode declares {declared_mode:#o}, observed {:#o}",
                captured.mode
            ),
        });
    }
    let observed_len =
        u64::try_from(captured.bytes.len()).map_err(|error| BackupVerifyError::Invalid {
            subject: path.clone(),
            detail: format!("member length cannot cross as u64: {error}"),
        })?;
    if declared_len != observed_len {
        return Err(BackupVerifyError::Invalid {
            subject: path,
            detail: format!("member length declares {declared_len}, observed {observed_len}"),
        });
    }
    let observed = Digest::compute(Domain::JournalBackupMember, &captured.bytes);
    if *declared_digest == observed {
        Ok(CapturedMember {
            digest: observed,
            bytes: captured.bytes,
        })
    } else {
        Err(BackupVerifyError::Invalid {
            subject: path,
            detail: format!("member digest declares {declared_digest}, observed {observed}"),
        })
    }
}

fn verify_authority(
    authority: &[u8],
    manifest_schema: u32,
) -> Result<(crate::Verification, Vec<TaskStamp>), BackupVerifyError> {
    let reader = JournalReader::from_serialized(authority).map_err(BackupVerifyError::Authority)?;
    let schema = reader
        .schema_version()
        .map_err(BackupVerifyError::Authority)?;
    if schema != manifest_schema {
        return Err(BackupVerifyError::SchemaMismatch {
            manifest: manifest_schema,
            authority: schema,
        });
    }
    let verified = reader.verify().map_err(BackupVerifyError::Replay)?;
    let tasks = reader
        .read(|read| {
            let page = read.task_head_page(None, crate::FactReadBudget::RETAINED)?;
            if let Some(continuation) = page.continuation {
                return Err(crate::GuardError::Invalid(format!(
                    "backup authority task heads require continuation after {continuation}"
                )));
            }
            Ok(page.rows)
        })
        .map_err(BackupVerifyError::AuthorityView)?
        .into_iter()
        .map(|heads| heads.statement().body().born.clone())
        .collect();
    Ok((verified, tasks))
}

fn count(value: usize, subject: &str) -> Result<u64, BackupVerifyError> {
    u64::try_from(value).map_err(|error| BackupVerifyError::Invalid {
        subject: PathBuf::from(MANIFEST),
        detail: format!("{subject} count cannot cross as u64: {error}"),
    })
}

#[derive(Debug)]
struct CapturedFile {
    mode: u32,
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct CapturedMember {
    digest: Digest,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    len: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
            #[cfg(unix)]
            modified_seconds: metadata.mtime(),
            #[cfg(unix)]
            modified_nanoseconds: metadata.mtime_nsec(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

fn capture_regular_file(
    bundle: &BundleAuthority,
    relative: &Path,
) -> Result<CapturedFile, BackupVerifyError> {
    let path = bundle.root.join(relative);
    let before_file = bundle.open_regular(relative)?;
    let before = before_file
        .metadata()
        .map_err(|source| BackupVerifyError::Io {
            path: path.clone(),
            source,
        })?;
    require_regular(&path, &before)?;
    let expected = FileIdentity::from_metadata(&before);
    drop(before_file);
    let mut file = bundle.open_regular(relative)?;
    let opened = file.metadata().map_err(|source| BackupVerifyError::Io {
        path: path.clone(),
        source,
    })?;
    require_regular(&path, &opened)?;
    if FileIdentity::from_metadata(&opened) != expected {
        return changed_during_capture(&path);
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| BackupVerifyError::Io {
            path: path.clone(),
            source,
        })?;

    let after_handle = file.metadata().map_err(|source| BackupVerifyError::Io {
        path: path.clone(),
        source,
    })?;
    let after_file = bundle.open_regular(relative)?;
    let after_path = after_file
        .metadata()
        .map_err(|source| BackupVerifyError::Io {
            path: path.clone(),
            source,
        })?;
    require_regular(&path, &after_handle)?;
    require_regular(&path, &after_path)?;
    if FileIdentity::from_metadata(&after_handle) != expected
        || FileIdentity::from_metadata(&after_path) != expected
        || u64::try_from(bytes.len()).ok() != Some(expected.len)
    {
        return changed_during_capture(&path);
    }

    Ok(CapturedFile {
        mode: posix_mode(&path, &after_handle)?,
        bytes,
    })
}

#[cfg(unix)]
fn open_directory_at(parent: &File, component: &OsStr) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW;
    rustix::fs::openat(parent, component, flags, Mode::empty())
        .map(File::from)
        .map_err(Into::into)
}

#[cfg(unix)]
fn open_file_at(parent: &File, component: &OsStr) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    rustix::fs::openat(parent, component, flags, Mode::empty())
        .map(File::from)
        .map_err(Into::into)
}

fn confined_error(path: &Path, source: io::Error) -> BackupVerifyError {
    if source.kind() == io::ErrorKind::NotFound {
        BackupVerifyError::Missing {
            path: path.to_path_buf(),
        }
    } else if is_unconfined(&source) {
        BackupVerifyError::Invalid {
            subject: path.to_path_buf(),
            detail: concat!(
                "backup path contains a symlink or non-directory component outside ",
                "its captured directory authority"
            )
            .to_owned(),
        }
    } else {
        BackupVerifyError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

fn is_unconfined(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error().is_some_and(|code| {
            code == rustix::io::Errno::LOOP.raw_os_error()
                || code == rustix::io::Errno::NOTDIR.raw_os_error()
        })
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn require_regular(path: &Path, metadata: &Metadata) -> Result<(), BackupVerifyError> {
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(BackupVerifyError::Invalid {
            subject: path.to_path_buf(),
            detail: "backup member is not a real regular file".to_owned(),
        })
    }
}

fn changed_during_capture(path: &Path) -> Result<CapturedFile, BackupVerifyError> {
    Err(BackupVerifyError::Invalid {
        subject: path.to_path_buf(),
        detail: "backup member changed identity while its generation was captured".to_owned(),
    })
}

fn posix_mode(path: &Path, metadata: &Metadata) -> Result<u32, BackupVerifyError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let _ = path;
        Ok(metadata.mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(BackupVerifyError::Invalid {
            subject: path.to_path_buf(),
            detail: "backup member POSIX mode is unavailable on this platform".to_owned(),
        })
    }
}

fn admitted_member_path(text: &str) -> Result<PathBuf, BackupVerifyError> {
    let path = Path::new(text);
    let canonical = !text.is_empty()
        && !text.contains('\\')
        && text != MANIFEST
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .is_some_and(|parts| parts.join("/") == text);
    if canonical {
        Ok(path.to_path_buf())
    } else {
        Err(BackupVerifyError::Invalid {
            subject: PathBuf::from(MANIFEST),
            detail: format!("backup member path is not canonical and relative: {text:?}"),
        })
    }
}

fn bundle_inventory(bundle: &BundleAuthority) -> Result<Vec<String>, BackupVerifyError> {
    #[cfg(unix)]
    {
        let mut inventory = Vec::new();
        walk(
            bundle,
            bundle.root_directory()?,
            Path::new(""),
            &mut inventory,
        )?;
        inventory.sort();
        Ok(inventory)
    }
    #[cfg(not(unix))]
    {
        Err(BackupVerifyError::Invalid {
            subject: bundle.root.clone(),
            detail: "directory-authority restore verification requires a Unix target".to_owned(),
        })
    }
}

#[cfg(unix)]
fn walk(
    bundle: &BundleAuthority,
    directory: File,
    relative: &Path,
    inventory: &mut Vec<String>,
) -> Result<(), BackupVerifyError> {
    let display = bundle.root.join(relative);
    let mut entries = Dir::read_from(&directory).map_err(|source| BackupVerifyError::Io {
        path: display.clone(),
        source: source.into(),
    })?;
    let mut names = Vec::new();
    for entry in &mut entries {
        let entry = entry.map_err(|source| BackupVerifyError::Io {
            path: display.clone(),
            source: source.into(),
        })?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let name = std::str::from_utf8(bytes).map_err(|_| BackupVerifyError::Invalid {
            subject: display.clone(),
            detail: "backup member name is not UTF-8".to_owned(),
        })?;
        names.push(name.to_owned());
    }
    names.sort();
    for name in names {
        let child = relative.join(&name);
        let path = bundle.root.join(&child);
        let stat = rustix::fs::statat(&directory, OsStr::new(&name), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|source| confined_error(&path, source.into()))?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {
                let child_directory = open_directory_at(&directory, OsStr::new(&name))
                    .map_err(|source| confined_error(&path, source))?;
                walk(bundle, child_directory, &child, inventory)?;
            }
            FileType::RegularFile => {
                let spelling = child
                    .components()
                    .map(|component| component.as_os_str().to_str())
                    .collect::<Option<Vec<_>>>()
                    .map(|parts| parts.join("/"))
                    .ok_or_else(|| BackupVerifyError::Invalid {
                        subject: path,
                        detail: "backup member path is not UTF-8".to_owned(),
                    })?;
                if spelling != MANIFEST {
                    inventory.push(spelling);
                }
            }
            _ => {
                return Err(BackupVerifyError::Invalid {
                    subject: path,
                    detail: "backup inventory contains a symlink or special file".to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Why an offline backup bundle did not authenticate.
#[derive(Debug)]
pub enum BackupVerifyError {
    Missing {
        path: PathBuf,
    },
    Incomplete {
        detail: String,
    },
    UnsupportedVersion {
        found: u16,
        current: u16,
    },
    UnsupportedWire {
        found: WireVersion,
        current: WireVersion,
    },
    ForwardSchema {
        found: u32,
        current: u32,
    },
    SchemaMismatch {
        manifest: u32,
        authority: u32,
    },
    Invalid {
        subject: PathBuf,
        detail: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Authority(OpenError),
    AuthorityView(crate::GuardError),
    Replay(VerifyError),
}

impl fmt::Display for BackupVerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => {
                write!(formatter, "backup member is missing: {}", path.display())
            }
            Self::Incomplete { detail } => {
                write!(formatter, "backup bundle is incomplete: {detail}")
            }
            Self::UnsupportedVersion { found, current } => write!(
                formatter,
                "backup manifest version {found} is unsupported; current is {current}"
            ),
            Self::UnsupportedWire { found, current } => write!(
                formatter,
                "backup manifest wire version {found} is unsupported; current is {current}"
            ),
            Self::ForwardSchema { found, current } => write!(
                formatter,
                "backup journal schema {found} is newer than current schema {current}"
            ),
            Self::SchemaMismatch {
                manifest,
                authority,
            } => write!(
                formatter,
                "backup manifest schema {manifest} disagrees with authority schema {authority}"
            ),
            Self::Invalid { subject, detail } => write!(
                formatter,
                "backup bundle is invalid at {}: {detail}",
                subject.display()
            ),
            Self::Io { path, source } => write!(
                formatter,
                "backup bundle access failed at {}: {source}",
                path.display()
            ),
            Self::Authority(error) => write!(formatter, "backup authority was refused: {error}"),
            Self::AuthorityView(error) => {
                write!(formatter, "backup authority task view was refused: {error}")
            }
            Self::Replay(error) => write!(formatter, "backup replay closure failed: {error}"),
        }
    }
}

impl Error for BackupVerifyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Authority(error) => Some(error),
            Self::AuthorityView(error) => Some(error),
            Self::Replay(error) => Some(error),
            Self::Missing { .. }
            | Self::Incomplete { .. }
            | Self::UnsupportedVersion { .. }
            | Self::UnsupportedWire { .. }
            | Self::ForwardSchema { .. }
            | Self::SchemaMismatch { .. }
            | Self::Invalid { .. } => None,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};
    use wire::{Digest, Domain, WireVersion};

    use super::{
        BackupVerifyError, verify_restore_bundle_after_capture,
        verify_restore_bundle_after_inventory,
    };

    static NEXT_BUNDLE: AtomicU64 = AtomicU64::new(0);

    struct Bundle(PathBuf);

    impl Bundle {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "journal-restore-captured-generation-{}-{}",
                std::process::id(),
                NEXT_BUNDLE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("bundle root creates");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Bundle {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("bundle fixture removes");
        }
    }

    #[test]
    fn intermediate_symlink_substitution_cannot_escape_the_captured_bundle() {
        use std::os::unix::fs::symlink;

        let bundle = Bundle::new();
        let outside = Bundle::new();
        let authority = bundle.path().join("spicejournal.sqlite3");
        drop(crate::Journal::open(&authority).expect("authority initializes"));
        fs::create_dir(bundle.path().join("tape")).expect("tape directory creates");
        let member_bytes = b"exact external member bytes";
        fs::write(bundle.path().join("tape/lane-a"), member_bytes).expect("bundle member writes");
        fs::write(outside.path().join("lane-a"), member_bytes).expect("external member writes");

        let mut members = ["spicejournal.sqlite3", "tape/lane-a"]
            .into_iter()
            .map(|relative| {
                let path = bundle.path().join(relative);
                let bytes = fs::read(&path).expect("manifest member reads");
                JournalBackupMember {
                    path: relative.to_owned(),
                    mode: fs::metadata(path)
                        .expect("manifest member metadata reads")
                        .permissions()
                        .mode()
                        & 0o7777,
                    bytes: u64::try_from(bytes.len()).expect("member length crosses"),
                    digest: Digest::compute(Domain::JournalBackupMember, &bytes),
                }
            })
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = JournalBackupManifest {
            version: JOURNAL_BACKUP_VERSION,
            wire: WireVersion::CURRENT,
            journal_schema: crate::SCHEMA_VERSION,
            authority: "spicejournal.sqlite3".to_owned(),
            members,
        };
        fs::write(
            bundle.path().join("manifest.json"),
            wire::encode(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");

        let held = bundle.path().join("captured-tape");
        let result = verify_restore_bundle_after_inventory(bundle.path(), |root| {
            fs::rename(root.join("tape"), &held).expect("inventory directory displaces");
            symlink(outside.path(), root.join("tape")).expect("external directory substitutes");
        });

        assert!(matches!(
            result,
            Err(BackupVerifyError::Invalid { detail, .. })
                if detail.contains("captured directory authority")
        ));
        assert_eq!(
            fs::read(outside.path().join("lane-a")).expect("external member reads"),
            member_bytes
        );
    }

    #[test]
    fn authority_replay_stays_bound_to_the_generation_captured_before_path_replacement() {
        let bundle = Bundle::new();
        let authority = bundle.path().join("spicejournal.sqlite3");
        drop(crate::Journal::open(&authority).expect("authority initializes"));
        let authority_bytes = fs::read(&authority).expect("authority reads");
        let authority_digest = Digest::compute(Domain::JournalBackupMember, &authority_bytes);
        let mode = fs::metadata(&authority)
            .expect("authority metadata reads")
            .permissions()
            .mode()
            & 0o7777;
        let manifest = JournalBackupManifest {
            version: JOURNAL_BACKUP_VERSION,
            wire: WireVersion::CURRENT,
            journal_schema: crate::SCHEMA_VERSION,
            authority: "spicejournal.sqlite3".to_owned(),
            members: vec![JournalBackupMember {
                path: "spicejournal.sqlite3".to_owned(),
                mode,
                bytes: u64::try_from(authority_bytes.len()).expect("authority length crosses"),
                digest: authority_digest.clone(),
            }],
        };
        fs::write(
            bundle.path().join("manifest.json"),
            wire::encode(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");

        let displaced = bundle.path().join("captured.sqlite3");
        let result = verify_restore_bundle_after_capture(bundle.path(), |path| {
            fs::rename(path, &displaced).expect("captured authority displaces");
            fs::write(path, b"replacement is deliberately not SQLite")
                .expect("replacement authority writes");
        })
        .expect("captured authority verifies despite replacement");

        assert_eq!(result.authority, authority_digest);
        assert_eq!(result.wire, WireVersion::CURRENT);
        assert_eq!(result.journal_schema, crate::SCHEMA_VERSION);
        assert_eq!(result.members, 1);
        assert_eq!(
            fs::read(authority).expect("replacement reads"),
            b"replacement is deliberately not SQLite"
        );
        assert_eq!(
            fs::read(displaced).expect("captured authority reads"),
            authority_bytes
        );
    }
}
