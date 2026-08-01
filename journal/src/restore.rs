use std::error::Error;
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest};
use wire::{Digest, Domain, RestoreVerification, WireVersion};

use crate::{JournalReader, OpenError, SCHEMA_VERSION, VerifyError};

const MANIFEST: &str = "manifest.json";

/// Authenticates one offline journal backup bundle without writer capability.
///
/// # Errors
///
/// Returns a typed bundle refusal or filesystem/store failure. The bundle is
/// never repaired, migrated, or otherwise mutated.
pub fn verify_restore_bundle(
    root: impl AsRef<Path>,
) -> Result<RestoreVerification, BackupVerifyError> {
    verify_restore_bundle_after_capture(root.as_ref(), |_| {})
}

fn verify_restore_bundle_after_capture<F>(
    root: &Path,
    after_capture: F,
) -> Result<RestoreVerification, BackupVerifyError>
where
    F: FnOnce(&Path),
{
    admit_root(root)?;
    let (manifest, manifest_bytes) = read_manifest(root)?;
    admit_manifest_versions(&manifest)?;
    let authority = admitted_member_path(&manifest.authority)?;
    let authority_capture = authenticate_members(root, &manifest, &authority)?;
    after_capture(&root.join(&authority));
    let verified = verify_authority(&authority_capture.bytes, manifest.journal_schema)?;

    Ok(RestoreVerification {
        bundle: Digest::compute(Domain::JournalBackupManifest, &manifest_bytes),
        wire: manifest.wire,
        journal_schema: manifest.journal_schema,
        authority: authority_capture.digest,
        members: count(manifest.members.len(), "members")?,
        facts: count(verified.facts, "facts")?,
        receipts: count(verified.receipts, "receipts")?,
        heads: count(verified.heads, "heads")?,
        threads: count(verified.threads, "threads")?,
    })
}

fn admit_root(root: &Path) -> Result<(), BackupVerifyError> {
    if metadata(root)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(BackupVerifyError::Invalid {
            subject: root.to_path_buf(),
            detail: "backup bundle root is not a real directory".to_owned(),
        })
    }
}

