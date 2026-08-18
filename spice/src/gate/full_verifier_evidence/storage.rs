//! Versioned at-rest storage for repository-common verifier evidence.
//!
//! One repository-common lock serializes forward migration, publication,
//! admission reads, and grooming. The current release reads exactly one
//! integrity-checked container. A missing format marker authorizes one
//! idempotent sweep from recognized plain or raw-gzip documents; the marker is
//! published only after every rewrite succeeds.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::repository::StatePaths;

use super::MergeParentEvidence;

mod container;
mod retention;

#[cfg(test)]
mod tests;

const ARCHIVE_LOCK_NAME: &str = "full-verifier-evidence-archive";
const ARCHIVE_LOCK_FILE: &str = "full-verifier-evidence-archive.lock";
const FORMAT_MARKER_NAME: &str = "FORMAT";
const FORMAT_MARKER: &[u8] = b"spice/full-verifier-archive/v1\n";
/// Hex length of a bundle stem: archived names are one source digest each.
pub(super) const BUNDLE_STEM_HEX: usize = 64;

/// Exclusive access to the repository-common archive.
pub(super) struct LockedArchive {
    root: PathBuf,
    _possession: sill::Possession,
}

impl LockedArchive {
    /// Acquire the common archive lock and finish the current-release migration.
    pub(super) fn acquire(paths: &StatePaths) -> Result<Self, String> {
        fs::create_dir_all(paths.locks()).map_err(|error| {
            format!(
                "create full-verifier archive lock directory {}: {error}",
                paths.locks().display(),
            )
        })?;
        fs::create_dir_all(paths.full_verifier_evidence_archive()).map_err(|error| {
            format!(
                "create full-verifier archive {}: {error}",
                paths.full_verifier_evidence_archive().display(),
            )
        })?;
        Self::acquire_at(
            paths.full_verifier_evidence_archive(),
            &paths.locks().join(ARCHIVE_LOCK_FILE),
        )
    }

    fn acquire_at(root: &Path, lock_path: &Path) -> Result<Self, String> {
        let archive = Self::lock_at(root, lock_path)?;
        archive.ensure_current()?;
        Ok(archive)
    }

    fn lock_at(root: &Path, lock_path: &Path) -> Result<Self, String> {
        let name = sill::LockName::new(ARCHIVE_LOCK_NAME)
            .expect("the archive lock name is one constant path component");
        let possession = sill::LockFile::new(name, lock_path)
            .acquire()
            .map_err(|error| format!("acquire full-verifier archive lock: {error}"))?;
        Ok(Self {
            root: root.to_path_buf(),
            _possession: possession,
        })
    }

    /// Read one current bundle while the common lock prevents peer grooming.
    pub(super) fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, String> {
        let stored = match fs::read(path) {
            Ok(stored) => stored,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("read bundle {}: {error}", path.display())),
        };
        decode_current_bundle(&stored)
            .map(Some)
            .map_err(|error| format!("decode bundle {}: {error}", path.display()))
    }

    /// Publish one current bundle and apply a fully validated retention plan.
    pub(super) fn publish_and_groom(
        &self,
        source: &str,
        plain: &[u8],
        protected: &BTreeSet<String>,
    ) -> Result<(), String> {
        validate_plain_bundle(plain)?;
        let stored = container::encode(plain)?;
        let destination = self.root.join(format!("{source}.json"));
        let plan = retention::plan(&self.root, &destination, &stored, protected)?;

        publish_bytes(&destination, &stored, "publish full-verifier archive")?;
        for path in plan.evictions() {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("evict {}: {error}", path.display())),
            }
        }
        retention::verify_bounds(&self.root)
    }

    fn ensure_current(&self) -> Result<(), String> {
        let marker = self.root.join(FORMAT_MARKER_NAME);
        match fs::read(&marker) {
            Ok(document) if document == FORMAT_MARKER => return Ok(()),
            Ok(document) => {
                return Err(format!(
                    "archive format marker {} is unsupported: {:?}",
                    marker.display(),
                    String::from_utf8_lossy(&document),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "read archive format marker {}: {error}",
                    marker.display(),
                ));
            }
        }
        self.migrate_with(|_| Ok(()))
    }

    fn migrate_with(
        &self,
        mut after_rewrite: impl FnMut(&Path) -> Result<(), String>,
    ) -> Result<(), String> {
        let rewrites = migration_plan(&self.root)?;
        for (path, stored, modified) in rewrites {
            publish_migration_bytes(&path, &stored, modified)?;
            after_rewrite(&path)?;
        }
        publish_bytes(
            &self.root.join(FORMAT_MARKER_NAME),
            FORMAT_MARKER,
            "publish full-verifier archive format marker",
        )
    }
}

