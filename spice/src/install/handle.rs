use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(debug_assertions)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use super::path::validate_relative;
use super::{DIRECTORY_MODE, FileImage, Generation, InstallError, ObservedFile, ParentObservation};

#[cfg(all(unix, any(target_vendor = "apple", target_os = "linux")))]
use rustix::fs::RenameFlags;
#[cfg(unix)]
use rustix::fs::{self as unix_fs, AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(debug_assertions)]
static MUTATION_RENDEZVOUS: OnceLock<Option<MutationRendezvous>> = OnceLock::new();

#[cfg(debug_assertions)]
#[derive(Debug)]
struct MutationRendezvous {
    control: PathBuf,
    action: OsString,
    path: PathBuf,
}

#[cfg(unix)]
struct OpenedParents {
    directories: Vec<File>,
    ready: bool,
    preexisting_introduced: bool,
}

/// One destination whose parent is held open from its admitted root.
pub(super) struct AdmittedPath {
    canonical_root: PathBuf,
    display: PathBuf,
    file_name: OsString,
    components: Vec<OsString>,
    directories: Vec<File>,
    ready: bool,
    introduced: usize,
    preexisting_introduced: bool,
}

impl AdmittedPath {
    pub(super) fn open(
        root: &Path,
        destination: &str,
        observation: &ParentObservation,
        create_missing: bool,
        introduced: usize,
    ) -> Result<Self, InstallError> {
        #[cfg(unix)]
        {
            Self::open_unix(root, destination, observation, create_missing, introduced)
        }
        #[cfg(not(unix))]
        {
            let _ = (root, destination, observation, create_missing, introduced);
            Err(InstallError::InvalidRecord(
                "handle-bound installation requires a Unix filesystem".to_owned(),
            ))
        }
    }

    #[cfg(unix)]
    fn open_unix(
        root: &Path,
        destination: &str,
        observation: &ParentObservation,
        create_missing: bool,
        introduced: usize,
    ) -> Result<Self, InstallError> {
        let destination_path = validate_relative(destination)?;
        let file_name = destination_path
            .file_name()
            .expect("a strict destination has a file name")
            .to_os_string();
        let expected_parent = joined_parent(observation)?;
        if destination_path.parent() != Some(expected_parent.as_path()) {
            return Err(InstallError::InvalidRecord(
                "bound destination does not close over its admitted parent".to_owned(),
            ));
        }
        let canonical_root = fs::canonicalize(root)
            .map_err(|source| InstallError::io("resolve admitted install root", root, source))?;
        let root_fd = unix_fs::open(
            &canonical_root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|source| {
            InstallError::io("open admitted install root", &canonical_root, source.into())
        })?;
        let components = path_components(&expected_parent)?;
        let anchor_components = path_components(Path::new(&observation.ancestor))?.len();
        let first_introduced = components.len().checked_sub(introduced).ok_or_else(|| {
            InstallError::InvalidRecord(
                "introduced install parents exceed the bound parent depth".to_owned(),
            )
        })?;
        if create_missing && first_introduced < anchor_components {
            return Err(InstallError::InvalidRecord(
                "introduced install parents overlap the admitted ancestor".to_owned(),
            ));
        }
        let opened = open_parent_chain(
            &canonical_root,
            root_fd,
            &components,
            observation,
            create_missing,
            anchor_components,
            first_introduced,
        )?;
        if anchor_components == 0 {
            require_directory_generation(&opened.directories[0], &canonical_root, observation)?;
        }
        Ok(Self {
            canonical_root,
            display: root.join(destination_path),
            file_name,
            components,
            directories: opened.directories,
            ready: opened.ready,
            introduced,
            preexisting_introduced: opened.preexisting_introduced,
        })
    }