fn read_manifest(root: &Path) -> Result<(JournalBackupManifest, Vec<u8>), BackupVerifyError> {
    let path = root.join(MANIFEST);
    let bytes = capture_regular_file(&path)?.bytes;
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

fn admit_manifest_versions(manifest: &JournalBackupManifest) -> Result<(), BackupVerifyError> {
    if manifest.version != JOURNAL_BACKUP_VERSION {
        Err(BackupVerifyError::UnsupportedVersion {
            found: manifest.version,
            current: JOURNAL_BACKUP_VERSION,
        })
    } else if !manifest.wire.is_current() {
        Err(BackupVerifyError::UnsupportedWire {
            found: manifest.wire,
            current: WireVersion::CURRENT,
        })
    } else if manifest.journal_schema > SCHEMA_VERSION {
        Err(BackupVerifyError::ForwardSchema {
            found: manifest.journal_schema,
            current: SCHEMA_VERSION,
        })
    } else if manifest.journal_schema != SCHEMA_VERSION {
        Err(BackupVerifyError::SchemaMismatch {
            manifest: manifest.journal_schema,
            authority: SCHEMA_VERSION,
        })
    } else {
        Ok(())
    }
}

fn authenticate_members(
    root: &Path,
    manifest: &JournalBackupManifest,
    authority: &Path,
) -> Result<CapturedMember, BackupVerifyError> {
    admit_inventory(root, manifest)?;
    let mut previous: Option<&str> = None;
    let mut authority_capture = None;
    for member in &manifest.members {
        let relative = admitted_member_path(&member.path)?;
        admit_member_order(root, previous, &member.path)?;
        previous = Some(&member.path);
        let observed =
            authenticate_member(root, &relative, member.mode, member.bytes, &member.digest)?;
        if relative == authority {
            authority_capture = Some(observed);
        }
    }
    authority_capture.ok_or_else(|| BackupVerifyError::Incomplete {
        detail: format!(
            "authority member {:?} is absent from the manifest inventory",
            manifest.authority
        ),
    })
}

fn admit_inventory(root: &Path, manifest: &JournalBackupManifest) -> Result<(), BackupVerifyError> {
    let inventory = bundle_inventory(root)?;
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
    root: &Path,
    relative: &Path,
    declared_mode: u32,
    declared_len: u64,
    declared_digest: &Digest,
) -> Result<CapturedMember, BackupVerifyError> {
    let path = root.join(relative);
    if declared_mode & !0o7777 != 0 {
        return Err(BackupVerifyError::Invalid {
            subject: root.join(MANIFEST),
            detail: format!("member mode {declared_mode:#o} is outside the POSIX 0o7777 mask"),
        });
    }
    let captured = capture_regular_file(&path)?;
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
) -> Result<crate::Verification, BackupVerifyError> {
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
    reader.verify().map_err(BackupVerifyError::Replay)
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

fn capture_regular_file(path: &Path) -> Result<CapturedFile, BackupVerifyError> {
    let before = metadata(path)?;
    require_regular(path, &before)?;
    let expected = FileIdentity::from_metadata(&before);
    let mut file = open_capture(path)?;
    let opened = file.metadata().map_err(|source| BackupVerifyError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    require_regular(path, &opened)?;
    if FileIdentity::from_metadata(&opened) != expected {
        return changed_during_capture(path);
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| BackupVerifyError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    let after_handle = file.metadata().map_err(|source| BackupVerifyError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let after_path = metadata(path)?;
    require_regular(path, &after_handle)?;
    require_regular(path, &after_path)?;
    if FileIdentity::from_metadata(&after_handle) != expected
        || FileIdentity::from_metadata(&after_path) != expected
        || u64::try_from(bytes.len()).ok() != Some(expected.len)
    {
        return changed_during_capture(path);
    }

    Ok(CapturedFile {
        mode: posix_mode(path, &after_handle)?,
        bytes,
    })
}

fn open_capture(path: &Path) -> Result<File, BackupVerifyError> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path).map_err(|source| BackupVerifyError::Io {
        path: path.to_path_buf(),
        source,
    })
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

#[cfg(unix)]
fn posix_mode(_path: &Path, metadata: &Metadata) -> Result<u32, BackupVerifyError> {
    use std::os::unix::fs::MetadataExt;

    Ok(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn posix_mode(path: &Path, _metadata: &Metadata) -> Result<u32, BackupVerifyError> {
    Err(BackupVerifyError::Invalid {
        subject: path.to_path_buf(),
        detail: "backup member POSIX mode is unavailable on this platform".to_owned(),
    })
}

fn metadata(path: &Path) -> Result<Metadata, BackupVerifyError> {
    fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            BackupVerifyError::Missing {
                path: path.to_path_buf(),
            }
        } else {
            BackupVerifyError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    })
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

fn bundle_inventory(root: &Path) -> Result<Vec<String>, BackupVerifyError> {
    let mut inventory = Vec::new();
    walk(root, Path::new(""), &mut inventory)?;
    inventory.sort();
    Ok(inventory)
}

fn walk(
    root: &Path,
    relative: &Path,
    inventory: &mut Vec<String>,
) -> Result<(), BackupVerifyError> {
    let directory = root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .map_err(|source| BackupVerifyError::Io {
            path: directory.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| BackupVerifyError::Io {
            path: directory,
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| BackupVerifyError::Invalid {
                subject: entry.path(),
                detail: "backup member name is not UTF-8".to_owned(),
            })?;
        let child = relative.join(name);
        let kind = metadata(&root.join(&child))?.file_type();
        if kind.is_dir() {
            walk(root, &child, inventory)?;
        } else if kind.is_file() {
            let spelling = child
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.join("/"))
                .ok_or_else(|| BackupVerifyError::Invalid {
                    subject: root.join(&child),
                    detail: "backup member path is not UTF-8".to_owned(),
                })?;
            if spelling != MANIFEST {
                inventory.push(spelling);
            }
        } else {
            return Err(BackupVerifyError::Invalid {
                subject: root.join(&child),
                detail: "backup inventory contains a symlink or special file".to_owned(),
            });
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
            Self::Replay(error) => write!(formatter, "backup replay closure failed: {error}"),
        }
    }
}

impl Error for BackupVerifyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Authority(error) => Some(error),
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

    use super::verify_restore_bundle_after_capture;

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
