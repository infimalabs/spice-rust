use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;

use windows_sys::Win32::Foundation::{
    GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_FLAGS, ACE_HEADER, ACL, ACL_REVISION, AddAccessAllowedAceEx,
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid,
    GetTokenInformation, INHERIT_ONLY_ACE, InitializeAcl, InitializeSecurityDescriptor,
    OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, TOKEN_INFORMATION_CLASS, TOKEN_OWNER,
    TOKEN_QUERY, TOKEN_USER, TokenOwner, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    FILE_DELETE_CHILD, FILE_EXECUTE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FileBasicInfo, FileIdInfo, FileStandardInfo,
    GetFileInformationByHandleEx, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    READ_CONTROL, SYNCHRONIZE, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, IO_REPARSE_TAG_MOUNT_POINT,
    SECURITY_DESCRIPTOR_REVISION,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{Entry, FileIdentity, FileObject};

/// Access mapped to one owner permission bit (`r`, `w`, or `x`).
const READ_BITS: u32 = 0o4;
const WRITE_BITS: u32 = 0o2;
const EXECUTE_BITS: u32 = 0o1;
const ALL_BITS: u32 = 0o7;

/// Rights every owner grant carries so metadata and security stay readable.
const BASE_RIGHTS: u32 = READ_CONTROL | SYNCHRONIZE | FILE_READ_ATTRIBUTES;

/// The NT object-manager prefix a junction's substitute name requires.
const NT_PREFIX: &str = r"\??\";

/// The four 16-bit name fields that precede a mount-point path buffer.
const MOUNT_POINT_HEADER_BYTES: usize = 8;

pub(super) fn owner_mode(path: &Path) -> io::Result<u32> {
    let file = open(path, READ_CONTROL)?;
    let principal = Principal::current()?;
    let descriptor = Descriptor::read(&file)?;
    let mut owner = 0;
    let mut other = 0;
    if !principal.controls(descriptor.owner) {
        other = ALL_BITS;
    }
    if descriptor.dacl.is_null() {
        return Ok(0o777);
    }
    // SAFETY: a non-null DACL returned by `GetSecurityInfo` lives inside the
    // descriptor this function still owns.
    let count = unsafe { (*descriptor.dacl).AceCount };
    for index in 0..u32::from(count) {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: `index` is below the ACL's own count, and `ace` receives a
        // pointer into the same owned descriptor.
        if unsafe { GetAce(descriptor.dacl, index, &raw mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every ACE begins with an `ACE_HEADER`.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match u32::from(header.AceType) {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                // SAFETY: the header names this ACE an access-allowed ACE,
                // whose SID begins at `SidStart`.
                let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                let sid: PSID = (&raw const allowed.SidStart).cast_mut().cast();
                let granted = bits(allowed.Mask);
                // SAFETY: both SIDs are valid for the duration of the call.
                if unsafe { EqualSid(sid, principal.user()) } != 0 {
                    owner |= granted;
                } else {
                    other |= granted;
                }
            }
            _ => other = ALL_BITS,
        }
    }
    Ok(owner << 6 | other << 3 | other)
}

pub(super) fn set_owner_mode(path: &Path, mode: u32) -> io::Result<()> {
    protect(
        &open(path, READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)?,
        mode,
    )
}

/// Replace the DACL of the object `file` holds open with `WRITE_DAC` by one
/// protected owner-only grant of `mode`'s owner bits.
fn protect(file: &File, mode: u32) -> io::Result<()> {
    let directory = file.metadata()?.is_dir();
    let principal = Principal::current()?;
    let acl = owner_acl(principal.user(), mode >> 6, directory)?;
    // SAFETY: the handle is open with `WRITE_DAC`, and `acl` is an initialized
    // ACL that outlives the call.
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl.as_ptr().cast::<ACL>(),
            ptr::null(),
        )
    };
    win32_status(status)
}

pub(super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let principal = Principal::current()?;
    let acl = owner_acl(principal.user(), ALL_BITS, true)?;
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let raw: PSECURITY_DESCRIPTOR = (&raw mut descriptor).cast();
    // SAFETY: `raw` addresses a writable descriptor-sized buffer, and the DACL
    // it references outlives `CreateDirectoryW`.
    let prepared = unsafe {
        InitializeSecurityDescriptor(raw, SECURITY_DESCRIPTOR_REVISION) != 0
            && SetSecurityDescriptorDacl(raw, 1, acl.as_ptr().cast::<ACL>(), 0) != 0
            && SetSecurityDescriptorControl(raw, SE_DACL_PROTECTED, SE_DACL_PROTECTED) != 0
    };
    if !prepared {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: raw,
        bInheritHandle: 0,
    };
    let path = wide(path)?;
    // SAFETY: `path` is NUL-terminated and `attributes` outlives the call.
    if unsafe { CreateDirectoryW(path.as_ptr(), &raw const attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide(source)?;
    let destination = wide(destination)?;
    // SAFETY: both paths are NUL-terminated. Without
    // `MOVEFILE_REPLACE_EXISTING`, an existing destination refuses the move.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn directory_link_form(target: &Path) -> io::Result<PathBuf> {
    drive_form(&std::path::absolute(target)?).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a directory junction needs an absolute local target: {target:?}"),
        )
    })
}