    pub(super) fn observe(&self) -> Result<Option<ObservedFile>, InstallError> {
        if !self.ready {
            return Ok(None);
        }
        self.verify_named_parent()?;
        #[cfg(unix)]
        {
            let descriptor = match unix_fs::openat(
                self.parent(),
                &self.file_name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(descriptor) => descriptor,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(source) => {
                    return Err(InstallError::io(
                        "open admitted install file",
                        &self.display,
                        source.into(),
                    ));
                }
            };
            let mut file = File::from(descriptor);
            let metadata = file.metadata().map_err(|source| {
                InstallError::io("inspect admitted install file", &self.display, source)
            })?;
            if !metadata.is_file() {
                return Err(InstallError::PathShape(self.display.clone()));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|source| {
                InstallError::io("read admitted install file", &self.display, source)
            })?;
            Ok(Some(ObservedFile {
                image: FileImage::new(metadata.permissions().mode() & 0o777, bytes),
                generation: Generation::observe(&metadata),
            }))
        }
        #[cfg(not(unix))]
        unreachable!("construction refuses non-Unix filesystems")
    }

    pub(super) fn parent_generation(&self) -> Result<Option<Generation>, InstallError> {
        if !self.ready {
            return Ok(None);
        }
        self.verify_named_parent()?;
        let metadata = self.parent().metadata().map_err(|source| {
            InstallError::io("inspect admitted destination parent", &self.display, source)
        })?;
        if !metadata.is_dir() {
            return Err(InstallError::PathShape(
                self.display
                    .parent()
                    .expect("a strict destination has a parent")
                    .to_path_buf(),
            ));
        }
        Ok(Some(Generation::observe(&metadata)))
    }

    pub(super) fn publish(
        &self,
        image: &FileImage,
        expected: Option<Generation>,
    ) -> Result<Generation, InstallError> {
        self.publish_with(image, expected, || mutation_pause("publish", &self.display))
    }

