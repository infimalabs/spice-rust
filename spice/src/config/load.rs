//! Read the two exact repository configuration documents and resolve them.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use wire::ConfigSource;

use super::{Document, FlagOverride, Inputs, ResolveError, Resolved, resolve};
use crate::repository::{Repository, ResolveError as RepositoryError};

const TRACKED_FILE: &str = "spice.toml";
const LOCAL_FILE: &str = ".spice/config.toml";

/// Exact repository configuration locations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Locations {
    tracked: PathBuf,
    local: PathBuf,
}

impl Locations {
    /// Tracked worktree policy document.
    #[must_use]
    pub fn tracked(&self) -> &Path {
        &self.tracked
    }

    /// Machine-local Git-common document.
    #[must_use]
    pub fn local(&self) -> &Path {
        &self.local
    }
}

/// Derive the two configuration paths from the canonical repository layout.
#[must_use]
pub fn locations(repository: &Repository) -> Locations {
    Locations {
        tracked: repository.worktree_root().join(TRACKED_FILE),
        local: repository.common_dir().join(LOCAL_FILE),
    }
}

/// Why one existing configuration path could not become document text.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AcquisitionKind {
    /// The exact configuration path is a symbolic link.
    Symlink,
    /// The exact configuration path is a directory.
    Directory,
    /// The exact configuration path is another non-regular object.
    NonRegular,
    /// Metadata or content could not be read.
    Unreadable,
    /// The regular file does not contain UTF-8.
    NonUtf8,
}

/// One typed refusal acquiring an exact configuration path.
#[derive(Debug)]
pub struct AcquisitionIssue {
    source: ConfigSource,
    path: PathBuf,
    kind: AcquisitionKind,
    cause: Option<io::Error>,
}

impl AcquisitionIssue {
    /// Layer whose exact path was inspected.
    #[must_use]
    pub const fn source(&self) -> ConfigSource {
        self.source
    }

    /// Exact path that could not become document text.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stable acquisition refusal category.
    #[must_use]
    pub const fn kind(&self) -> AcquisitionKind {
        self.kind
    }

    /// Underlying filesystem error for an unreadable path.
    #[must_use]
    pub const fn io_error(&self) -> Option<&io::Error> {
        self.cause.as_ref()
    }

    fn shape(source: ConfigSource, path: &Path, kind: AcquisitionKind) -> Self {
        Self {
            source,
            path: path.to_path_buf(),
            kind,
            cause: None,
        }
    }

    fn unreadable(source: ConfigSource, path: &Path, cause: io::Error) -> Self {
        Self {
            source,
            path: path.to_path_buf(),
            kind: AcquisitionKind::Unreadable,
            cause: Some(cause),
        }
    }
}

impl fmt::Display for AcquisitionIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} configuration at {} is {}",
            self.source,
            self.path.display(),
            match self.kind {
                AcquisitionKind::Symlink => "a symlink",
                AcquisitionKind::Directory => "a directory",
                AcquisitionKind::NonRegular => "not a regular file",
                AcquisitionKind::Unreadable => "unreadable",
                AcquisitionKind::NonUtf8 => "not UTF-8",
            }
        )?;
        if let Some(cause) = &self.cause {
            write!(formatter, ": {cause}")?;
        }
        Ok(())
    }
}

/// Every reason one repository configuration load was refused.
#[derive(Debug)]
pub struct LoadError {
    repository: Option<RepositoryError>,
    acquisition: Vec<AcquisitionIssue>,
    resolution: Option<ResolveError>,
}

impl LoadError {
    /// Repository discovery failure, before document paths were known.
    #[must_use]
    pub const fn repository(&self) -> Option<&RepositoryError> {
        self.repository.as_ref()
    }

    /// Acquisition failures in tracked then local order.
    #[must_use]
    pub fn acquisition(&self) -> &[AcquisitionIssue] {
        &self.acquisition
    }

    /// Accumulated registry and domain diagnostics, when any were found.
    #[must_use]
    pub const fn resolution(&self) -> Option<&ResolveError> {
        self.resolution.as_ref()
    }

    fn repository_failure(source: RepositoryError) -> Self {
        Self {
            repository: Some(source),
            acquisition: Vec::new(),
            resolution: None,
        }
    }

    fn configuration(acquisition: Vec<AcquisitionIssue>, resolution: Option<ResolveError>) -> Self {
        Self {
            repository: None,
            acquisition,
            resolution,
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(repository) = &self.repository {
            return write!(
                formatter,
                "configuration repository resolution failed: {repository}"
            );
        }
        let resolution = self
            .resolution
            .as_ref()
            .map_or(0, |error| error.issues().len());
        write!(
            formatter,
            "configuration load refused with {} acquisition and {resolution} resolution issue(s)",
            self.acquisition.len()
        )
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let Some(repository) = &self.repository {
            return Some(repository);
        }
        if let Some(cause) = self.acquisition.iter().find_map(AcquisitionIssue::io_error) {
            return Some(cause);
        }
        self.resolution
            .as_ref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

/// Load effective configuration for the repository containing `start`.
///
/// Acquisition inspects exactly the tracked worktree document and the local
/// Git-common document. Missing documents are empty intent.
///
/// # Errors
///
/// Returns repository discovery failure, or every acquisition and resolution
/// diagnostic found after both exact document paths have been inspected.
pub fn load(start: impl AsRef<Path>, flags: &[FlagOverride]) -> Result<Resolved, LoadError> {
    let repository = Repository::discover(start).map_err(LoadError::repository_failure)?;
    load_repository(&repository, flags)
}

/// Load effective configuration from an already resolved repository.
///
/// # Errors
///
/// Returns every acquisition and resolution diagnostic found after both exact
/// document paths have been inspected.
pub fn load_repository(
    repository: &Repository,
    flags: &[FlagOverride],
) -> Result<Resolved, LoadError> {
    let locations = locations(repository);
    let tracked_path = locations.tracked();
    let local_path = locations.local();

    let mut acquisition = Vec::new();
    let tracked = acquire(ConfigSource::Tracked, tracked_path, &mut acquisition);
    let local = acquire(ConfigSource::Local, local_path, &mut acquisition);
    let tracked_location = tracked_path.display().to_string();
    let local_location = local_path.display().to_string();
    let resolved = resolve(Inputs::new(
        tracked
            .as_deref()
            .map(|text| Document::new(&tracked_location, text)),
        local
            .as_deref()
            .map(|text| Document::new(&local_location, text)),
        flags,
    ));

    match (acquisition.is_empty(), resolved) {
        (true, Ok(resolved)) => Ok(resolved),
        (_, result) => Err(LoadError::configuration(acquisition, result.err())),
    }
}

fn acquire(
    source: ConfigSource,
    path: &Path,
    issues: &mut Vec<AcquisitionIssue>,
) -> Option<String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            issues.push(AcquisitionIssue::unreadable(source, path, error));
            return None;
        }
    };
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        Some(AcquisitionKind::Symlink)
    } else if file_type.is_dir() {
        Some(AcquisitionKind::Directory)
    } else if !file_type.is_file() {
        Some(AcquisitionKind::NonRegular)
    } else {
        None
    };
    if let Some(kind) = kind {
        issues.push(AcquisitionIssue::shape(source, path, kind));
        return None;
    }

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            issues.push(AcquisitionIssue::unreadable(source, path, error));
            return None;
        }
    };
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(_) => {
            issues.push(AcquisitionIssue::shape(
                source,
                path,
                AcquisitionKind::NonUtf8,
            ));
            None
        }
    }
}
