//! Owner-private filesystem authority.
//!
//! These operations give callers one permission and publication vocabulary on
//! every release target. A mode is the Unix owner/group/other triplet. On
//! Windows it is projected from the object's discretionary ACL: the owner bits
//! are the access granted to the current token user, and the group and other
//! bits are any access granted to another principal or held through foreign
//! ownership, so a nonzero `0o077` always means another principal can reach the
//! object. Setting a mode there writes one protected owner-only ACL; a directory
//! also carries inherit-only grants equivalent to a `077` umask, so entries
//! created inside a private directory are private. Write access never grants
//! deletion of the object itself; as on Unix, removing an entry is governed by
//! its parent directory's write bit.
//!
//! None of these operations requires elevation or Windows Developer Mode.

use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// Owner, group, and other permission bits of `path`, without following a
/// final link.
///
/// # Errors
///
/// Returns an operating-system refusal reading the object's metadata or
/// security descriptor.
pub fn owner_mode(path: &Path) -> io::Result<u32> {
    platform::owner_mode(path)
}

/// Make `path` accessible to its owner alone with exactly the `mode` owner
/// bits.
///
/// The caller must have refused links first: Unix follows a final symlink.
///
/// # Errors
///
/// Returns `InvalidInput` when `mode` grants group or other access, or an
/// operating-system refusal applying the permissions.
pub fn set_owner_mode(path: &Path, mode: u32) -> io::Result<()> {
    if mode & !0o700 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("owner mode {mode:04o} grants access beyond the owner"),
        ));
    }
    platform::set_owner_mode(path, mode)
}

/// Create one directory that only its owner can reach (mode `0700`).
///
/// # Errors
///
/// Returns `AlreadyExists` when `path` names any existing entry, or another
/// operating-system refusal creating it.
pub fn create_private_directory(path: &Path) -> io::Result<()> {
    platform::create_private_directory(path)
}

/// Atomically rename `source` to `destination` unless `destination` exists.
///
/// # Errors
///
/// Returns `AlreadyExists` when `destination` names any entry, or another
/// operating-system refusal renaming the entry.
pub fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    platform::rename_no_replace(source, destination)
}

/// Create `link` as a directory link to `target`.
///
/// Unix creates a symbolic link and preserves a relative target. Windows
/// creates an unprivileged directory junction, which requires `target` to be
/// an absolute local path. Either way the link records
/// [`directory_link_form`]`(target)`.
///
/// # Errors
///
/// Returns `InvalidInput` when Windows receives a non-local target, or an
/// operating-system refusal creating the link.
pub fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    platform::link_directory(target, link)
}

/// The target a directory link records for `target`, exactly as
/// [`read_directory_link`] reports it.
///
/// Unix records `target` verbatim. Windows records the absolute drive form, so
/// `\\?\C:\x` and `C:\x` record the same `C:\x`.
///
/// # Errors
///
/// Returns `InvalidInput` when Windows receives a target that is not a local
/// drive path.
pub fn directory_link_form(target: &Path) -> io::Result<PathBuf> {
    platform::directory_link_form(target)
}

/// The target recorded by a directory link that [`link_directory`] created.
///
/// # Errors
///
/// Returns an operating-system refusal when `link` is not a link.
pub fn read_directory_link(link: &Path) -> io::Result<PathBuf> {
    platform::read_directory_link(link)
}

/// Bytes the filesystem allocated for the entry `metadata` describes.
///
/// `metadata` must be the unfollowed metadata of `path`.
///
/// # Errors
///
/// Returns an operating-system refusal querying the allocation.
pub fn allocated_bytes(path: &Path, metadata: &Metadata) -> io::Result<u64> {
    platform::allocated_bytes(path, metadata)
}

/// Whether unfollowed `metadata` describes a link or any other reparse point.
///
/// Windows reparse points include junctions, mount points, and placeholders
/// whose content another filter supplies; none is an owned regular entry.
pub fn is_link_like(metadata: &Metadata) -> bool {
    platform::is_link_like(metadata)
}

#[cfg(test)]
mod tests;