    fn publish_with(
        &self,
        image: &FileImage,
        expected: Option<Generation>,
        before_mutation: impl FnOnce() -> Result<(), InstallError>,
    ) -> Result<Generation, InstallError> {
        if !self.ready {
            return Err(InstallError::ParentChanged(self.display.clone()));
        }
        if expected.is_none() && self.preexisting_introduced {
            return Err(InstallError::ParentChanged(self.display.clone()));
        }
        self.require_generation(expected)?;
        #[cfg(unix)]
        {
            let temporary = temporary_name();
            let descriptor = unix_fs::openat(
                self.parent(),
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|source| {
                InstallError::io("create admitted staged file", &self.display, source.into())
            })?;
            let mut staged = File::from(descriptor);
            let result = (|| {
                staged.write_all(&image.bytes).map_err(|source| {
                    InstallError::io("write admitted staged file", &self.display, source)
                })?;
                staged
                    .set_permissions(fs::Permissions::from_mode(image.mode))
                    .map_err(|source| {
                        InstallError::io("set admitted staged mode", &self.display, source)
                    })?;
                staged.sync_all().map_err(|source| {
                    InstallError::io("synchronize admitted staged file", &self.display, source)
                })?;
                let staged_generation =
                    Generation::observe(&staged.metadata().map_err(|source| {
                        InstallError::io("inspect admitted staged file", &self.display, source)
                    })?);
                self.verify_named_parent()?;
                self.require_generation(expected)?;
                before_mutation()?;
                self.admit_staged_name(&temporary, expected, staged_generation)?;
                if expected.is_some() {
                    remove_name(self.parent(), &temporary, AtFlags::empty(), &self.display)?;
                }
                self.parent().sync_all().map_err(|source| {
                    InstallError::io(
                        "synchronize admitted destination parent",
                        &self.display,
                        source,
                    )
                })?;
                Ok(staged_generation)
            })();
            if result.is_err() {
                let _ = unix_fs::unlinkat(self.parent(), &temporary, AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        unreachable!("construction refuses non-Unix filesystems")
    }

    /// Put the staged name in the destination's place, or leave neither moved.
    ///
    /// Both arms verify the parent and the names after moving, and both undo
    /// their own move when that verification refuses, so a caller observes
    /// either the admitted destination or exactly the state it started from.
    /// They differ only in whether a destination is expected to be there: an
    /// expected one is exchanged, leaving its prior image under the temporary
    /// name for the caller to remove, while an unexpected one must not be
    /// replaced at all.
    #[cfg(unix)]
    fn admit_staged_name(
        &self,
        temporary: &OsString,
        expected: Option<Generation>,
        staged_generation: Generation,
    ) -> Result<(), InstallError> {
        match expected {
            Some(expected) => changed_on_name_race(
                exchange(
                    self.parent(),
                    temporary,
                    self.parent(),
                    &self.file_name,
                    &self.display,
                ),
                &self.display,
            )
            .and_then(|()| {
                let admitted = self.verify_named_parent().and_then(|()| {
                    self.require_named_generation(&self.file_name, staged_generation)?;
                    self.require_named_generation(temporary, expected)
                });
                if let Err(error) = admitted {
                    exchange(
                        self.parent(),
                        temporary,
                        self.parent(),
                        &self.file_name,
                        &self.display,
                    )?;
                    return Err(error);
                }
                Ok(())
            }),
            None => changed_on_name_race(
                rename_noreplace(
                    self.parent(),
                    temporary,
                    self.parent(),
                    &self.file_name,
                    &self.display,
                ),
                &self.display,
            )
            .and_then(|()| {
                let admitted = self.verify_named_parent().and_then(|()| {
                    self.require_named_generation(&self.file_name, staged_generation)
                });
                if let Err(error) = admitted {
                    rename_noreplace(
                        self.parent(),
                        &self.file_name,
                        self.parent(),
                        temporary,
                        &self.display,
                    )?;
                    return Err(error);
                }
                Ok(())
            }),
        }
    }

    pub(super) fn remove(&self, expected: Generation) -> Result<(), InstallError> {
        self.remove_with(expected, || mutation_pause("remove", &self.display))
    }

    fn remove_with(
        &self,
        expected: Generation,
        before_mutation: impl FnOnce() -> Result<(), InstallError>,
    ) -> Result<(), InstallError> {
        self.require_generation(Some(expected))?;
        self.verify_named_parent()?;
        #[cfg(unix)]
        {
            let temporary = temporary_name();
            before_mutation()?;
            changed_on_name_race(
                rename_noreplace(
                    self.parent(),
                    &self.file_name,
                    self.parent(),
                    &temporary,
                    &self.display,
                ),
                &self.display,
            )?;
            let admitted = self
                .verify_named_parent()
                .and_then(|()| self.require_named_generation(&temporary, expected));
            if let Err(error) = admitted {
                rename_noreplace(
                    self.parent(),
                    &temporary,
                    self.parent(),
                    &self.file_name,
                    &self.display,
                )?;
                return Err(error);
            }
            remove_name(self.parent(), &temporary, AtFlags::empty(), &self.display)?;
            self.parent().sync_all().map_err(|source| {
                InstallError::io(
                    "synchronize admitted destination parent",
                    &self.display,
                    source,
                )
            })?;
            Ok(())
        }
        #[cfg(not(unix))]
        unreachable!("construction refuses non-Unix filesystems")
    }

    pub(super) fn cleanup_introduced(&mut self) -> Result<(), InstallError> {
        if !self.ready {
            return Ok(());
        }
        #[cfg(unix)]
        {
            self.verify_named_parent()?;
            let first = self.components.len().saturating_sub(self.introduced);
            for index in (first..self.components.len()).rev() {
                let path = self
                    .display
                    .parent()
                    .expect("a strict destination has a parent")
                    .ancestors()
                    .nth(self.components.len() - index - 1)
                    .expect("an introduced directory is below the root")
                    .to_path_buf();
                self.cleanup_directory(index, &path)?;
            }
            Ok(())
        }
        #[cfg(not(unix))]
        unreachable!("construction refuses non-Unix filesystems")
    }

    fn require_generation(&self, expected: Option<Generation>) -> Result<(), InstallError> {
        let observed = self.observe()?.map(|observed| observed.generation);
        if observed == expected {
            Ok(())
        } else {
            Err(InstallError::ParentChanged(self.display.clone()))
        }
    }

    #[cfg(unix)]
    fn require_named_generation(
        &self,
        name: &OsString,
        expected: Generation,
    ) -> Result<(), InstallError> {
        let observed = generation_at(self.parent(), name, &self.display)?;
        if observed == Some(expected) {
            Ok(())
        } else {
            Err(InstallError::ParentChanged(self.display.clone()))
        }
    }

    #[cfg(unix)]
    fn cleanup_directory(&self, index: usize, path: &Path) -> Result<(), InstallError> {
        self.cleanup_directory_with(index, path, || mutation_pause("cleanup", path))
    }

    #[cfg(unix)]
    fn cleanup_directory_with(
        &self,
        index: usize,
        path: &Path,
        before_mutation: impl FnOnce() -> Result<(), InstallError>,
    ) -> Result<(), InstallError> {
        let parent = &self.directories[index];
        let component = &self.components[index];
        let expected =
            Generation::observe(&self.directories[index + 1].metadata().map_err(|source| {
                InstallError::io("inspect introduced directory", path, source)
            })?);
        let temporary = temporary_name();
        before_mutation()?;
        match rename_noreplace(parent, component, parent, &temporary, path) {
            Err(InstallError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                self.verify_named_prefix(index)?;
                return Ok(());
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        let admitted = self.verify_named_prefix(index).and_then(|()| {
            let observed = generation_at(parent, &temporary, path)?;
            if observed == Some(expected) {
                Ok(())
            } else {
                Err(InstallError::ParentChanged(path.to_path_buf()))
            }
        });
        if let Err(error) = admitted {
            rename_noreplace(parent, &temporary, parent, component, path)?;
            return Err(error);
        }
        match unix_fs::unlinkat(parent, &temporary, AtFlags::REMOVEDIR) {
            Ok(()) => parent.sync_all().map_err(|source| {
                InstallError::io("synchronize introduced directory parent", path, source)
            }),
            Err(rustix::io::Errno::NOTEMPTY) => {
                rename_noreplace(parent, &temporary, parent, component, path)
            }
            Err(source) => {
                rename_noreplace(parent, &temporary, parent, component, path)?;
                Err(InstallError::io(
                    "remove introduced directory",
                    path,
                    source.into(),
                ))
            }
        }
    }

    #[cfg(unix)]
    fn verify_named_parent(&self) -> Result<(), InstallError> {
        self.verify_named_prefix(self.components.len())
    }

    #[cfg(unix)]
    fn verify_named_prefix(&self, component_count: usize) -> Result<(), InstallError> {
        let root = unix_fs::open(
            &self.canonical_root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|source| {
            InstallError::io(
                "reopen admitted install root",
                &self.canonical_root,
                source.into(),
            )
        })?;
        if Generation::observe(&root.metadata().map_err(|source| {
            InstallError::io(
                "inspect reopened install root",
                &self.canonical_root,
                source,
            )
        })?) != Generation::observe(&self.directories[0].metadata().map_err(|source| {
            InstallError::io("inspect held install root", &self.canonical_root, source)
        })?) {
            return Err(InstallError::ParentChanged(self.canonical_root.clone()));
        }
        let mut current = root;
        for (index, component) in self.components.iter().take(component_count).enumerate() {
            let reopened = open_directory(&current, component)
                .map_err(|_| InstallError::ParentChanged(self.display.clone()))?;
            let reopened_generation =
                Generation::observe(&reopened.metadata().map_err(|source| {
                    InstallError::io("inspect reopened install directory", &self.display, source)
                })?);
            let held_generation =
                Generation::observe(&self.directories[index + 1].metadata().map_err(|source| {
                    InstallError::io("inspect held install directory", &self.display, source)
                })?);
            if reopened_generation != held_generation {
                return Err(InstallError::ParentChanged(self.display.clone()));
            }
            current = reopened;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn verify_named_parent(&self) -> Result<(), InstallError> {
        unreachable!("construction refuses non-Unix filesystems")
    }

    fn parent(&self) -> &File {
        self.directories
            .last()
            .expect("an admitted path retains its root directory")
    }
}

fn joined_parent(observation: &ParentObservation) -> Result<PathBuf, InstallError> {
    let mut parent = PathBuf::new();
    if !observation.ancestor.is_empty() {
        parent.push(validate_relative(&observation.ancestor)?);
    }
    if !observation.missing.is_empty() {
        parent.push(validate_relative(&observation.missing)?);
    }
    Ok(parent)
}

fn path_components(path: &Path) -> Result<Vec<OsString>, InstallError> {
    if path.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    Ok(validate_relative(path.to_string_lossy().as_ref())?
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect())
}

fn components_path(components: &[OsString]) -> PathBuf {
    components.iter().collect()
}

#[cfg(unix)]
fn open_parent_chain(
    canonical_root: &Path,
    root: File,
    components: &[OsString],
    observation: &ParentObservation,
    create_missing: bool,
    anchor_components: usize,
    first_introduced: usize,
) -> Result<OpenedParents, InstallError> {
    let mut directories = vec![root];
    let mut ready = true;
    let mut preexisting_introduced = false;
    for (index, component) in components.iter().enumerate() {
        let display = canonical_root.join(components_path(&components[..=index]));
        let parent = directories.last().expect("the root directory is open");
        match open_directory(parent, component) {
            Ok(directory) => {
                if create_missing && index >= first_introduced {
                    preexisting_introduced = true;
                }
                directories.push(directory);
            }
            Err(source)
                if source.kind() == io::ErrorKind::NotFound && index >= anchor_components =>
            {
                if !create_missing || preexisting_introduced {
                    ready = false;
                    break;
                }
                if index < first_introduced {
                    return Err(InstallError::ParentChanged(display));
                }
                let directory = create_directory(parent, component, &display, || {
                    mutation_pause("create", &display)
                })?;
                directories.push(directory);
            }
            Err(source) => {
                return Err(InstallError::io(
                    "open admitted install directory",
                    display,
                    source,
                ));
            }
        }
        if index + 1 == anchor_components {
            require_directory_generation(
                directories.last().expect("the admitted anchor is open"),
                &display,
                observation,
            )?;
        }
    }
    Ok(OpenedParents {
        directories,
        ready,
        preexisting_introduced,
    })
}

#[cfg(unix)]
fn require_directory_generation(
    directory: &File,
    display: &Path,
    observation: &ParentObservation,
) -> Result<(), InstallError> {
    let metadata = directory.metadata().map_err(|source| {
        InstallError::io("inspect admitted install directory", display, source)
    })?;
    if metadata.is_dir() && observation.generation.matches(&metadata) {
        Ok(())
    } else {
        Err(InstallError::ParentChanged(display.to_path_buf()))
    }
}

#[cfg(unix)]
fn open_directory(parent: &File, name: &OsString) -> io::Result<File> {
    unix_fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(Into::into)
}

#[cfg(unix)]
fn create_directory(
    parent: &File,
    name: &OsString,
    display: &Path,
    before_open: impl FnOnce() -> Result<(), InstallError>,
) -> Result<File, InstallError> {
    unix_fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE as _)).map_err(|source| {
        if source == rustix::io::Errno::EXIST {
            InstallError::ParentChanged(display.to_path_buf())
        } else {
            InstallError::io("create admitted install directory", display, source.into())
        }
    })?;
    let created = generation_at(parent, name, display)?
        .ok_or_else(|| InstallError::ParentChanged(display.to_path_buf()))?;
    before_open()?;
    let directory = open_directory(parent, name)
        .map_err(|source| InstallError::io("open newly created install directory", display, source))
        .or_else(|error| changed_on_name_race(Err(error), display))?;
    let opened = Generation::observe(&directory.metadata().map_err(|source| {
        InstallError::io("inspect newly created install directory", display, source)
    })?);
    if opened != created {
        return Err(InstallError::ParentChanged(display.to_path_buf()));
    }
    parent.sync_all().map_err(|source| {
        InstallError::io("synchronize admitted directory parent", display, source)
    })?;
    Ok(directory)
}

#[cfg(unix)]
fn generation_at(
    parent: &File,
    name: &OsString,
    display: &Path,
) -> Result<Option<Generation>, InstallError> {
    match unix_fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            #[cfg(target_vendor = "apple")]
            let device = stat.st_dev as u64;
            #[cfg(not(target_vendor = "apple"))]
            let device = stat.st_dev;
            Ok(Some(Generation {
                device,
                inode: stat.st_ino,
            }))
        }
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(source) => Err(InstallError::io(
            "inspect admitted install name",
            display,
            source.into(),
        )),
    }
}

#[cfg(unix)]
fn changed_on_name_race<T>(
    result: Result<T, InstallError>,
    display: &Path,
) -> Result<T, InstallError> {
    match result {
        Err(InstallError::Io { source, .. })
            if matches!(
                source.kind(),
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
            ) =>
        {
            Err(InstallError::ParentChanged(display.to_path_buf()))
        }
        other => other,
    }
}

#[cfg(unix)]
fn remove_name(
    parent: &File,
    name: &OsString,
    flags: AtFlags,
    display: &Path,
) -> Result<(), InstallError> {
    unix_fs::unlinkat(parent, name, flags)
        .map_err(|source| InstallError::io("remove admitted install name", display, source.into()))
}

#[cfg(all(unix, any(target_vendor = "apple", target_os = "linux")))]
fn rename_noreplace(
    old_parent: &File,
    old_name: &OsString,
    new_parent: &File,
    new_name: &OsString,
    display: &Path,
) -> Result<(), InstallError> {
    unix_fs::renameat_with(
        old_parent,
        old_name,
        new_parent,
        new_name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|source| {
        InstallError::io(
            "atomically move admitted install name",
            display,
            source.into(),
        )
    })
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "linux"))))]
fn rename_noreplace(
    _old_parent: &File,
    _old_name: &OsString,
    _new_parent: &File,
    _new_name: &OsString,
    display: &Path,
) -> Result<(), InstallError> {
    Err(InstallError::InvalidRecord(format!(
        "atomic install rename is unavailable for {}",
        display.display()
    )))
}

