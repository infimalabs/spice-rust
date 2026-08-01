//! Read the two exact repository configuration documents and resolve them.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

use wire::ConfigSource;

#[cfg(unix)]
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(not(unix))]
use std::fs;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::Read as _;
#[cfg(unix)]
use std::path::Component;

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

/// One admitted configuration load together with the exact documents that
/// produced it.
///
/// Keeping the acquired text beside the resolved tree lets trust and other
/// admission surfaces bind the same bytes that were parsed. Callers do not
/// need to reopen either path and race a replacement after validation.
#[derive(Debug)]
pub struct Loaded {
    resolved: Resolved,
    locations: Locations,
    tracked: Option<String>,
    local: Option<String>,
    generation: CaptureGeneration,
}

/// Filesystem generation of the two documents in one configuration capture.
///
/// The fields stay opaque so callers can compare captures without treating a
/// pathname, timestamp, or content digest as sufficient authority by itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureGeneration {
    tracked: DocumentGeneration,
    local: DocumentGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DocumentGeneration {
    root: DirectoryIdentity,
    directories: Vec<(OsString, DirectoryIdentity)>,
    relative: PathBuf,
    state: DocumentState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DocumentState {
    Missing { component: usize },
    Present(FileIdentity),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
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
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

#[derive(Debug)]
struct Acquired {
    text: Option<String>,
    generation: DocumentGeneration,
}

#[cfg(unix)]
#[derive(Debug)]
struct OpenParent {
    root: DirectoryIdentity,
    components: Vec<OsString>,
    directories: Vec<File>,
    generations: Vec<(OsString, DirectoryIdentity)>,
}

#[cfg(unix)]
#[derive(Debug)]
enum ParentCapture {
    Missing(Acquired),
    Open(OpenParent),
}

impl Loaded {
    /// Effective typed configuration resolved from the captured documents.
    #[must_use]
    pub const fn resolved(&self) -> &Resolved {
        &self.resolved
    }

    /// Exact repository configuration locations used for this capture.
    #[must_use]
    pub const fn locations(&self) -> &Locations {
        &self.locations
    }

    /// Exact tracked document text, absent when the path did not exist.
    #[must_use]
    pub fn tracked(&self) -> Option<&str> {
        self.tracked.as_deref()
    }

    /// Exact machine-local document text, absent when the path did not exist.
    #[must_use]
    pub fn local(&self) -> Option<&str> {
        self.local.as_deref()
    }

    /// Exact root, parent, and document generation used by this capture.
    #[must_use]
    pub const fn generation(&self) -> &CaptureGeneration {
        &self.generation
    }

    /// Consume the capture and retain only the effective typed configuration.
    #[must_use]
    pub fn into_resolved(self) -> Resolved {
        self.resolved
    }
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
    capture_repository(repository, flags).map(Loaded::into_resolved)
}

/// Capture the exact repository documents and their effective configuration.
///
/// # Errors
///
/// Returns every acquisition and resolution diagnostic found after both exact
/// document paths have been inspected.
pub fn capture_repository(
    repository: &Repository,
    flags: &[FlagOverride],
) -> Result<Loaded, LoadError> {
    capture_repository_with(repository, flags, false)
}

/// Load configuration while the Python scaffold still owns tracked task and
/// gate-policy tables in the same `spice.toml`.
///
/// This mode ignores exactly the tracked root tables `tasks` and `policy`.
/// Their bytes never enter the Rust typed tree, local configuration remains
/// strict, and every other unknown path still refuses. Release-one admission
/// stays available through [`load_repository`] without this interregnum rule.
///
/// # Errors
///
/// Returns every ordinary acquisition and resolution diagnostic, including
/// unknown roots other than the two named scaffold tables.
pub fn load_coexisting_repository(
    repository: &Repository,
    flags: &[FlagOverride],
) -> Result<Resolved, LoadError> {
    capture_coexisting_repository(repository, flags).map(Loaded::into_resolved)
}

/// Capture exact repository documents while applying the Python scaffold
/// coexistence rule to the effective configuration.
///
/// The returned documents and generation still cover every byte in both
/// configuration files, including tracked tables omitted from the typed tree.
///
/// # Errors
///
/// Returns every ordinary acquisition and resolution diagnostic, including
/// unknown roots other than the two named scaffold tables.
pub fn capture_coexisting_repository(
    repository: &Repository,
    flags: &[FlagOverride],
) -> Result<Loaded, LoadError> {
    capture_repository_with(repository, flags, true)
}

fn capture_repository_with(
    repository: &Repository,
    flags: &[FlagOverride],
    coexist: bool,
) -> Result<Loaded, LoadError> {
    let locations = locations(repository);
    let tracked_path = locations.tracked();
    let local_path = locations.local();

    let mut acquisition = Vec::new();
    let tracked = acquire(
        ConfigSource::Tracked,
        repository.worktree_root(),
        Path::new(TRACKED_FILE),
        &mut acquisition,
    );
    let local = acquire(
        ConfigSource::Local,
        repository.common_dir(),
        Path::new(LOCAL_FILE),
        &mut acquisition,
    );
    let tracked_location = tracked_path.display().to_string();
    let local_location = local_path.display().to_string();
    let tracked_document = tracked
        .as_ref()
        .and_then(|document| document.text.as_deref())
        .map(|text| Document::new(&tracked_location, text));
    let local_document = local
        .as_ref()
        .and_then(|document| document.text.as_deref())
        .map(|text| Document::new(&local_location, text));
    let inputs = if coexist {
        Inputs::coexisting(tracked_document, local_document, flags)
    } else {
        Inputs::new(tracked_document, local_document, flags)
    };
    let resolved = resolve(inputs);

    match (acquisition.is_empty(), resolved) {
        (true, Ok(resolved)) => {
            let tracked = tracked.expect("a successful tracked acquisition has a generation");
            let local = local.expect("a successful local acquisition has a generation");
            Ok(Loaded {
                resolved,
                locations,
                tracked: tracked.text,
                local: local.text,
                generation: CaptureGeneration {
                    tracked: tracked.generation,
                    local: local.generation,
                },
            })
        }
        (_, result) => Err(LoadError::configuration(acquisition, result.err())),
    }
}

fn acquire(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
    issues: &mut Vec<AcquisitionIssue>,
) -> Option<Acquired> {
    match acquire_document(source, root, relative) {
        Ok(document) => Some(document),
        Err(issue) => {
            issues.push(issue);
            None
        }
    }
}

#[cfg(unix)]
fn acquire_document(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
) -> Result<Acquired, AcquisitionIssue> {
    acquire_document_with(source, root, relative, || {})
}

#[cfg(unix)]
fn acquire_document_with<F>(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
    before_read: F,
) -> Result<Acquired, AcquisitionIssue>
where
    F: FnOnce(),
{
    match walk_to_document_parent(source, root, relative)? {
        ParentCapture::Missing(document) => Ok(document),
        ParentCapture::Open(parent) => {
            acquire_from_parent(source, root, relative, parent, before_read)
        }
    }
}

#[cfg(unix)]
fn walk_to_document_parent(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
) -> Result<ParentCapture, AcquisitionIssue> {
    let root_file = open_root(source, root)?;
    let root_identity = DirectoryIdentity::from_metadata(
        &root_file
            .metadata()
            .map_err(|error| AcquisitionIssue::unreadable(source, root, error))?,
    );

    let components = relative
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect::<Vec<_>>();
    let parent_count = components
        .len()
        .checked_sub(1)
        .expect("configuration document paths have one file name");
    let mut directories = vec![root_file];
    let mut directory_generations = Vec::new();

    for index in 0..parent_count {
        let component = &components[index];
        let display = root.join(components.iter().take(index + 1).collect::<PathBuf>());
        let file_type = component_type(
            directories.last().expect("configuration root is open"),
            component,
        )
        .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?;
        if file_type.is_none() {
            return missing_acquired(
                source,
                root,
                relative,
                directories,
                components,
                root_identity,
                directory_generations,
                index,
            )
            .map(ParentCapture::Missing);
        }
        require_directory(source, &display, file_type.expect("component exists"))?;
        let directory = open_directory_at(
            directories.last().expect("configuration parent is open"),
            component,
        )
        .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?;
        let identity = DirectoryIdentity::from_metadata(
            &directory
                .metadata()
                .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?,
        );
        directory_generations.push((component.clone(), identity));
        directories.push(directory);
    }

    Ok(ParentCapture::Open(OpenParent {
        root: root_identity,
        components,
        directories,
        generations: directory_generations,
    }))
}

#[cfg(unix)]
fn acquire_from_parent<F>(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
    parent: OpenParent,
    before_read: F,
) -> Result<Acquired, AcquisitionIssue>
where
    F: FnOnce(),
{
    let missing = parent
        .components
        .len()
        .checked_sub(1)
        .expect("configuration document paths have one file name");
    let (file_name, _) = parent
        .components
        .split_last()
        .expect("configuration document paths have one file name");
    let path = root.join(relative);
    let file_type = component_type(
        parent
            .directories
            .last()
            .expect("configuration parent is open"),
        file_name,
    )
    .map_err(|error| AcquisitionIssue::unreadable(source, &path, error))?;
    let Some(file_type) = file_type else {
        return missing_acquired(
            source,
            root,
            relative,
            parent.directories,
            parent.components,
            parent.root,
            parent.generations,
            missing,
        );
    };
    require_regular_file(source, &path, file_type)?;
    let opened_parent = parent
        .directories
        .last()
        .expect("configuration parent is open");
    let (bytes, identity) = read_opened_file(source, &path, opened_parent, file_name, before_read)?;
    let generation = DocumentGeneration {
        root: parent.root,
        directories: parent.generations,
        relative: relative.to_path_buf(),
        state: DocumentState::Present(identity),
    };
    verify_named_directories(source, root, &generation)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| AcquisitionIssue::shape(source, &path, AcquisitionKind::NonUtf8))?;
    Ok(Acquired {
        text: Some(text),
        generation,
    })
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn missing_acquired(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
    directories: Vec<File>,
    components: Vec<OsString>,
    root_identity: DirectoryIdentity,
    directory_generations: Vec<(OsString, DirectoryIdentity)>,
    missing: usize,
) -> Result<Acquired, AcquisitionIssue> {
    let generation = DocumentGeneration {
        root: root_identity,
        directories: directory_generations,
        relative: relative.to_path_buf(),
        state: DocumentState::Missing { component: missing },
    };
    verify_missing(
        source,
        root,
        &directories,
        &components,
        missing,
        &generation,
    )?;
    Ok(Acquired {
        text: None,
        generation,
    })
}

#[cfg(unix)]
fn require_directory(
    source: ConfigSource,
    path: &Path,
    file_type: FileType,
) -> Result<(), AcquisitionIssue> {
    match file_type {
        FileType::Directory => Ok(()),
        FileType::Symlink => Err(AcquisitionIssue::shape(
            source,
            path,
            AcquisitionKind::Symlink,
        )),
        _ => Err(AcquisitionIssue::shape(
            source,
            path,
            AcquisitionKind::NonRegular,
        )),
    }
}

#[cfg(unix)]
fn require_regular_file(
    source: ConfigSource,
    path: &Path,
    file_type: FileType,
) -> Result<(), AcquisitionIssue> {
    let kind = match file_type {
        FileType::RegularFile => return Ok(()),
        FileType::Symlink => AcquisitionKind::Symlink,
        FileType::Directory => AcquisitionKind::Directory,
        _ => AcquisitionKind::NonRegular,
    };
    Err(AcquisitionIssue::shape(source, path, kind))
}

#[cfg(unix)]
fn read_opened_file<F>(
    source: ConfigSource,
    path: &Path,
    parent: &File,
    file_name: &OsStr,
    before_read: F,
) -> Result<(Vec<u8>, FileIdentity), AcquisitionIssue>
where
    F: FnOnce(),
{
    let mut file = open_file_at(parent, file_name)
        .map_err(|error| AcquisitionIssue::unreadable(source, path, error))?;
    let opened = file
        .metadata()
        .map_err(|error| AcquisitionIssue::unreadable(source, path, error))?;
    if !opened.is_file() {
        return Err(AcquisitionIssue::shape(
            source,
            path,
            AcquisitionKind::NonRegular,
        ));
    }
    let identity = FileIdentity::from_metadata(&opened);
    before_read();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AcquisitionIssue::unreadable(source, path, error))?;
    let after = file
        .metadata()
        .map_err(|error| AcquisitionIssue::unreadable(source, path, error))?;
    let named = open_file_at(parent, file_name)
        .and_then(|file| file.metadata())
        .map_err(|error| AcquisitionIssue::unreadable(source, path, error))?;
    if FileIdentity::from_metadata(&after) != identity
        || FileIdentity::from_metadata(&named) != identity
        || u64::try_from(bytes.len()).ok() != Some(identity.len)
    {
        return Err(generation_moved(source, path));
    }
    Ok((bytes, identity))
}

#[cfg(not(unix))]
fn acquire_document(
    source: ConfigSource,
    root: &Path,
    relative: &Path,
) -> Result<Acquired, AcquisitionIssue> {
    let path = root.join(relative);
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| AcquisitionIssue::unreadable(source, root, error))?;
    if !root_metadata.is_dir() {
        return Err(AcquisitionIssue::shape(
            source,
            root,
            AcquisitionKind::NonRegular,
        ));
    }
    let root_identity = DirectoryIdentity::from_metadata(&root_metadata);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Acquired {
                text: None,
                generation: DocumentGeneration {
                    root: root_identity,
                    directories: Vec::new(),
                    relative: relative.to_path_buf(),
                    state: DocumentState::Missing { component: 0 },
                },
            });
        }
        Err(error) => return Err(AcquisitionIssue::unreadable(source, &path, error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(AcquisitionIssue::shape(
            source,
            &path,
            AcquisitionKind::Symlink,
        ));
    }
    if metadata.is_dir() {
        return Err(AcquisitionIssue::shape(
            source,
            &path,
            AcquisitionKind::Directory,
        ));
    }
    if !metadata.is_file() {
        return Err(AcquisitionIssue::shape(
            source,
            &path,
            AcquisitionKind::NonRegular,
        ));
    }
    let bytes =
        fs::read(&path).map_err(|error| AcquisitionIssue::unreadable(source, &path, error))?;
    let identity = FileIdentity::from_metadata(&metadata);
    let text = String::from_utf8(bytes)
        .map_err(|_| AcquisitionIssue::shape(source, &path, AcquisitionKind::NonUtf8))?;
    Ok(Acquired {
        text: Some(text),
        generation: DocumentGeneration {
            root: root_identity,
            directories: Vec::new(),
            relative: relative.to_path_buf(),
            state: DocumentState::Present(identity),
        },
    })
}