pub(super) fn link_directory(target: &Path, link: &Path) -> io::Result<()> {
    let target = directory_link_form(target)?;
    let buffer = mount_point(&target)?;
    let wide_link = wide(link)?;
    // SAFETY: `wide_link` is NUL-terminated; the new directory inherits its
    // parent's protection.
    if unsafe { CreateDirectoryW(wide_link.as_ptr(), ptr::null()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = open(link, FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES).and_then(|file| {
        let mut returned = 0;
        // SAFETY: the handle is open for writing, the input buffer is a
        // complete mount-point reparse buffer of the stated length, and the
        // call completes synchronously.
        let installed = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
                ptr::null_mut(),
                0,
                &raw mut returned,
                ptr::null_mut(),
            )
        };
        if installed == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    });
    if result.is_err() {
        let _ = fs::remove_dir(link);
    }
    result
}

pub(super) fn read_directory_link(link: &Path) -> io::Result<PathBuf> {
    let target = fs::read_link(link)?;
    Ok(drive_form(&target).unwrap_or(target))
}

pub(super) fn allocated_bytes(path: &Path, _metadata: &Metadata) -> io::Result<u64> {
    let standard: FILE_STANDARD_INFO =
        information(&open(path, FILE_READ_ATTRIBUTES)?, FileStandardInfo)?;
    Ok(u64::try_from(standard.AllocationSize).unwrap_or(0))
}

pub(super) fn is_link_like(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(super) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let id: FILE_ID_INFO = information(file, FileIdInfo)?;
    let basic: FILE_BASIC_INFO = information(file, FileBasicInfo)?;
    let standard: FILE_STANDARD_INFO = information(file, FileStandardInfo)?;
    Ok(FileIdentity {
        object: FileObject {
            volume: id.VolumeSerialNumber,
            index: id.FileId.Identifier,
        },
        bytes: u64::try_from(standard.EndOfFile).unwrap_or(0),
        modified: i128::from(basic.LastWriteTime),
        changed: i128::from(basic.ChangeTime),
    })
}

pub(super) fn path_identity(path: &Path) -> io::Result<FileIdentity> {
    file_identity(&open(path, FILE_READ_ATTRIBUTES)?)
}

pub(super) fn owned_by_current_user(path: &Path) -> io::Result<bool> {
    let descriptor = Descriptor::read(&open(path, READ_CONTROL)?)?;
    Ok(Principal::current()?.controls(descriptor.owner))
}

/// One fixed-size `GetFileInformationByHandleEx` record for `file`.
fn information<T: Default>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    let mut record = T::default();
    // SAFETY: the handle is open, and the output buffer is exactly one `T`,
    // the record type `class` names.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            class,
            (&raw mut record).cast(),
            size_of::<T>() as u32,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(record)
}

pub(super) fn open_unfollowed(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Hold a directory without delete sharing: while the handle is open, the
/// directory can be neither renamed nor removed, so its path keeps naming it.
///
/// Share modes bind only handles holding data access, so the hold lists the
/// directory; attribute access alone would leave a rename unobstructed.
pub(super) fn hold_directory(path: &Path) -> io::Result<File> {
    let directory = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = directory.metadata()?;
    if is_link_like(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a held directory cannot be a link or reparse point: {path:?}"),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok(directory)
}

pub(super) fn create_private_entry(
    _directory: &File,
    path: &Path,
    name: &OsStr,
) -> io::Result<File> {
    let entry = path.join(name);
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&entry);
    let file = match created {
        Ok(file) => file,
        // An existing directory or directory link refuses `CREATE_NEW` as
        // access denied rather than as a collision.
        Err(error)
            if error.kind() == io::ErrorKind::PermissionDenied
                && fs::symlink_metadata(&entry).is_ok() =>
        {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, error));
        }
        Err(error) => return Err(error),
    };
    protect(&file, 0o600)?;
    Ok(file)
}

