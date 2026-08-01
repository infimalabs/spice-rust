use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::repository::Repository;

use super::path::validate_relative;
use super::{
    Asset, DIRECTORY_MODE, FileImage, Generation, InstallError, ObservedFile, Operation,
    RECORD_MODE, SKILL, exclusions, read_binary,
};

pub(super) fn publish_operation(
    repository: &Repository,
    operation: &Operation,
    image: &FileImage,
) -> Result<Generation, InstallError> {
    for relative in &operation.introduced_parents {
        ensure_directory(operation.location.root(repository), relative)?;
    }
    publish_image(&operation.destination(repository), image, false)
}

pub(super) fn materialize_generated(
    operation: &Operation,
    rust_binary: Option<&Path>,
) -> Result<FileImage, InstallError> {
    let bytes = match operation.asset {
        Asset::Exclusions => exclusions(
            operation
                .previous
                .as_ref()
                .map(|previous| previous.image.bytes()),
        )?,
        Asset::HookEntry => steer::hook::STAGES[0].bytes().as_bytes().to_vec(),
        Asset::HookStatic => steer::hook::STAGES[1].bytes().as_bytes().to_vec(),
        Asset::Skill => SKILL.as_bytes().to_vec(),
        Asset::Binary => read_binary(rust_binary.ok_or(InstallError::BinaryContextChanged)?)?,
    };
    let image = FileImage::new(operation.generated.mode, bytes);
    if image.generated() != operation.generated {
        return Err(InstallError::BinaryContextChanged);
    }
    Ok(image)
}

pub(super) fn matches_generated(observed: Option<&ObservedFile>, operation: &Operation) -> bool {
    observed.is_some_and(|image| image.generated() == operation.generated)
}

pub(super) fn restore_operation(
    repository: &Repository,
    operation: &Operation,
) -> Result<Option<Generation>, InstallError> {
    let destination = operation.destination(repository);
    match &operation.previous {
        Some(previous) => publish_image(&destination, &previous.image, false).map(Some),
        None => {
            fs::remove_file(&destination).map_err(|source| {
                InstallError::io("remove installed file", &destination, source)
            })?;
            sync_parent(&destination)?;
            Ok(None)
        }
    }
}

pub(super) fn publish_image(
    path: &Path,
    image: &FileImage,
    private: bool,
) -> Result<Generation, InstallError> {
    let destination = atomic::Destination::new(path).map_err(InstallError::Atomic)?;
    let mut staged = destination.stage().map_err(InstallError::Atomic)?;
    staged
        .write_all(&image.bytes)
        .map_err(|source| InstallError::io("write staged file", staged.temporary_path(), source))?;
    set_mode(
        staged.temporary_path(),
        if private { RECORD_MODE } else { image.mode },
    )?;
    staged.commit().map_err(InstallError::Atomic)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| InstallError::io("inspect published file", path, source))?;
    if !metadata.is_file() {
        return Err(InstallError::PathShape(path.to_path_buf()));
    }
    Ok(Generation::observe(&metadata))
}

pub(super) fn cleanup_parents(
    repository: &Repository,
    operation: &Operation,
) -> Result<(), InstallError> {
    for relative in operation.introduced_parents.iter().rev() {
        let path = operation.location.root(repository).join(relative);
        match fs::remove_dir(&path) {
            Ok(()) => sync_parent(&path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(source) => {
                return Err(InstallError::io(
                    "remove introduced directory",
                    path,
                    source,
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_directory(root: &Path, relative: &str) -> Result<(), InstallError> {
    let relative = validate_relative(relative)?;
    let path = root.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(InstallError::PathShape(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_directory_path(root, relative)
        }
        Err(source) => Err(InstallError::io("inspect directory", path, source)),
    }
}

pub(super) fn create_directory(root: &Path, relative: &str) -> Result<(), InstallError> {
    let relative = validate_relative(relative)?;
    create_directory_path(root, relative)
}

fn create_directory_path(root: &Path, relative: &Path) -> Result<(), InstallError> {
    let path = root.join(relative);
    let parent = path
        .parent()
        .expect("a relative directory below a root has a parent");
    let metadata = fs::symlink_metadata(parent)
        .map_err(|source| InstallError::io("inspect directory parent", parent, source))?;
    if !metadata.is_dir() {
        return Err(InstallError::PathShape(parent.to_path_buf()));
    }
    fs::create_dir(&path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            InstallError::ParentChanged(path.clone())
        } else {
            InstallError::io("create directory", &path, source)
        }
    })?;
    set_mode(&path, DIRECTORY_MODE)
}

pub(super) fn observe(path: &Path) -> Result<Option<ObservedFile>, InstallError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(InstallError::io("inspect file", path, source)),
    };
    if !metadata.file_type().is_file() {
        return Err(InstallError::PathShape(path.to_path_buf()));
    }
    let bytes = fs::read(path).map_err(|source| InstallError::io("read file", path, source))?;
    Ok(Some(ObservedFile {
        image: FileImage::new(file_mode(&metadata), bytes),
        generation: Generation::observe(&metadata),
    }))
}

fn sync_parent(path: &Path) -> Result<(), InstallError> {
    let parent = path.parent().expect("an installed path has a parent");
    let directory = fs::File::open(parent)
        .map_err(|source| InstallError::io("open parent directory", parent, source))?;
    directory
        .sync_all()
        .map_err(|source| InstallError::io("synchronize parent directory", parent, source))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    super::DATA_MODE
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| InstallError::io("set file mode", path, source))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), InstallError> {
    Ok(())
}
