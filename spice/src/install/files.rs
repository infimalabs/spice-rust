use std::fs;
use std::io;
use std::path::Path;

use crate::repository::Repository;

use super::handle::{AdmittedPath, retirement_temporary_name};
use super::{
    Asset, FileImage, Generation, InstallError, ObservedFile, Operation, ParentObservation,
    ReceiptGeneration, Retirement, fixed_asset_bytes, read_binary,
};

pub(super) fn retire_path(
    repository: &Repository,
    retirement: &Retirement,
    index: usize,
) -> Result<Option<Generation>, InstallError> {
    let path = AdmittedPath::open(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
        false,
        0,
    )?;
    let current = path.observe()?;
    if !retirement.discards_payload() {
        if let Some(current) = current {
            if !retirement.expects_in_place(Some(&current)) {
                return Err(super::retirement_drift(retirement, Some(current)));
            }
            path.remove(current.generation)?;
        }
        return path.parent_generation();
    }
    let temporary = retirement_temporary_name(index);
    let staged = path.observe_named(&temporary)?;
    match (current, staged) {
        (Some(current), None) if retirement.expects_in_place(Some(&current)) => {
            path.retire(current.generation, &temporary)?;
        }
        (None, Some(staged)) if retirement.expects_in_place(Some(&staged)) => {
            path.resume_retirement(staged.generation, &temporary)?;
        }
        (None, None) => {}
        (Some(_), Some(_)) => {
            return Err(temporary_collision(repository, retirement, &temporary));
        }
        (Some(current), None) => {
            return Err(super::retirement_drift(retirement, Some(current)));
        }
        (None, Some(staged)) => {
            return Err(super::retirement_drift(retirement, Some(staged)));
        }
    }
    path.parent_generation()
}

pub(super) fn prepare_retirement(
    repository: &Repository,
    retirement: &Retirement,
    index: usize,
) -> Result<Option<Generation>, InstallError> {
    debug_assert!(retirement.discards_payload());
    let path = AdmittedPath::open(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
        false,
        0,
    )?;
    let current = path.observe()?;
    if !retirement.expects_in_place(current.as_ref()) {
        return Err(super::retirement_drift(retirement, current));
    }
    let temporary = retirement_temporary_name(index);
    if path.observe_named(&temporary)?.is_some() {
        return Err(temporary_collision(repository, retirement, &temporary));
    }
    path.parent_generation()
}

/// Remove the directories a discarded retirement's binding names as missing,
/// each only while it is empty.
///
/// The binding anchors above those directories, so a directory an earlier
/// attempt already removed leaves nothing to open and nothing to do.
pub(super) fn withdraw_parent(
    repository: &Repository,
    retirement: &Retirement,
) -> Result<(), InstallError> {
    debug_assert!(retirement.discards_payload());
    let mut path = AdmittedPath::open(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
        false,
        Path::new(&retirement.parent.missing).components().count(),
    )?;
    path.cleanup_introduced()
}

