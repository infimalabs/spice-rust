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

use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

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

/// One filesystem object, whatever its contents: Unix device and inode, or
/// Windows volume serial and 128-bit file ID.
///
/// A directory keeps its object while entries come and go beneath it; a
/// replacement under the same name is always another object.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct FileObject {
    volume: u64,
    index: [u8; 16],
}

/// One filesystem object and the state it was observed in.
///
/// Equality means the same object, unchanged: same [`FileObject`], same
/// length, and the same modification and change times. A change time moves on
/// any metadata change, including permissions and ownership. Unlike length or
/// timestamps alone, a replacement can never match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    object: FileObject,
    bytes: u64,
    modified: i128,
    changed: i128,
}

impl FileIdentity {
    /// The object observed.
    #[must_use]
    pub const fn object(&self) -> FileObject {
        self.object
    }

    /// The object's length in bytes when observed.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// The identity of the object `file` holds open.
///
/// # Errors
///
/// Returns an operating-system refusal querying the handle.
pub fn file_identity(file: &File) -> io::Result<FileIdentity> {
    platform::file_identity(file)
}

/// The identity of the object at `path`, without following a final link or
/// reparse point.
///
/// # Errors
///
/// Returns an operating-system refusal opening or querying the object.
pub fn path_identity(path: &Path) -> io::Result<FileIdentity> {
    platform::path_identity(path)
}

/// Whether the current user owns the object at `path`, without following a
/// final link.
///
/// Unix compares the effective user. Windows accepts the token user or the
/// default owner that token assigns to new objects.
///
/// # Errors
///
/// Returns an operating-system refusal reading the object's owner.
pub fn owned_by_current_user(path: &Path) -> io::Result<bool> {
    platform::owned_by_current_user(path)
}

/// Open `path` for reading without following a final link or reparse point.
///
/// A final link opens as itself or is refused, so the opened object's
/// [`file_identity`] never silently names a link's target. Unix opens
/// non-blocking, so a planted FIFO cannot stall the open; Windows shares the
/// object for read, write, and delete, so a publisher can still replace it.
///
/// # Errors
///
/// Returns an operating-system refusal opening the object, including the Unix
/// refusal of a final symbolic link.
pub fn open_unfollowed(path: &Path) -> io::Result<File> {
    platform::open_unfollowed(path)
}

/// One entry opened by name through a held [`Directory`].
#[derive(Debug)]
pub enum Entry {
    /// Nothing is named.
    Missing,
    /// The name is a link or reparse point, which was not followed.
    Link,
    /// The named object itself, which may still be any non-link kind.
    Opened(File),
}

/// A held directory whose entries are reached by name without resolving the
/// directory's own path again.
///
/// Unix holds a descriptor and resolves every entry relative to it, so a
/// rename of the directory cannot redirect an operation. Windows holds a
/// handle that refuses delete sharing, so the directory can be neither renamed
/// nor removed while held and its path keeps naming it. No operation follows a
/// final link: opening one reports [`Entry::Link`], and creation refuses any
/// existing entry.
#[derive(Debug)]
pub struct Directory {
    handle: File,
    path: PathBuf,
}

impl Directory {
    /// Hold the directory at `path`.
    ///
    /// # Errors
    ///
    /// Refuses a final link or reparse point and any object that is not a
    /// directory, or returns the operating-system refusal opening it.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            handle: platform::hold_directory(path)?,
            path: path.to_path_buf(),
        })
    }

    /// The path the directory was held at.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The held directory's identity.
    ///
    /// # Errors
    ///
    /// Returns an operating-system refusal querying the handle.
    pub fn identity(&self) -> io::Result<FileIdentity> {
        platform::file_identity(&self.handle)
    }

    /// Create `name` exclusively as an owner-only (`0o600`) file open for
    /// reading and writing.
    ///
    /// # Errors
    ///
    /// Refuses any existing entry, including a link, and a `name` that is not
    /// one plain path component.
    pub fn create_private(&self, name: &OsStr) -> io::Result<File> {
        platform::create_private_entry(&self.handle, &self.path, entry_name(name)?)
    }

    /// Open `name` for reading, or for reading and writing when `write` is
    /// set.
    ///
    /// # Errors
    ///
    /// Refuses a `name` that is not one plain path component, or returns the
    /// operating-system refusal opening the entry.
    pub fn open_entry(&self, name: &OsStr, write: bool) -> io::Result<Entry> {
        platform::open_entry(&self.handle, &self.path, entry_name(name)?, write)
    }

    /// Atomically rename `source` over `destination`, replacing a file there.
    ///
    /// # Errors
    ///
    /// Refuses a name that is not one plain path component, or returns the
    /// operating-system refusal of the rename.
    pub fn replace(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        platform::replace_entry(
            &self.handle,
            &self.path,
            entry_name(source)?,
            entry_name(destination)?,
        )
    }

    /// Remove the file or link `name`.
    ///
    /// # Errors
    ///
    /// Refuses a `name` that is not one plain path component, or returns the
    /// operating-system refusal of the removal.
    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        platform::remove_entry(&self.handle, &self.path, entry_name(name)?)
    }

    /// Make every completed entry change durable.
    ///
    /// # Errors
    ///
    /// Returns the operating-system refusal flushing the directory.
    pub fn sync(&self) -> io::Result<()> {
        platform::sync_directory(&self.handle, &self.path)
    }
}

/// `name` when it is exactly one plain path component.
fn entry_name(name: &OsStr) -> io::Result<&OsStr> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if component == name => Ok(name),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a directory entry needs one plain name: {name:?}"),
        )),
    }
}

#[cfg(test)]
mod tests;
