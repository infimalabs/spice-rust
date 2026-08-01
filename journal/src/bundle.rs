//! Offline admission of enumerated backup bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::{JournalReader, SCHEMA_VERSION};

/// Current backup-bundle manifest schema.
pub const BUNDLE_VERSION: u16 = 1;
/// Canonical manifest filename at the bundle root.
pub const MANIFEST_PATH: &str = "manifest.json";
/// Sole authority filename inside every bundle.
pub const JOURNAL_PATH: &str = "data/spicejournal.sqlite3";

/// One exact file enumerated by a backup bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFile {
    pub path: String,
    pub bytes: u64,
    pub digest: Digest,
}

/// Canonical inventory and schema identity carried by a backup bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub version: u16,
    pub journal_schema: u32,
    pub files: Vec<BundleFile>,
    pub tape: Vec<String>,
}

/// Facts established by a complete, zero-write bundle verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BundleVerification {
    pub bundle: PathBuf,
    pub identity: Digest,
    pub journal_schema: u32,
    pub files: usize,
    pub bytes: u64,
    pub tape_files: usize,
}

/// Verify one directory bundle without changing it.
///
/// # Errors
///
/// Refuses noncanonical manifests, unsafe or divergent inventories, checksum
/// drift, tape-manifest disagreement, and any journal outside the current
/// finalized schema. Filesystem failures remain distinct from invalid bundle
/// content so the binary rim can classify them as environmental faults.
pub fn verify_bundle(bundle: impl AsRef<Path>) -> Result<BundleVerification, BundleError> {
    let bundle = canonical_bundle(bundle.as_ref())?;
    let manifest_path = bundle.join(MANIFEST_PATH);
    let bytes = read_regular(&manifest_path)?;
    let manifest: BundleManifest =
        wire::decode(&bytes).map_err(|error| BundleError::ManifestDecode(error.to_string()))?;
    let canonical =
        wire::encode(&manifest).map_err(|error| BundleError::ManifestEncode(error.to_string()))?;
    if canonical != bytes {
        return Err(BundleError::ManifestNoncanonical);
    }
    admit_manifest(&manifest)?;
    let found = inventory(&bundle)?;
    verify_inventory(&bundle, &manifest, &found)?;
    let schema = JournalReader::open(bundle.join(JOURNAL_PATH))
        .and_then(|reader| reader.schema_version())
        .map_err(|error| BundleError::Journal(error.to_string()))?;
    if schema != manifest.journal_schema || schema != SCHEMA_VERSION {
        return Err(BundleError::JournalSchema {
            declared: manifest.journal_schema,
            found: schema,
            current: SCHEMA_VERSION,
        });
    }
    Ok(BundleVerification {
        bundle,
        identity: Digest::compute(Domain::RestoreBundleManifest, &canonical),
        journal_schema: schema,
        files: manifest.files.len(),
        bytes: manifest.files.iter().map(|file| file.bytes).sum(),
        tape_files: manifest.tape.len(),
    })
}

fn canonical_bundle(bundle: &Path) -> Result<PathBuf, BundleError> {
    let metadata = fs::symlink_metadata(bundle)
        .map_err(|source| BundleError::io("inspect bundle", bundle, source))?;
    if !metadata.file_type().is_dir() {
        return Err(BundleError::NotDirectory(bundle.to_path_buf()));
    }
    fs::canonicalize(bundle).map_err(|source| BundleError::io("resolve bundle", bundle, source))
}

fn admit_manifest(manifest: &BundleManifest) -> Result<(), BundleError> {
    if manifest.version != BUNDLE_VERSION {
        return Err(BundleError::Version(manifest.version));
    }
    if manifest.journal_schema != SCHEMA_VERSION {
        return Err(BundleError::DeclaredSchema {
            declared: manifest.journal_schema,
            current: SCHEMA_VERSION,
        });
    }
    strictly_sorted(
        manifest.files.iter().map(|file| file.path.as_str()),
        "file inventory",
    )?;
    strictly_sorted(manifest.tape.iter().map(String::as_str), "tape manifest")?;
    for file in &manifest.files {
        validate_relative(&file.path)?;
    }
    let journals = manifest
        .files
        .iter()
        .filter(|file| file.path == JOURNAL_PATH)
        .count();
    if journals != 1 {
        return Err(BundleError::JournalCount(journals));
    }
    let tape = manifest
        .files
        .iter()
        .filter(|file| file.path.starts_with("tape/"))
        .map(|file| file.path.as_str());
    if !tape.eq(manifest.tape.iter().map(String::as_str)) {
        return Err(BundleError::TapeManifest);
    }
    Ok(())
}