fn temporary_collision(
    repository: &Repository,
    retirement: &Retirement,
    temporary: &std::ffi::OsStr,
) -> InstallError {
    let mut path = retirement.destination(repository);
    path.set_file_name(temporary);
    InstallError::ParentChanged(path)
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
    app: Option<&super::app::Source>,
    symbols: Option<&super::symbols::Source>,
    common_git: &Path,
) -> Result<FileImage, InstallError> {
    let retained = super::app::operation_generation(operation.asset, &operation.path)
        .filter(|generation| app.is_some_and(|source| source.generation() != generation))
        .map(|_| observe(&common_git.join(&operation.destination)))
        .transpose()?
        .flatten()
        .filter(|observed| observed.generated() == operation.generated)
        .map(|observed| observed.image.bytes);
    let bytes = match retained.or_else(|| app.and_then(|source| source.bytes(operation.asset))) {
        Some(bytes) => bytes,
        None => match fixed_asset_bytes(operation.asset, common_git)? {
            Some(bytes) => bytes,
            None => match operation.asset {
                Asset::Exclusions => {
                    return Err(InstallError::InvalidRecord(
                        "install plan contains the retired version-5 exclusions asset".to_owned(),
                    ));
                }
                Asset::Binary => {
                    read_binary(spice_binary.ok_or(InstallError::BinaryContextChanged)?)?
                }
                Asset::SymbolsInfo | Asset::SymbolsDwarf => symbols
                    .and_then(|source| source.bytes(operation.asset))
                    .ok_or(InstallError::BinaryContextChanged)?
                    .to_vec(),
                _ => return Err(InstallError::BinaryContextChanged),
            },
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
    expected: Option<ReceiptGeneration>,
) -> Result<ReceiptGeneration, InstallError> {
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use wire::{Digest, Domain};

    use super::super::binding::bind_destination;
    use super::super::{DATA_MODE, GeneratedImage, app, retention};
    use super::*;

    /// One scratch repository, removed again when the test that made it ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn repository(label: &str) -> (Scratch, Repository) {
        let root = std::env::temp_dir().join(format!(
            "spice-install-files-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("scratch root creates");
        crate::test_repository::initialize(&root);
        let repository = Repository::discover(&root).expect("scratch repository discovers");
        (Scratch(root), repository)
    }

    /// Install one app generation file and the discard planning derives for it.
    fn installed_discard(repository: &Repository, generation: &str, file: &str) -> Retirement {
        let path = format!("{}/{generation}/{file}", app::GENERATIONS_PATH);
        let installed = repository.common_dir().join(&path);
        fs::create_dir_all(installed.parent().unwrap()).unwrap();
        write_installed(&installed, file.as_bytes());
        let bound = bind_destination(repository.common_dir(), &path).unwrap();
        let mut retirement = Retirement {
            path,
            destination: bound.destination,
            parent: bound.parent,
            previous: None,
            observed: None,
            superseded: Some(GeneratedImage::new(DATA_MODE, file.as_bytes())),
            discard: true,
        };
        (retirement.destination, retirement.parent) =
            retention::retirement_binding(repository, &retirement).unwrap();
        retirement
    }

    fn write_installed(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(DATA_MODE)).unwrap();
    }

    fn temporary(repository: &Repository, retirement: &Retirement, index: usize) -> PathBuf {
        retirement
            .destination(repository)
            .with_file_name(retirement_temporary_name(index))
    }

    fn generation() -> String {
        Digest::compute(Domain::InstallArtifact, b"retired generation").to_string()
    }

    #[test]
    fn an_interrupted_discard_resumes_and_withdraws_its_generation() {
        let (_scratch, repository) = repository("resume");
        let generation = generation();
        let first = installed_discard(&repository, &generation, "app.html");
        let second = installed_discard(&repository, &generation, "app.module");
        let directory = first.destination(&repository).parent().unwrap().to_owned();
        for (index, retirement) in [&first, &second].into_iter().enumerate() {
            assert!(prepare_retirement(&repository, retirement, index).is_ok());
        }

        // Interrupted after the file moved to its fenced temporary name.
        fs::rename(
            first.destination(&repository),
            temporary(&repository, &first, 0),
        )
        .unwrap();
        retire_path(&repository, &first, 0).expect("resume finishes the staged discard");
        assert!(!temporary(&repository, &first, 0).exists());
        assert!(!first.destination(&repository).exists());
        // Interrupted after the discard finished but before progress advanced.
        retire_path(&repository, &first, 0).expect("a finished discard is a no-op");

        retire_path(&repository, &second, 1).expect("the next discard retires in place");
        assert!(second.withdraws_parent(None));
        withdraw_parent(&repository, &second).expect("the empty generation withdraws");
        assert!(!directory.exists());
        withdraw_parent(&repository, &second).expect("a withdrawn generation stays withdrawn");
    }

    #[test]
    fn a_drifted_or_colliding_discard_refuses_without_moving_any_file() {
        let (_scratch, repository) = repository("drift");
        let retirement = installed_discard(&repository, &generation(), "app.html");
        let installed = retirement.destination(&repository);
        let staged = temporary(&repository, &retirement, 0);

        write_installed(&installed, b"operator bytes");
        assert!(matches!(
            prepare_retirement(&repository, &retirement, 0),
            Err(InstallError::Drift { .. })
        ));
        assert!(matches!(
            retire_path(&repository, &retirement, 0),
            Err(InstallError::Drift { .. })
        ));
        assert_eq!(fs::read(&installed).unwrap(), b"operator bytes");
        assert!(!staged.exists());

        write_installed(&installed, b"app.html");
        write_installed(&staged, b"app.html");
        assert!(matches!(
            prepare_retirement(&repository, &retirement, 0),
            Err(InstallError::ParentChanged(_))
        ));
        assert!(matches!(
            retire_path(&repository, &retirement, 0),
            Err(InstallError::ParentChanged(_))
        ));
        assert!(installed.exists() && staged.exists());

        fs::remove_file(&installed).unwrap();
        write_installed(&staged, b"operator bytes");
        assert!(matches!(
            retire_path(&repository, &retirement, 0),
            Err(InstallError::Drift { .. })
        ));
        assert_eq!(fs::read(&staged).unwrap(), b"operator bytes");
    }
}
