use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest};
use wire::{Digest, Domain, RestoreVerification};

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
    let root = root.as_ref();
    admit_root(root)?;
    let (manifest, manifest_bytes) = read_manifest(root)?;
    admit_manifest_versions(&manifest)?;
    let authority = admitted_member_path(&manifest.authority)?;
    let authority_digest = authenticate_members(root, &manifest, &authority)?;
    let verified = verify_authority(root, &authority, manifest.journal_schema)?;

    Ok(RestoreVerification {
        bundle: Digest::compute(Domain::JournalBackupManifest, &manifest_bytes),
        journal_schema: manifest.journal_schema,
        authority: authority_digest,
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
    let bytes = regular_file_bytes(&path)?;
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
) -> Result<Digest, BackupVerifyError> {
    admit_inventory(root, manifest)?;
    let mut previous: Option<&str> = None;
    let mut authority_digest = None;
    for member in &manifest.members {
        let relative = admitted_member_path(&member.path)?;
        admit_member_order(root, previous, &member.path)?;
        previous = Some(&member.path);
        let observed = authenticate_member(root, &relative, member.bytes, &member.digest)?;
        if relative == authority {
            authority_digest = Some(observed);
        }
    }
    authority_digest.ok_or_else(|| BackupVerifyError::Incomplete {
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
    declared_len: u64,
    declared_digest: &Digest,
) -> Result<Digest, BackupVerifyError> {
    let path = root.join(relative);
    let bytes = regular_file_bytes(&path)?;
    let observed_len = u64::try_from(bytes.len()).map_err(|error| BackupVerifyError::Invalid {
        subject: path.clone(),
        detail: format!("member length cannot cross as u64: {error}"),
    })?;
    if declared_len != observed_len {
        return Err(BackupVerifyError::Invalid {
            subject: path,
            detail: format!("member length declares {declared_len}, observed {observed_len}"),
        });
    }
    let observed = Digest::compute(Domain::JournalBackupMember, &bytes);
    if *declared_digest == observed {
        Ok(observed)
    } else {
        Err(BackupVerifyError::Invalid {
            subject: path,
            detail: format!("member digest declares {declared_digest}, observed {observed}"),
        })
    }
}

fn verify_authority(
    root: &Path,
    authority: &Path,
    manifest_schema: u32,
) -> Result<crate::Verification, BackupVerifyError> {
    let reader = JournalReader::open(root.join(authority)).map_err(BackupVerifyError::Authority)?;
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

fn metadata(path: &Path) -> Result<fs::Metadata, BackupVerifyError> {
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

fn regular_file_bytes(path: &Path) -> Result<Vec<u8>, BackupVerifyError> {
    if !metadata(path)?.file_type().is_file() {
        return Err(BackupVerifyError::Invalid {
            subject: path.to_path_buf(),
            detail: "backup member is not a real regular file".to_owned(),
        });
    }
    fs::read(path).map_err(|source| BackupVerifyError::Io {
        path: path.to_path_buf(),
        source,
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
    Missing { path: PathBuf },
    Incomplete { detail: String },
    UnsupportedVersion { found: u16, current: u16 },
    ForwardSchema { found: u32, current: u32 },
    SchemaMismatch { manifest: u32, authority: u32 },
    Invalid { subject: PathBuf, detail: String },
    Io { path: PathBuf, source: io::Error },
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
            | Self::ForwardSchema { .. }
            | Self::SchemaMismatch { .. }
            | Self::Invalid { .. } => None,
        }
    }
}
