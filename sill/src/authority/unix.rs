use std::ffi::CString;
use std::fs::{self, DirBuilder, Metadata, Permissions};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const BLOCK_BYTES: u64 = 512;

pub(super) fn owner_mode(path: &Path) -> io::Result<u32> {
    Ok(fs::symlink_metadata(path)?.mode() & 0o777)
}

pub(super) fn set_owner_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, Permissions::from_mode(mode))
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

pub(super) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = c_path(source)?;
    let destination = c_path(destination)?;
    // SAFETY: both arguments are NUL-terminated paths that outlive the call.
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    // SAFETY: both arguments are NUL-terminated paths that outlive the call,
    // and `AT_FDCWD` resolves them as `rename` would.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn directory_link_form(target: &Path) -> io::Result<PathBuf> {
    Ok(target.to_path_buf())
}

pub(super) fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

pub(super) fn read_directory_link(link: &Path) -> io::Result<PathBuf> {
    fs::read_link(link)
}

pub(super) fn allocated_bytes(_path: &Path, metadata: &Metadata) -> io::Result<u64> {
    Ok(metadata.blocks().saturating_mul(BLOCK_BYTES))
}

pub(super) fn is_link_like(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}
