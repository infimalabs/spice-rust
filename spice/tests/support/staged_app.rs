//! Closed atomic app-generation fixture for process tests that start a resident.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_family = "unix")]
use std::os::unix::fs::PermissionsExt as _;

const FIXTURE_GENERATION: &str = "4444444444444444444444444444444444444444444444444444444444444444";

pub(super) fn seed(root: &Path) {
    let stage = root.join(canvas::APP_STAGE);
    let generation = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(FIXTURE_GENERATION);
    create_dir_all(root, &generation).expect("fixture app generation directory");
    for asset in canvas::APP.assets() {
        write_file(
            root,
            &generation.join(asset.file()),
            format!("fixture lane app {}\n", asset.file()),
        )
        .expect("fixture app asset");
    }
    write_file(
        root,
        &stage.join(canvas::APP_STAGE_CURRENT),
        format!("{FIXTURE_GENERATION}\n"),
    )
    .expect("fixture app pointer");
}

fn create_dir_all(root: &Path, directory: &Path) -> io::Result<()> {
    validate_owned_path(root, directory)?;
    match fs::create_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            make_existing_ancestors_writable(root, directory)?;
            fs::create_dir_all(directory)
        }
        Err(error) => Err(error),
    }
}

fn write_file(root: &Path, path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    validate_owned_path(root, path)?;
    match fs::write(path, bytes.as_ref()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            make_existing_ancestors_writable(root, path)?;
            if let Some(parent) = path.parent() {
                make_fixture_root_writable(parent)?;
            }
            fs::write(path, bytes)
        }
        Err(error) => Err(error),
    }
}

fn make_existing_ancestors_writable(root: &Path, path: &Path) -> io::Result<()> {
    validate_owned_path(root, path)?;
    let relative = owned_relative_path(root, path)?;
    make_fixture_root_writable(root)?;
    let mut cursor = PathBuf::from(root);
    for component in relative.components() {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(path_escape_error()),
            Ok(metadata) if metadata.is_dir() => make_fixture_root_writable(&cursor)?,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn validate_owned_path(root: &Path, path: &Path) -> io::Result<()> {
    let relative = owned_relative_path(root, path)?;
    reject_symlink_if_present(root)?;
    let mut cursor = PathBuf::from(root);
    for component in relative.components() {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(path_escape_error()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn owned_relative_path<'a>(root: &Path, path: &'a Path) -> io::Result<&'a Path> {
    path.strip_prefix(root).map_err(|_| path_escape_error())
}

fn reject_symlink_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(path_escape_error()),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn path_escape_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "fixture app path escapes its owned root",
    )
}

#[cfg(target_family = "unix")]
fn make_fixture_root_writable(directory: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() {
        return Err(path_escape_error());
    }
    let mode = metadata.permissions().mode();
    fs::set_permissions(directory, fs::Permissions::from_mode(mode | 0o700))
}

#[cfg(not(target_family = "unix"))]
fn make_fixture_root_writable(directory: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() {
        return Err(path_escape_error());
    }
    let mut permissions = metadata.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(directory, permissions)
}
