use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use wire::Digest;
use wire::manifest::{MAC_SYMBOL_ARTIFACT_VERSION, MacSymbolArtifact};

use super::BuildError;

const MAC_SYMBOL_ARTIFACT_BYTES_LIMIT: u64 = 512 * 1024 * 1024;
const MAC_SYMBOL_ENCODING_OVERHEAD: u64 = 4096;
const MAC_SYMBOL_RAW_BYTES_LIMIT: u64 =
    (MAC_SYMBOL_ARTIFACT_BYTES_LIMIT - MAC_SYMBOL_ENCODING_OVERHEAD) / 4;

pub(super) fn encode_symbol_bundle(
    bundle: &Path,
    executable: &Path,
    uuid: &str,
    executable_digest: Digest,
) -> Result<Vec<u8>, BuildError> {
    let name = executable
        .file_name()
        .ok_or_else(|| BuildError::new("linked executable has no filename"))?;
    let root = open_directory(bundle)?;
    require_entries(&root, bundle, &[OsStr::new("Contents")])?;
    let contents_path = bundle.join("Contents");
    let contents = open_directory_at(&root, OsStr::new("Contents"), &contents_path)?;
    require_entries(
        &contents,
        &contents_path,
        &[OsStr::new("Info.plist"), OsStr::new("Resources")],
    )?;
    let resources_path = contents_path.join("Resources");
    let resources = open_directory_at(&contents, OsStr::new("Resources"), &resources_path)?;
    require_entries(&resources, &resources_path, &[OsStr::new("DWARF")])?;
    let dwarf_path = resources_path.join("DWARF");
    let dwarf_root = open_directory_at(&resources, OsStr::new("DWARF"), &dwarf_path)?;
    require_entries(&dwarf_root, &dwarf_path, &[name])?;

    let info_path = contents_path.join("Info.plist");
    let info = SymbolMember::open(&contents, OsStr::new("Info.plist"), info_path)?;
    let dwarf = SymbolMember::open(&dwarf_root, name, dwarf_path.join(name))?;
    let raw_bytes = info
        .identity
        .length
        .checked_add(dwarf.identity.length)
        .filter(|bytes| *bytes <= MAC_SYMBOL_RAW_BYTES_LIMIT)
        .ok_or_else(|| BuildError::new("dSYM bundle exceeds its fixed encoded byte limit"))?;
    if raw_bytes
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(MAC_SYMBOL_ENCODING_OVERHEAD))
        .is_none_or(|bytes| bytes > MAC_SYMBOL_ARTIFACT_BYTES_LIMIT)
    {
        return Err(BuildError::new(
            "dSYM bundle exceeds its fixed encoded byte limit",
        ));
    }

    let artifact = wire::encode(&MacSymbolArtifact {
        version: MAC_SYMBOL_ARTIFACT_VERSION,
        uuid: uuid.to_owned(),
        executable: executable_digest,
        info: info.read()?,
        dwarf: dwarf.read()?,
    })
    .map_err(|error| BuildError::new(error.to_string()))?;
    if artifact.len() as u64 > MAC_SYMBOL_ARTIFACT_BYTES_LIMIT {
        return Err(BuildError::new(
            "encoded dSYM artifact exceeds its fixed byte limit",
        ));
    }
    require_same_path(bundle, &root)?;
    require_child(
        &root,
        OsStr::new("Contents"),
        &contents,
        &contents_path,
        true,
    )?;
    require_child(
        &contents,
        OsStr::new("Resources"),
        &resources,
        &resources_path,
        true,
    )?;
    require_child(
        &resources,
        OsStr::new("DWARF"),
        &dwarf_root,
        &dwarf_path,
        true,
    )?;
    info.require_current(&contents, OsStr::new("Info.plist"))?;
    dwarf.require_current(&dwarf_root, name)?;
    Ok(artifact)
}

struct SymbolMember {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
}

impl SymbolMember {
    fn open(parent: &File, name: &OsStr, path: PathBuf) -> Result<Self, BuildError> {
        let file = open_file_at(parent, name, &path)?;
        let metadata = file
            .metadata()
            .map_err(|error| path_error("inspect", &path, error))?;
        if !metadata.is_file() {
            return Err(BuildError::new(format!(
                "dSYM member {} is not a regular file",
                path.display()
            )));
        }
        let identity = FileIdentity::from_metadata(&metadata);
        if identity.length > MAC_SYMBOL_RAW_BYTES_LIMIT {
            return Err(BuildError::new(format!(
                "dSYM member {} exceeds its fixed byte limit",
                path.display()
            )));
        }
        Ok(Self {
            path,
            file,
            identity,
        })
    }

    fn read(&self) -> Result<Vec<u8>, BuildError> {
        let mut file = self
            .file
            .try_clone()
            .map_err(|error| path_error("clone", &self.path, error))?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(self.identity.length).expect("bounded dSYM member length fits usize"),
        );
        file.by_ref()
            .take(self.identity.length.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| path_error("read", &self.path, error))?;
        if bytes.len() as u64 != self.identity.length
            || FileIdentity::capture(&self.file, &self.path)? != self.identity
        {
            return Err(moved(&self.path));
        }
        Ok(bytes)
    }

    fn require_current(&self, parent: &File, name: &OsStr) -> Result<(), BuildError> {
        require_child(parent, name, &self.file, &self.path, false)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    length: u64,
    modified: Option<std::time::SystemTime>,
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn capture(file: &File, path: &Path) -> Result<Self, BuildError> {
        file.metadata()
            .map(|metadata| Self::from_metadata(&metadata))
            .map_err(|error| path_error("inspect", path, error))
    }

    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

fn open_directory(path: &Path) -> Result<File, BuildError> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| path_error("open", path, error))
}