pub(super) fn open_entry(
    _directory: &File,
    path: &Path,
    name: &OsStr,
    write: bool,
) -> io::Result<Entry> {
    let opened = OpenOptions::new()
        .read(true)
        .write(write)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path.join(name));
    let file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Entry::Missing),
        Err(error) => return Err(error),
    };
    if is_link_like(&file.metadata()?) {
        return Ok(Entry::Link);
    }
    Ok(Entry::Opened(file))
}

pub(super) fn replace_entry(
    _directory: &File,
    path: &Path,
    source: &OsStr,
    destination: &OsStr,
) -> io::Result<()> {
    let source = wide(&path.join(source))?;
    let destination = wide(&path.join(destination))?;
    // SAFETY: both paths are NUL-terminated and outlive the call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn remove_entry(_directory: &File, path: &Path, name: &OsStr) -> io::Result<()> {
    fs::remove_file(path.join(name))
}

/// Flush the held directory. `FlushFileBuffers` needs write access, which the
/// held handle deliberately lacks; the path still names the held directory.
pub(super) fn sync_directory(_directory: &File, path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?
        .sync_all()
}

/// Open the entry itself, including a directory or a reparse point.
fn open(path: &Path, access: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// The current token's user and default owner.
struct Principal {
    user: Vec<usize>,
    owner: Vec<usize>,
}

impl Principal {
    fn current() -> io::Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: the pseudo-handle names this process, and `token` receives a
        // new handle that `OwnedHandle` closes.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `OpenProcessToken` succeeded, so `token` is owned here.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        Ok(Self {
            user: token_information(&token, TokenUser)?,
            owner: token_information(&token, TokenOwner)?,
        })
    }

    fn user(&self) -> PSID {
        // SAFETY: `user` holds a `TOKEN_USER` whose SID points into itself.
        unsafe { (*self.user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn default_owner(&self) -> PSID {
        // SAFETY: `owner` holds a `TOKEN_OWNER` whose SID points into itself.
        unsafe { (*self.owner.as_ptr().cast::<TOKEN_OWNER>()).Owner }
    }

    /// Whether `owner` is this token's user or the default owner it assigns
    /// to new objects, which is Administrators for an elevated token.
    fn controls(&self, owner: PSID) -> bool {
        // SAFETY: all SIDs are valid for the duration of the calls.
        !owner.is_null()
            && unsafe {
                EqualSid(owner, self.user()) != 0 || EqualSid(owner, self.default_owner()) != 0
            }
    }
}

fn token_information(
    token: &OwnedHandle,
    class: TOKEN_INFORMATION_CLASS,
) -> io::Result<Vec<usize>> {
    let mut needed = 0;
    // SAFETY: a null buffer of length zero asks only for the required length.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            ptr::null_mut(),
            0,
            &raw mut needed,
        );
    }
    let mut buffer = vec![0_usize; (needed as usize).div_ceil(size_of::<usize>())];
    // SAFETY: `buffer` is pointer-aligned and at least `needed` bytes long.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer)
}

/// An owned security descriptor with its owner and DACL pointers.
struct Descriptor {
    owner: PSID,
    dacl: *mut ACL,
    raw: PSECURITY_DESCRIPTOR,
}

impl Descriptor {
    fn read(file: &File) -> io::Result<Self> {
        let mut descriptor = Self {
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
            raw: ptr::null_mut(),
        };
        // SAFETY: the handle is open with `READ_CONTROL`; the out-pointers
        // receive pointers into one allocation that `Drop` frees.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &raw mut descriptor.owner,
                ptr::null_mut(),
                &raw mut descriptor.dacl,
                ptr::null_mut(),
                &raw mut descriptor.raw,
            )
        };
        win32_status(status)?;
        Ok(descriptor)
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `GetSecurityInfo` allocated `raw` with `LocalAlloc`.
            unsafe { LocalFree(self.raw) };
        }
    }
}