fn strictly_sorted<'a>(
    values: impl Iterator<Item = &'a str>,
    name: &'static str,
) -> Result<(), BundleError> {
    let mut previous: Option<&str> = None;
    for value in values {
        if previous.is_some_and(|prior| prior >= value) {
            return Err(BundleError::Unsorted(name));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_relative(text: &str) -> Result<(), BundleError> {
    if text.is_empty() || text.contains('\\') {
        return Err(BundleError::UnsafePath(text.to_owned()));
    }
    let path = Path::new(text);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path.to_str() != Some(text)
    {
        return Err(BundleError::UnsafePath(text.to_owned()));
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<BTreeSet<String>, BundleError> {
    let mut found = BTreeSet::new();
    walk(root, root, &mut found)?;
    found.remove(MANIFEST_PATH);
    Ok(found)
}

fn walk(root: &Path, directory: &Path, found: &mut BTreeSet<String>) -> Result<(), BundleError> {
    let entries = fs::read_dir(directory)
        .map_err(|source| BundleError::io("read bundle directory", directory, source))?;
    for entry in entries {
        let entry =
            entry.map_err(|source| BundleError::io("read bundle entry", directory, source))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|source| BundleError::io("inspect bundle entry", &path, source))?;
        if kind.is_dir() {
            walk(root, &path, found)?;
        } else if kind.is_file() {
            found.insert(relative(root, &path)?);
        } else {
            return Err(BundleError::NonRegular(path));
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> Result<String, BundleError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BundleError::UnsafePath(path.display().to_string()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(BundleError::UnsafePath(relative.display().to_string()));
        };
        parts.push(
            part.to_str()
                .ok_or_else(|| BundleError::UnsafePath(relative.display().to_string()))?,
        );
    }
    Ok(parts.join("/"))
}

fn verify_inventory(
    root: &Path,
    manifest: &BundleManifest,
    found: &BTreeSet<String>,
) -> Result<(), BundleError> {
    let listed = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let expected = listed.keys().cloned().collect::<BTreeSet<_>>();
    if expected != *found {
        return Err(BundleError::Inventory {
            expected,
            found: found.clone(),
        });
    }
    for (path, entry) in listed {
        let bytes = read_regular(&root.join(&path))?;
        if bytes.len() as u64 != entry.bytes {
            return Err(BundleError::Length {
                path,
                expected: entry.bytes,
                found: bytes.len() as u64,
            });
        }
        let digest = Digest::compute(Domain::RestoreBundleFile, &bytes);
        if digest != entry.digest {
            return Err(BundleError::Digest {
                path,
                expected: entry.digest.clone(),
                found: digest,
            });
        }
    }
    Ok(())
}

fn read_regular(path: &Path) -> Result<Vec<u8>, BundleError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| BundleError::io("inspect bundle file", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(BundleError::NonRegular(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| BundleError::io("read bundle file", path, source))
}

/// Why a backup bundle could not be admitted for restore.
#[derive(Debug)]
pub enum BundleError {
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    NotDirectory(PathBuf),
    NonRegular(PathBuf),
    ManifestDecode(String),
    ManifestEncode(String),
    ManifestNoncanonical,
    Version(u16),
    DeclaredSchema {
        declared: u32,
        current: u32,
    },
    JournalSchema {
        declared: u32,
        found: u32,
        current: u32,
    },
    Journal(String),
    Unsorted(&'static str),
    UnsafePath(String),
    JournalCount(usize),
    TapeManifest,
    Inventory {
        expected: BTreeSet<String>,
        found: BTreeSet<String>,
    },
    Length {
        path: String,
        expected: u64,
        found: u64,
    },
    Digest {
        path: String,
        expected: Digest,
        found: Digest,
    },
}

impl BundleError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }

    /// Whether verification failed before bundle bytes could be interpreted.
    #[must_use]
    pub const fn is_environment(&self) -> bool {
        matches!(self, Self::Io { .. })
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} at {}: {source}", path.display()),
            Self::NotDirectory(path) => write!(
                formatter,
                "bundle is not a real directory: {}",
                path.display()
            ),
            Self::NonRegular(path) => write!(
                formatter,
                "bundle entry is not a regular file or directory: {}",
                path.display()
            ),
            Self::ManifestDecode(error) => {
                write!(formatter, "bundle manifest does not decode: {error}")
            }
            Self::ManifestEncode(error) => write!(
                formatter,
                "bundle manifest cannot be canonically encoded: {error}"
            ),
            Self::ManifestNoncanonical => {
                formatter.write_str("bundle manifest bytes are not canonical")
            }
            Self::Version(version) => write!(
                formatter,
                "bundle manifest version {version} is not {BUNDLE_VERSION}"
            ),
            Self::DeclaredSchema { declared, current } => write!(
                formatter,
                "bundle declares journal schema {declared}, not current schema {current}"
            ),
            Self::JournalSchema {
                declared,
                found,
                current,
            } => write!(
                formatter,
                concat!(
                    "bundle journal schema {} disagrees with declaration ",
                    "{} or current schema {}"
                ),
                found, declared, current,
            ),
            Self::Journal(error) => write!(
                formatter,
                "bundle journal is not a finalized authority: {error}"
            ),
            Self::Unsorted(name) => {
                write!(formatter, "bundle {name} is not strictly sorted and unique")
            }
            Self::UnsafePath(path) => write!(
                formatter,
                "bundle path is not one safe relative spelling: {path:?}"
            ),
            Self::JournalCount(count) => write!(
                formatter,
                "bundle inventory names {count} journal files instead of one"
            ),
            Self::TapeManifest => {
                formatter.write_str("bundle tape manifest does not equal the enumerated tape files")
            }
            Self::Inventory { expected, found } => write!(
                formatter,
                "bundle filenames disagree: expected {expected:?}, found {found:?}"
            ),
            Self::Length {
                path,
                expected,
                found,
            } => write!(
                formatter,
                "bundle file {path:?} has {found} bytes, expected {expected}"
            ),
            Self::Digest {
                path,
                expected,
                found,
            } => write!(
                formatter,
                "bundle file {path:?} digest {found} disagrees with {expected}"
            ),
        }
    }
}

impl Error for BundleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