#[cfg(all(unix, any(target_vendor = "apple", target_os = "linux")))]
fn exchange(
    old_parent: &File,
    old_name: &OsString,
    new_parent: &File,
    new_name: &OsString,
    display: &Path,
) -> Result<(), InstallError> {
    unix_fs::renameat_with(
        old_parent,
        old_name,
        new_parent,
        new_name,
        RenameFlags::EXCHANGE,
    )
    .map_err(|source| {
        InstallError::io(
            "atomically exchange admitted install names",
            display,
            source.into(),
        )
    })
}

#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "linux"))))]
fn exchange(
    _old_parent: &File,
    _old_name: &OsString,
    _new_parent: &File,
    _new_name: &OsString,
    display: &Path,
) -> Result<(), InstallError> {
    Err(InstallError::InvalidRecord(format!(
        "atomic install exchange is unavailable for {}",
        display.display()
    )))
}

#[cfg(debug_assertions)]
fn mutation_pause(action: &str, display: &Path) -> Result<(), InstallError> {
    let Some(rendezvous) = MUTATION_RENDEZVOUS.get().and_then(Option::as_ref) else {
        return Ok(());
    };
    if rendezvous.action != action || rendezvous.path != display {
        return Ok(());
    }
    await_release(&rendezvous.control, action, display)
}

