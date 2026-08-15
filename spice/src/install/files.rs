use std::fs;
use std::io;
use std::path::Path;

use crate::repository::Repository;

use super::handle::AdmittedPath;
use super::{
    Asset, FileImage, Generation, InstallError, ObservedFile, Operation, ParentObservation,
    Retirement, fixed_asset_bytes, read_binary,
};

pub(super) fn retire_path(
    repository: &Repository,
    retirement: &Retirement,
) -> Result<Option<Generation>, InstallError> {
    let path = AdmittedPath::open(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
        false,
        0,
    )?;
    let current = path.observe()?;
    if let Some(current) = current {
        if !retirement.expects_in_place(Some(&current)) {
            return Err(super::retirement_drift(retirement, Some(current)));
        }
        path.remove(current.generation)?;
    } else if !retirement.expects_in_place(None) {
        // Absence is also the completed side of an interrupted retirement.
        // The progress verifier admits that transition before reaching here.
        return path.parent_generation();
    }
    path.parent_generation()
}

pub(super) fn restore_retirement(
    repository: &Repository,
    retirement: &Retirement,
    previous: Option<&FileImage>,
) -> Result<Option<Generation>, InstallError> {
    let path = AdmittedPath::open(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
        false,
        0,
    )?;
    let current = path.observe()?;
    if current.is_some() {
        return Err(super::retirement_drift(retirement, current));
    }
    previous
        .map(|previous| path.publish(previous, None).map(Some))
        .unwrap_or(Ok(None))
}

pub(super) fn publish_operation(
    repository: &Repository,
    operation: &Operation,
    image: &FileImage,
) -> Result<(Generation, Generation), InstallError> {
    let path = AdmittedPath::open(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
        true,
        operation.introduced_parents.len(),
    )?;
    let current = path.observe()?;
    if !operation.expects_in_place(current.as_ref()) {
        return Err(InstallError::Drift {
            path: operation.path.clone(),
            observed: current.map(|observed| observed.image.generated().digest),
        });
    }
    let generation = path.publish(image, current.map(|observed| observed.generation))?;
    let parent = path
        .parent_generation()?
        .ok_or_else(|| InstallError::ParentChanged(operation.destination(repository)))?;
    Ok((generation, parent))
}

pub(super) fn materialize_generated(
    operation: &Operation,
    spice_binary: Option<&Path>,
    common_git: &Path,
) -> Result<FileImage, InstallError> {
    let bytes = match fixed_asset_bytes(operation.asset, common_git)? {
        Some(bytes) => bytes,
        None => match operation.asset {
            Asset::Exclusions => {
                return Err(InstallError::InvalidRecord(
                    "install plan contains the retired version-5 exclusions asset".to_owned(),
                ));
            }
            Asset::Binary => read_binary(spice_binary.ok_or(InstallError::BinaryContextChanged)?)?,
            _ => unreachable!("fixed assets materialize from repository-bound bytes"),
        },
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
    expected: Generation,
    previous: Option<&FileImage>,
) -> Result<Option<Generation>, InstallError> {
    let mut path = AdmittedPath::open(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
        false,
        operation.introduced_parents.len(),
    )?;
    let restored = match previous {
        Some(previous) => path.publish(previous, Some(expected)).map(Some),
        None => {
            path.remove(expected)?;
            Ok(None)
        }
    }?;
    path.cleanup_introduced()?;
    Ok(restored)
}

pub(super) fn cleanup_operation(
    repository: &Repository,
    operation: &Operation,
) -> Result<(), InstallError> {
    let mut path = AdmittedPath::open(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
        false,
        operation.introduced_parents.len(),
    )?;
    path.cleanup_introduced()
}

pub(super) fn observe_bound(
    root: &Path,
    destination: &str,
    parent: &ParentObservation,
) -> Result<(Option<ObservedFile>, Option<Generation>), InstallError> {
    let path = AdmittedPath::open(root, destination, parent, false, 0)?;
    Ok((path.observe()?, path.parent_generation()?))
}

pub(super) fn publish_bound(
    root: &Path,
    destination: &str,
    parent: &ParentObservation,
    introduced: usize,
    image: &FileImage,
    expected: Option<Generation>,
) -> Result<Generation, InstallError> {
    let path = AdmittedPath::open(root, destination, parent, true, introduced)?;
    path.publish_receipt(image, expected)
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

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    super::DATA_MODE
}