/// A protected owner-only DACL granting `bits` to `user`.
///
/// A directory also carries inherit-only grants: subdirectories receive
/// `rwx` and files receive `rw-`, the equivalent of a `077` umask.
fn owner_acl(user: PSID, bits: u32, directory: bool) -> io::Result<Vec<u32>> {
    let object = (0, rights(bits & ALL_BITS, directory));
    let entries: &[(ACE_FLAGS, u32)] = if directory {
        &[
            object,
            (
                CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
                rights(ALL_BITS, true),
            ),
            (
                OBJECT_INHERIT_ACE | INHERIT_ONLY_ACE,
                rights(READ_BITS | WRITE_BITS, false),
            ),
        ]
    } else {
        &[object]
    };
    // SAFETY: `user` is a valid SID.
    let sid_bytes = unsafe { GetLengthSid(user) } as usize;
    let ace_bytes = size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_bytes;
    let acl_bytes = size_of::<ACL>() + ace_bytes * entries.len();
    let mut buffer = vec![0_u32; acl_bytes.div_ceil(size_of::<u32>())];
    let acl = buffer.as_mut_ptr().cast::<ACL>();
    // SAFETY: `buffer` is DWORD-aligned and `acl_bytes` long, enough for the
    // header and every ACE added below.
    if unsafe { InitializeAcl(acl, acl_bytes as u32, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    for &(flags, mask) in entries {
        // SAFETY: as above; `user` stays valid for the call.
        if unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION, flags, mask, user) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(buffer)
}

/// Rights for owner permission `bits`.
///
/// Write never includes `DELETE`; removing an entry needs its parent's write
/// bit, which carries `FILE_DELETE_CHILD`.
fn rights(bits: u32, directory: bool) -> u32 {
    let mut mask = BASE_RIGHTS;
    if bits & READ_BITS != 0 {
        mask |= FILE_GENERIC_READ;
    }
    if bits & WRITE_BITS != 0 {
        mask |= FILE_GENERIC_WRITE;
        if directory {
            mask |= FILE_DELETE_CHILD;
        }
    }
    if bits & EXECUTE_BITS != 0 {
        mask |= FILE_GENERIC_EXECUTE;
    }
    mask
}

/// Permission bits one grant confers, counting any right that can alter the
/// object or its protection as write.
fn bits(mask: u32) -> u32 {
    let mut bits = 0;
    if mask & (GENERIC_ALL | GENERIC_READ | FILE_READ_DATA) != 0 {
        bits |= READ_BITS;
    }
    if mask
        & (GENERIC_ALL
            | GENERIC_WRITE
            | FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_DELETE_CHILD
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER)
        != 0
    {
        bits |= WRITE_BITS;
    }
    if mask & (GENERIC_ALL | GENERIC_EXECUTE | FILE_EXECUTE) != 0 {
        bits |= EXECUTE_BITS;
    }
    bits
}

/// A complete `IO_REPARSE_TAG_MOUNT_POINT` reparse buffer naming `target`.
fn mount_point(target: &Path) -> io::Result<Vec<u8>> {
    let print = target.as_os_str().encode_wide().collect::<Vec<_>>();
    let substitute = OsStr::new(NT_PREFIX)
        .encode_wide()
        .chain(print.iter().copied())
        .collect::<Vec<_>>();
    let unit = size_of::<u16>();
    let substitute_bytes = substitute.len() * unit;
    let print_bytes = print.len() * unit;
    let names = substitute_bytes + unit + print_bytes + unit;
    let too_long = || io::Error::new(io::ErrorKind::InvalidInput, "junction target is too long");
    let data = u16::try_from(MOUNT_POINT_HEADER_BYTES + names).map_err(|_| too_long())?;
    let substitute_length = u16::try_from(substitute_bytes).map_err(|_| too_long())?;
    let print_offset = u16::try_from(substitute_bytes + unit).map_err(|_| too_long())?;
    let print_length = u16::try_from(print_bytes).map_err(|_| too_long())?;
    let mut buffer = Vec::with_capacity(MOUNT_POINT_HEADER_BYTES + usize::from(data));
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&data.to_le_bytes());
    buffer.extend_from_slice(&0_u16.to_le_bytes());
    for field in [0, substitute_length, print_offset, print_length] {
        buffer.extend_from_slice(&field.to_le_bytes());
    }
    for name in [&substitute, &print] {
        for unit in name.iter().chain([&0]) {
            buffer.extend_from_slice(&unit.to_le_bytes());
        }
    }
    Ok(buffer)
}

/// `path` as a `C:\...` drive path, or `None` for any other prefix.
fn drive_form(path: &Path) -> Option<PathBuf> {
    match path.components().next()? {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(_) => Some(path.to_path_buf()),
            Prefix::VerbatimDisk(_) => {
                let units = path.as_os_str().encode_wide().skip(4).collect::<Vec<_>>();
                Some(PathBuf::from(OsString::from_wide(&units)))
            }
            _ => None,
        },
        _ => None,
    }
}

/// A NUL-terminated extended-length form of `path` for direct Win32 calls.
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let absolute = std::path::absolute(path)?;
    let text = absolute.as_os_str().encode_wide().collect::<Vec<_>>();
    let verbatim = OsStr::new(r"\\?\").encode_wide().collect::<Vec<_>>();
    let mut converted = match absolute.components().next() {
        Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim() => text,
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(..)) => verbatim
            .into_iter()
            .chain(OsStr::new("UNC").encode_wide())
            .chain(text.into_iter().skip(1))
            .collect(),
        _ => verbatim.into_iter().chain(text).collect(),
    };
    if converted.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    converted.push(0);
    Ok(converted)
}

fn win32_status(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}