/// Block until the harness releases this pause, or the horizon is spent.
///
/// A release landed before the child starts remains an explicit file. Otherwise
/// the child binds a loopback listener before announcing readiness, then blocks
/// on one exact release token. The harness therefore cannot observe
/// readiness until the event receiver is armed, and no filesystem notification
/// or polling interval sits between the release and the resumed mutation.
#[cfg(debug_assertions)]
fn await_release(control: &Path, action: &str, display: &Path) -> Result<(), InstallError> {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread;
    use std::time::Duration;

    if control.join("release").join("continue").exists() {
        return signal_ready(control, action, None, display);
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|source| InstallError::io("bind install mutation rendezvous", display, source))?;
    let release_port = listener
        .local_addr()
        .map_err(|source| InstallError::io("inspect install mutation rendezvous", display, source))?
        .port();
    signal_ready(control, action, Some(release_port), display)?;

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = listener.accept().and_then(|(mut stream, _)| {
            let mut token = [0_u8; 8];
            stream.read_exact(&mut token)?;
            if token == *b"continue" {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "install rendezvous received the wrong release token",
                ))
            }
        });
        let _ = sender.send(result);
    });

    match receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(source)) => Err(InstallError::io(
            "receive install mutation rendezvous",
            display,
            source,
        )),
        Err(RecvTimeoutError::Timeout) => Err(InstallError::io(
            "wait for install mutation rendezvous",
            display,
            io::Error::new(io::ErrorKind::TimedOut, "test rendezvous timed out"),
        )),
        Err(RecvTimeoutError::Disconnected) => Err(InstallError::io(
            "wait for install mutation rendezvous",
            display,
            io::Error::new(io::ErrorKind::BrokenPipe, "test rendezvous disconnected"),
        )),
    }
}

