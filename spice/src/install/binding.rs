use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::path::{relative_path, validate_relative};
use super::{InstallError, Location};

/// Stable native identity of one observed filesystem object.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub device: u64,
    pub inode: u64,
}

impl Generation {
    pub(super) fn observe(metadata: &fs::Metadata) -> Self {
        generation(metadata)
    }
}

/// Canonical existing ancestor that anchors a destination parent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentObservation {
    pub(super) ancestor: String,
    pub(super) generation: Generation,
    pub(super) missing: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BoundDestination {
    pub(super) destination: String,
    pub(super) parent: ParentObservation,
    pub(super) introduced_parents: Vec<String>,
}

pub(super) fn bind_destination(
    root: &Path,
    relative: &str,
) -> Result<BoundDestination, InstallError> {
    let relative = validate_relative(relative)?;
    let file_name = relative
        .file_name()
        .expect("a strict relative file path has a file name");
    let logical_parent = root
        .join(relative)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            InstallError::InvalidRecord("install destination has no parent".to_owned())
        })?;
    let mut candidate = logical_parent.as_path();
    let canonical = loop {
        match fs::symlink_metadata(candidate) {
            Ok(_) => {
                break fs::canonicalize(candidate).map_err(|source| {
                    InstallError::io("resolve destination parent", candidate, source)
                })?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = candidate
                    .parent()
                    .ok_or_else(|| InstallError::PathOutsideRoot(logical_parent.clone()))?;
            }
            Err(source) => {
                return Err(InstallError::io(
                    "inspect destination parent",
                    candidate,
                    source,
                ));
            }
        }
    };
    let root = fs::canonicalize(root)
        .map_err(|source| InstallError::io("resolve install root", root, source))?;
    if !canonical.starts_with(&root) {
        return Err(InstallError::PathOutsideRoot(canonical));
    }
    let metadata = fs::metadata(&canonical).map_err(|source| {
        InstallError::io("inspect resolved destination parent", &canonical, source)
    })?;
    if !metadata.is_dir() {
        return Err(InstallError::PathShape(canonical));
    }
    let missing = logical_parent.strip_prefix(candidate).map_err(|_| {
        InstallError::InvalidRecord("destination parent suffix is not relative".to_owned())
    })?;
    let physical_parent = canonical.join(missing);
    let destination = physical_parent.join(file_name);
    if !destination.starts_with(&root) {
        return Err(InstallError::PathOutsideRoot(destination));
    }
    let ancestor = relative_or_empty(&canonical, &root)?;
    let missing = relative_or_empty(missing, Path::new(""))?;
    let destination = relative_path(
        destination
            .strip_prefix(&root)
            .map_err(|_| InstallError::PathOutsideRoot(destination.clone()))?,
    )?;
    let introduced_parents = introduced(&ancestor, &missing)?;
    Ok(BoundDestination {
        destination,
        parent: ParentObservation {
            ancestor,
            generation: Generation::observe(&metadata),
            missing,
        },
        introduced_parents,
    })
}

pub(super) fn validate_binding(
    root: &Path,
    logical: &str,
    destination: &str,
    parent: &ParentObservation,
) -> Result<(), InstallError> {
    validate_relative(logical)?;
    validate_relative(destination)?;
    validate_optional_relative(&parent.ancestor)?;
    validate_optional_relative(&parent.missing)?;
    let root = fs::canonicalize(root)
        .map_err(|source| InstallError::io("resolve install root", root, source))?;
    let ancestor = root.join(&parent.ancestor);
    let metadata = fs::metadata(&ancestor).map_err(|source| {
        InstallError::io("inspect bound destination parent", &ancestor, source)
    })?;
    if !metadata.is_dir() || Generation::observe(&metadata) != parent.generation {
        return Err(InstallError::ParentChanged(ancestor));
    }
    let expected_parent = ancestor.join(&parent.missing);
    if root.join(destination).parent() != Some(expected_parent.as_path()) {
        return Err(InstallError::InvalidRecord(
            "bound destination does not close over its parent observation".to_owned(),
        ));
    }
    let current = bind_destination(&root, logical)?;
    if current.destination != destination {
        return Err(InstallError::ParentChanged(root.join(logical)));
    }
    Ok(())
}

pub(super) fn validate_record_binding(
    root: &Path,
    binding: &BoundDestination,
    logical: &str,
) -> Result<(), InstallError> {
    validate_binding(root, logical, &binding.destination, &binding.parent)
}

pub(super) fn validate_operation_binding(
    repository: &crate::repository::Repository,
    location: Location,
    logical: &str,
    destination: &str,
    parent: &ParentObservation,
) -> Result<(), InstallError> {
    validate_binding(location.root(repository), logical, destination, parent)
}

fn introduced(ancestor: &str, missing: &str) -> Result<Vec<String>, InstallError> {
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let mut current = PathBuf::from(ancestor);
    let mut paths = Vec::new();
    for component in validate_relative(missing)?.components() {
        current.push(component);
        paths.push(relative_path(&current)?);
    }
    Ok(paths)
}

fn relative_or_empty(path: &Path, root: &Path) -> Result<String, InstallError> {
    let relative = if root.as_os_str().is_empty() {
        path
    } else {
        path.strip_prefix(root).map_err(|_| {
            InstallError::InvalidRecord("bound path is not below its root".to_owned())
        })?
    };
    if relative.as_os_str().is_empty() {
        Ok(String::new())
    } else {
        relative_path(relative)
    }
}

fn validate_optional_relative(path: &str) -> Result<(), InstallError> {
    if path.is_empty() {
        Ok(())
    } else {
        validate_relative(path).map(|_| ())
    }
}

#[cfg(unix)]
fn generation(metadata: &fs::Metadata) -> Generation {
    use std::os::unix::fs::MetadataExt;
    Generation {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn generation(metadata: &fs::Metadata) -> Generation {
    use std::time::UNIX_EPOCH;
    let changed = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos() as u64);
    Generation {
        device: metadata.len(),
        inode: changed,
    }
}