fn open_directory_at(parent: &File, name: &OsStr, path: &Path) -> Result<File, BuildError> {
    open_at(
        parent,
        name,
        path,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
    )
}

fn open_file_at(parent: &File, name: &OsStr, path: &Path) -> Result<File, BuildError> {
    open_at(
        parent,
        name,
        path,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
    )
}

fn require_entries(directory: &File, path: &Path, expected: &[&OsStr]) -> Result<(), BuildError> {
    let mut directory = Directory::open(directory, path)?;
    let mut entries = Vec::with_capacity(expected.len());
    while let Some(name) = directory.next(path)? {
        if matches!(name.as_bytes(), b"." | b"..") {
            continue;
        }
        if entries.len() == expected.len() {
            return Err(BuildError::new(format!(
                "dSYM directory {} exceeds its fixed entry count",
                path.display()
            )));
        }
        entries.push(name);
    }
    entries.sort();
    let mut expected = expected
        .iter()
        .map(|name| (*name).to_os_string())
        .collect::<Vec<_>>();
    expected.sort();
    if entries == expected {
        Ok(())
    } else {
        Err(BuildError::new(format!(
            "dSYM directory {} contains {entries:?}, expected {expected:?}",
            path.display()
        )))
    }
}

fn open_at(parent: &File, name: &OsStr, path: &Path, flags: i32) -> Result<File, BuildError> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| BuildError::new(format!("dSYM path {} contains NUL", path.display())))?;
    // SAFETY: parent and name remain valid for the call, flags never create a
    // file, and a successful descriptor is transferred exactly once to File.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        Err(path_error("open", path, std::io::Error::last_os_error()))
    } else {
        // SAFETY: openat returned this owned descriptor and no other owner exists.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

struct Directory(*mut libc::DIR);

impl Directory {
    fn open(file: &File, path: &Path) -> Result<Self, BuildError> {
        // SAFETY: dup only reads the live descriptor and returns new ownership.
        let descriptor = unsafe { libc::dup(file.as_raw_fd()) };
        if descriptor < 0 {
            return Err(path_error(
                "duplicate",
                path,
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: descriptor is a duplicate of an opened directory; fdopendir
        // takes ownership on success.
        let directory = unsafe { libc::fdopendir(descriptor) };
        if directory.is_null() {
            // SAFETY: fdopendir failed, so ownership remains with this scope.
            unsafe { libc::close(descriptor) };
            Err(path_error("read", path, std::io::Error::last_os_error()))
        } else {
            Ok(Self(directory))
        }
    }

    fn next(&mut self, path: &Path) -> Result<Option<OsString>, BuildError> {
        // SAFETY: errno_location returns this thread's live errno cell.
        unsafe { *errno_location() = 0 };
        // SAFETY: self owns a live DIR until Drop.
        let entry = unsafe { libc::readdir(self.0) };
        if entry.is_null() {
            // SAFETY: errno_location returns this thread's live errno cell.
            let errno = unsafe { *errno_location() };
            return if errno == 0 {
                Ok(None)
            } else {
                Err(path_error(
                    "read",
                    path,
                    std::io::Error::from_raw_os_error(errno),
                ))
            };
        }
        // SAFETY: readdir returned a live entry whose d_name is NUL-terminated
        // until the next call on this directory stream.
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        Ok(Some(OsString::from_vec(bytes.to_vec())))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: self uniquely owns the DIR returned by fdopendir.
        unsafe { libc::closedir(self.0) };
    }
}

#[cfg(target_os = "macos")]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: libc exposes the current thread's errno pointer.
    unsafe { libc::__error() }
}

#[cfg(target_os = "linux")]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: libc exposes the current thread's errno pointer.
    unsafe { libc::__errno_location() }
}

fn require_same_path(path: &Path, held: &File) -> Result<(), BuildError> {
    let reopened = open_directory(path)?;
    if FileIdentity::capture(held, path)? == FileIdentity::capture(&reopened, path)? {
        Ok(())
    } else {
        Err(moved(path))
    }
}

fn require_child(
    parent: &File,
    name: &OsStr,
    held: &File,
    path: &Path,
    directory: bool,
) -> Result<(), BuildError> {
    let reopened = if directory {
        open_directory_at(parent, name, path)?
    } else {
        open_file_at(parent, name, path)?
    };
    if FileIdentity::capture(held, path)? == FileIdentity::capture(&reopened, path)? {
        Ok(())
    } else {
        Err(moved(path))
    }
}

fn moved(path: &Path) -> BuildError {
    BuildError::new(format!(
        "dSYM member {} moved while captured",
        path.display()
    ))
}

fn path_error(operation: &str, path: &Path, error: std::io::Error) -> BuildError {
    BuildError::new(format!(
        "cannot {operation} dSYM member {}: {error}",
        path.display()
    ))
}