#[cfg(debug_assertions)]
fn signal_ready(
    control: &Path,
    action: &str,
    release_port: Option<u16>,
    display: &Path,
) -> Result<(), InstallError> {
    use std::net::{Ipv4Addr, TcpStream};

    let ready_port = fs::read_to_string(control.join("ready").join("port"))
        .map_err(|source| InstallError::io("read install readiness rendezvous", display, source))?
        .parse::<u16>()
        .map_err(|source| {
            InstallError::io(
                "decode install readiness rendezvous",
                display,
                io::Error::new(io::ErrorKind::InvalidData, source),
            )
        })?;
    let mut ready = TcpStream::connect((Ipv4Addr::LOCALHOST, ready_port)).map_err(|source| {
        InstallError::io("connect install readiness rendezvous", display, source)
    })?;
    write!(ready, "{action} {}", release_port.unwrap_or_default())
        .map_err(|source| InstallError::io("signal install mutation rendezvous", display, source))
}

#[cfg(not(debug_assertions))]
fn mutation_pause(_action: &str, _display: &Path) -> Result<(), InstallError> {
    Ok(())
}

#[cfg(debug_assertions)]
pub(super) fn configure_mutation_rendezvous(
    control: Option<OsString>,
    action: Option<OsString>,
    path: Option<OsString>,
) {
    let rendezvous = match (control, action, path) {
        (Some(control), Some(action), Some(path)) => Some(MutationRendezvous {
            control: PathBuf::from(control),
            action,
            path: PathBuf::from(path),
        }),
        _ => None,
    };
    MUTATION_RENDEZVOUS
        .set(rendezvous)
        .expect("install mutation rendezvous is configured once at startup");
}