#[cfg(unix)]
fn verify_missing(
    source: ConfigSource,
    root: &Path,
    directories: &[File],
    components: &[OsString],
    missing: usize,
    generation: &DocumentGeneration,
) -> Result<(), AcquisitionIssue> {
    verify_named_directories(source, root, generation)?;
    let parent = directories
        .last()
        .expect("a missing configuration component retains its parent");
    let display = root.join(components.iter().take(missing + 1).collect::<PathBuf>());
    match component_type(parent, &components[missing])
        .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?
    {
        None => Ok(()),
        Some(_) => Err(generation_moved(source, &display)),
    }
}

#[cfg(unix)]
fn verify_named_directories(
    source: ConfigSource,
    root: &Path,
    generation: &DocumentGeneration,
) -> Result<(), AcquisitionIssue> {
    let mut current = open_root(source, root)?;
    let observed = DirectoryIdentity::from_metadata(
        &current
            .metadata()
            .map_err(|error| AcquisitionIssue::unreadable(source, root, error))?,
    );
    if observed != generation.root {
        return Err(generation_moved(source, root));
    }
    let mut display = root.to_path_buf();
    for (component, expected) in &generation.directories {
        display.push(component);
        let reopened = open_directory_at(&current, component)
            .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?;
        let observed = DirectoryIdentity::from_metadata(
            &reopened
                .metadata()
                .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?,
        );
        if &observed != expected {
            return Err(generation_moved(source, &display));
        }
        current = reopened;
    }
    Ok(())
}