fn migration_plan(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>, SystemTime)>, String> {
    let listing = fs::read_dir(root)
        .map_err(|error| format!("list archive {} for migration: {error}", root.display()))?;
    let mut rewrites = Vec::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("read archive entry in {}: {error}", root.display(),));
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_bundle_name(name) {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "inspect migration bundle {}: {error}",
                    path.display(),
                ));
            }
        };
        if !metadata.file_type().is_file() {
            return Err(format!(
                "archive migration refuses non-regular bundle {}",
                path.display(),
            ));
        }
        let modified = metadata.modified().map_err(|error| {
            format!(
                "read migration bundle modification time {}: {error}",
                path.display(),
            )
        })?;
        let document = match fs::read(&path) {
            Ok(document) => document,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("read migration bundle {}: {error}", path.display(),));
            }
        };
        if container::is_current(&document) {
            decode_current_bundle(&document)
                .map_err(|error| format!("validate current bundle {}: {error}", path.display()))?;
            continue;
        }
        let plain = container::decode_pre_container(&document)
            .map_err(|error| format!("decode pre-container bundle {}: {error}", path.display()))?;
        validate_plain_bundle(&plain).map_err(|error| {
            format!("validate pre-container bundle {}: {error}", path.display())
        })?;
        let stored = container::encode(&plain)
            .map_err(|error| format!("encode current bundle {}: {error}", path.display()))?;
        rewrites.push((path, stored, modified));
    }
    rewrites.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(rewrites)
}

fn decode_current_bundle(stored: &[u8]) -> Result<Vec<u8>, String> {
    let plain = container::decode(stored)?;
    validate_plain_bundle(&plain)?;
    Ok(plain)
}

fn validate_plain_bundle(document: &[u8]) -> Result<(), String> {
    MergeParentEvidence::decode(document).map(|_| ())
}

fn publish_bytes(path: &Path, bytes: &[u8], action: &str) -> Result<(), String> {
    atomic::Destination::new(path)
        .map_err(|error| format!("resolve {} for {action}: {error}", path.display()))?
        .publish(|staged| staged.write_all(bytes))
        .map_err(|error| format!("{action} {}: {error}", path.display()))
}

fn publish_migration_bytes(path: &Path, bytes: &[u8], modified: SystemTime) -> Result<(), String> {
    let destination = atomic::Destination::new(path)
        .map_err(|error| format!("resolve {} for archive migration: {error}", path.display()))?;
    let mut staged = destination
        .stage()
        .map_err(|error| format!("stage archive migration {}: {error}", path.display()))?;
    staged
        .write_all(bytes)
        .map_err(|error| format!("write archive migration {}: {error}", path.display()))?;
    fs::File::options()
        .write(true)
        .open(staged.temporary_path())
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(modified)))
        .map_err(|error| {
            format!(
                "preserve archive migration modification time {}: {error}",
                path.display(),
            )
        })?;
    staged
        .commit()
        .map_err(|error| format!("publish archive migration {}: {error}", path.display()))
}

pub(super) fn is_bundle_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    stem.len() == BUNDLE_STEM_HEX
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