fn temporary_name() -> OsString {
    OsString::from(format!(
        ".spice-install-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn renamed_parent_at_publication_is_restored_on_refusal() {
        let root = std::env::temp_dir().join(format!(
            "spice-admitted-path-{}-{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        let target = root.join("target");
        let parked = root.join("parked");
        let outside = root.join("outside");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(target.join("file"), b"before").unwrap();
        let metadata = fs::metadata(&target).unwrap();
        let parent = ParentObservation {
            ancestor: "target".to_owned(),
            generation: Generation::observe(&metadata),
            missing: String::new(),
        };
        let admitted = AdmittedPath::open(&root, "target/file", &parent, false, 0).unwrap();
        let expected = admitted.observe().unwrap().unwrap().generation;

        let error = admitted
            .publish_with(
                &FileImage::new(0o644, b"after".to_vec()),
                Some(expected),
                || {
                    fs::rename(&target, &parked).unwrap();
                    symlink(&outside, &target).unwrap();
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(parked.join("file")).unwrap(), b"before");
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_destination_at_publication_is_restored_on_refusal() {
        let root = test_root("publish-replacement");
        let target = root.join("target");
        let destination = target.join("file");
        let parked = target.join("parked");
        fs::create_dir_all(&target).unwrap();
        fs::write(&destination, b"before").unwrap();
        let admitted = admitted_file(&root, &target, "target/file");
        let expected = admitted.observe().unwrap().unwrap().generation;

        let error = admitted
            .publish_with(
                &FileImage::new(0o644, b"after".to_vec()),
                Some(expected),
                || {
                    fs::rename(&destination, &parked).unwrap();
                    fs::write(&destination, b"racer").unwrap();
                    Ok(())
                },
            )
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(&destination).unwrap(), b"racer");
        assert_eq!(fs::read(&parked).unwrap(), b"before");
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appeared_destination_at_publication_is_unchanged_on_refusal() {
        let root = test_root("publish-appearance");
        let target = root.join("target");
        let destination = target.join("file");
        fs::create_dir_all(&target).unwrap();
        let admitted = admitted_file(&root, &target, "target/file");

        let error = admitted
            .publish_with(&FileImage::new(0o644, b"after".to_vec()), None, || {
                fs::write(&destination, b"racer").unwrap();
                Ok(())
            })
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(&destination).unwrap(), b"racer");
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_destination_at_removal_is_restored_on_refusal() {
        let root = test_root("remove-replacement");
        let target = root.join("target");
        let destination = target.join("file");
        let parked = target.join("parked");
        fs::create_dir_all(&target).unwrap();
        fs::write(&destination, b"before").unwrap();
        let admitted = admitted_file(&root, &target, "target/file");
        let expected = admitted.observe().unwrap().unwrap().generation;

        let error = admitted
            .remove_with(expected, || {
                fs::rename(&destination, &parked).unwrap();
                fs::write(&destination, b"racer").unwrap();
                Ok(())
            })
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(&destination).unwrap(), b"racer");
        assert_eq!(fs::read(&parked).unwrap(), b"before");
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_new_directory_is_refused_by_descriptor_identity() {
        let root = test_root("create-replacement");
        fs::create_dir_all(&root).unwrap();
        let parent = unix_fs::open(
            &root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(File::from)
        .unwrap();
        let name = OsString::from("created");
        let created = root.join(&name);
        let parked = root.join("parked");

        let error = create_directory(&parent, &name, &created, || {
            fs::rename(&created, &parked).unwrap();
            fs::create_dir(&created).unwrap();
            fs::write(created.join("racer"), b"outside").unwrap();
            Ok(())
        })
        .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert!(parked.is_dir());
        assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
        drop(parent);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn planned_missing_directory_that_appears_before_open_is_refused() {
        let root = test_root("missing-appearance");
        fs::create_dir_all(&root).unwrap();
        let parent = ParentObservation {
            ancestor: String::new(),
            generation: Generation::observe(&fs::metadata(&root).unwrap()),
            missing: "created/nested".to_owned(),
        };
        let created = root.join("created");
        fs::create_dir(&created).unwrap();
        fs::write(created.join("racer"), b"outside").unwrap();

        let admitted = AdmittedPath::open(&root, "created/nested/file", &parent, true, 2).unwrap();
        let error = admitted
            .publish(&FileImage::new(0o644, b"after".to_vec()), None)
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
        assert!(!created.join("nested").exists());
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_introduced_directory_at_cleanup_is_restored_on_refusal() {
        let root = test_root("cleanup-replacement");
        fs::create_dir_all(&root).unwrap();
        let parent = ParentObservation {
            ancestor: String::new(),
            generation: Generation::observe(&fs::metadata(&root).unwrap()),
            missing: "created".to_owned(),
        };
        let created = root.join("created");
        let parked = root.join("parked");
        let admitted = AdmittedPath::open(&root, "created/file", &parent, true, 1).unwrap();

        let error = admitted
            .cleanup_directory_with(0, &created, || {
                fs::rename(&created, &parked).unwrap();
                fs::create_dir(&created).unwrap();
                fs::write(created.join("racer"), b"outside").unwrap();
                Ok(())
            })
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert!(fs::read_dir(&parked).unwrap().next().is_none());
        assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }

    fn admitted_file(root: &Path, target: &Path, destination: &str) -> AdmittedPath {
        let parent = ParentObservation {
            ancestor: "target".to_owned(),
            generation: Generation::observe(&fs::metadata(target).unwrap()),
            missing: String::new(),
        };
        AdmittedPath::open(root, destination, &parent, false, 0).unwrap()
    }

    fn test_root(case: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spice-admitted-path-{case}-{}-{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ))
    }
}