#[cfg(unix)]
fn component_type(parent: &File, component: &OsStr) -> io::Result<Option<FileType>> {
    match rustix::fs::statat(parent, component, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn open_root(source: ConfigSource, path: &Path) -> Result<File, AcquisitionIssue> {
    let mut directory = rustix::fs::open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(io::Error::from)
    .map_err(|error| AcquisitionIssue::unreadable(source, Path::new("/"), error))?;
    let mut display = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                display.push(name);
                let file_type = component_type(&directory, name)
                    .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?;
                match file_type {
                    Some(FileType::Directory) => {}
                    Some(FileType::Symlink) => {
                        return Err(AcquisitionIssue::shape(
                            source,
                            &display,
                            AcquisitionKind::Symlink,
                        ));
                    }
                    Some(_) => {
                        return Err(AcquisitionIssue::shape(
                            source,
                            &display,
                            AcquisitionKind::NonRegular,
                        ));
                    }
                    None => {
                        return Err(AcquisitionIssue::unreadable(
                            source,
                            &display,
                            io::Error::new(
                                io::ErrorKind::NotFound,
                                "configuration root component is missing",
                            ),
                        ));
                    }
                }
                directory = open_directory_at(&directory, name)
                    .map_err(|error| AcquisitionIssue::unreadable(source, &display, error))?;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(AcquisitionIssue::unreadable(
                    source,
                    path,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("configuration root is not canonical: {}", path.display()),
                    ),
                ));
            }
        }
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_directory_at(parent: &File, component: &OsStr) -> io::Result<File> {
    rustix::fs::openat(
        parent,
        component,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(Into::into)
}

#[cfg(unix)]
fn open_file_at(parent: &File, component: &OsStr) -> io::Result<File> {
    rustix::fs::openat(
        parent,
        component,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(Into::into)
}

impl DirectoryIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;

        Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(not(unix))]
            modified: metadata.modified().ok(),
            #[cfg(not(unix))]
            created: metadata.created().ok(),
        }
    }
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;

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
            #[cfg(not(unix))]
            modified: metadata.modified().ok(),
            #[cfg(not(unix))]
            created: metadata.created().ok(),
        }
    }
}

