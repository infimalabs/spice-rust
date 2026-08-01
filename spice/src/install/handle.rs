use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::path::validate_relative;
use super::{DIRECTORY_MODE, FileImage, Generation, InstallError, ObservedFile, ParentObservation};

#[cfg(unix)]
use rustix::fs::{self as unix_fs, AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One destination whose parent is held open from its admitted root.
pub(super) struct AdmittedPath {
    canonical_root: PathBuf,
    display: PathBuf,
    file_name: OsString,
    components: Vec<OsString>,
    directories: Vec<File>,
    ready: bool,
    introduced: usize,
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
        let mut directories = vec![root_fd];
        let mut ready = true;
        for (index, component) in components.iter().enumerate() {
            match open_directory(
                directories.last().expect("the root directory is open"),
                component,
            ) {
                Ok(directory) => directories.push(directory),
                Err(source)
                    if source.kind() == io::ErrorKind::NotFound && index >= anchor_components =>
                {
                    if !create_missing {
                        ready = false;
                        break;
                    }
                    create_directory(
                        directories.last().expect("the directory parent is open"),
                        component,
                        &canonical_root.join(components_path(&components[..=index])),
                    )?;
                    directories.push(
                        open_directory(
                            directories.last().expect("the directory parent is open"),
                            component,
                        )
                        .map_err(|source| {
                            InstallError::io(
                                "open newly created install directory",
                                canonical_root.join(components_path(&components[..=index])),
                                source,
                            )
                        })?,
                    );
                }
                Err(source) => {
                    return Err(InstallError::io(
                        "open admitted install directory",
                        canonical_root.join(components_path(&components[..=index])),
                        source,
                    ));
                }
            }
            if index + 1 == anchor_components {
                let metadata = directories
                    .last()
                    .expect("the admitted anchor is open")
                    .metadata()
                    .map_err(|source| {
                        InstallError::io(
                            "inspect admitted install directory",
                            canonical_root.join(components_path(&components[..=index])),
                            source,
                        )
                    })?;
                if !metadata.is_dir() || !observation.generation.matches(&metadata) {
                    return Err(InstallError::ParentChanged(
                        canonical_root.join(components_path(&components[..=index])),
                    ));
                }
            }
        }
        if anchor_components == 0 {
            let metadata = directories[0].metadata().map_err(|source| {
                InstallError::io("inspect admitted install root", &canonical_root, source)
            })?;
            if !observation.generation.matches(&metadata) {
                return Err(InstallError::ParentChanged(canonical_root));
            }
        }
        Ok(Self {
            canonical_root,
            display: root.join(destination_path),
            file_name,
            components,
            directories,
            ready,
            introduced,
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
        if !self.ready {
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
                self.verify_named_parent()?;
                self.require_generation(expected)?;
                unix_fs::renameat(self.parent(), &temporary, self.parent(), &self.file_name)
                    .map_err(|source| {
                        InstallError::io(
                            "publish admitted install file",
                            &self.display,
                            source.into(),
                        )
                    })?;
                self.parent().sync_all().map_err(|source| {
                    InstallError::io(
                        "synchronize admitted destination parent",
                        &self.display,
                        source,
                    )
                })?;
                self.observe()?
                    .map(|observed| observed.generation)
                    .ok_or_else(|| InstallError::ParentChanged(self.display.clone()))
            })();
            if result.is_err() {
                let _ = unix_fs::unlinkat(self.parent(), &temporary, AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        unreachable!("construction refuses non-Unix filesystems")
    }

    pub(super) fn remove(&self, expected: Generation) -> Result<(), InstallError> {
        self.require_generation(Some(expected))?;
        self.verify_named_parent()?;
        #[cfg(unix)]
        {
            unix_fs::unlinkat(self.parent(), &self.file_name, AtFlags::empty()).map_err(
                |source| {
                    InstallError::io("remove admitted install file", &self.display, source.into())
                },
            )?;
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
                match unix_fs::unlinkat(
                    &self.directories[index],
                    &self.components[index],
                    AtFlags::REMOVEDIR,
                ) {
                    Ok(()) => self.directories[index].sync_all().map_err(|source| {
                        InstallError::io("synchronize introduced directory parent", &path, source)
                    })?,
                    Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTEMPTY) => {}
                    Err(source) => {
                        return Err(InstallError::io(
                            "remove introduced directory",
                            path,
                            source.into(),
                        ));
                    }
                }
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
    fn verify_named_parent(&self) -> Result<(), InstallError> {
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
        for (index, component) in self.components.iter().enumerate() {
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
fn create_directory(parent: &File, name: &OsString, display: &Path) -> Result<(), InstallError> {
    unix_fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE as _)).map_err(|source| {
        if source == rustix::io::Errno::EXIST {
            InstallError::ParentChanged(display.to_path_buf())
        } else {
            InstallError::io("create admitted install directory", display, source.into())
        }
    })?;
    parent.sync_all().map_err(|source| {
        InstallError::io("synchronize admitted directory parent", display, source)
    })
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
    fn renamed_parent_after_admission_refuses_before_publication() {
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

        fs::rename(&target, &parked).unwrap();
        symlink(&outside, &target).unwrap();
        let error = admitted
            .publish(&FileImage::new(0o644, b"after".to_vec()), Some(expected))
            .unwrap_err();

        assert!(matches!(error, InstallError::ParentChanged(_)));
        assert_eq!(fs::read(parked.join("file")).unwrap(), b"before");
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        drop(admitted);
        fs::remove_dir_all(root).unwrap();
    }
}