#[cfg(unix)]
fn generation_moved(source: ConfigSource, path: &Path) -> AcquisitionIssue {
    AcquisitionIssue::unreadable(
        source,
        path,
        io::Error::other("configuration generation moved during capture"),
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        env, fs as file_system,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn replacement_while_an_open_file_is_read_refuses_the_whole_capture() {
        let root = env::temp_dir().join(format!(
            "spice-config-capture-{}-{}",
            std::process::id(),
            NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed)
        ));
        file_system::create_dir(&root).expect("create capture fixture");
        let admitted_root = file_system::canonicalize(&root).expect("canonical capture fixture");
        let path = admitted_root.join(TRACKED_FILE);
        let displaced = admitted_root.join("displaced.toml");
        let replacement = admitted_root.join("replacement.toml");
        file_system::write(&path, b"[listen]\nwindow = \"2m\"\n").expect("write original");
        file_system::write(&replacement, b"[listen]\nwindow = \"2m\"\n")
            .expect("write same-byte replacement");

        let error = acquire_document_with(
            ConfigSource::Tracked,
            &admitted_root,
            Path::new(TRACKED_FILE),
            || {
                file_system::rename(&path, &displaced).expect("displace opened generation");
                file_system::rename(&replacement, &path).expect("publish replacement generation");
            },
        )
        .expect_err("read-time replacement must refuse");

        assert_eq!(error.kind(), AcquisitionKind::Unreadable);
        assert_eq!(error.path(), path);
        assert_eq!(
            file_system::read(&displaced).expect("read displaced generation"),
            file_system::read(&path).expect("read replacement generation")
        );
        file_system::remove_dir_all(root).expect("remove capture fixture");
    }
}
