use std::error::Error;
use std::fmt;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct LockName(String);

impl LockName {
    pub fn new(name: impl Into<String>) -> Result<Self, LockNameError> {
        let name = name.into();
        if name.is_empty()
            || matches!(name.as_str(), "." | "..")
            || name
                .chars()
                .any(|character| character == '/' || character == '\\' || character == '\0')
        {
            return Err(LockNameError);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LockNameError;

impl fmt::Display for LockNameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("lock name must be one normal path component")
    }
}

impl Error for LockNameError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockFile {
    name: LockName,
    path: PathBuf,
}

impl LockFile {
    pub fn new(name: LockName, path: impl Into<PathBuf>) -> Self {
        Self {
            name,
            path: path.into(),
        }
    }

    pub fn name(&self) -> &LockName {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn try_acquire(&self) -> io::Result<Acquisition> {
        try_acquire(self)
    }

    /// Try to possess a caller-opened lock file without reopening its pathname.
    ///
    /// The caller owns how the descriptor is resolved. The descriptor must be
    /// a close-on-exec regular file opened for reading and writing.
    pub fn try_acquire_opened(&self, file: File) -> io::Result<Acquisition> {
        try_acquire_opened(self, file)
    }

    /// Observe whether the kernel would grant this existing lock now.
    ///
    /// Missing lock files are available. This operation never creates a file;
    /// an existing file is opened without following symlinks and held only for
    /// the duration of the nonblocking kernel probe.
    ///
    /// # Errors
    ///
    /// Returns an operating-system refusal opening or probing an existing
    /// target.
    pub fn availability(&self) -> io::Result<Availability> {
        availability(self)
    }

    pub(crate) fn availability_opened(&self, file: File) -> io::Result<Availability> {
        availability_opened(file)
    }

    /// Block until the kernel grants exclusive possession.
    ///
    /// # Errors
    ///
    /// Returns an operating-system refusal opening or possessing the target.
    pub fn acquire(&self) -> io::Result<Possession> {
        acquire(self)
    }
}

#[derive(Debug)]
pub enum Acquisition {
    Acquired(Possession),
    Contended,
}

/// One point-in-time observation of a configured lock file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Held,
}

#[derive(Debug)]
pub struct Possession {
    lock: LockFile,
    file: File,
}

impl Possession {
    pub fn lock(&self) -> &LockFile {
        &self.lock
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

#[cfg(unix)]
fn try_acquire(lock: &LockFile) -> io::Result<Acquisition> {
    match acquire_with(lock, platform::LOCK_EX | platform::LOCK_NB) {
        Ok(possession) => Ok(Acquisition::Acquired(possession)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Acquisition::Contended),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn try_acquire_opened(lock: &LockFile, file: File) -> io::Result<Acquisition> {
    match acquire_opened(lock, file, platform::LOCK_EX | platform::LOCK_NB) {
        Ok(possession) => Ok(Acquisition::Acquired(possession)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Acquisition::Contended),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn acquire(lock: &LockFile) -> io::Result<Possession> {
    acquire_with(lock, platform::LOCK_EX)
}

#[cfg(unix)]
fn acquire_with(lock: &LockFile, operation: std::os::raw::c_int) -> io::Result<Possession> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(platform::O_CLOEXEC | platform::O_NOFOLLOW)
        .open(&lock.path)?;
    acquire_opened(lock, file, operation)
}

#[cfg(unix)]
fn acquire_opened(
    lock: &LockFile,
    file: File,
    operation: std::os::raw::c_int,
) -> io::Result<Possession> {
    use std::os::fd::AsRawFd;

    require_regular(&file)?;
    ensure_cloexec(&file)?;

    let result = unsafe { platform::flock(file.as_raw_fd(), operation) };
    if result == 0 {
        return Ok(Possession {
            lock: lock.clone(),
            file,
        });
    }
    Err(io::Error::last_os_error())
}

#[cfg(unix)]
fn availability(lock: &LockFile) -> io::Result<Availability> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(platform::O_CLOEXEC | platform::O_NOFOLLOW)
        .open(&lock.path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Availability::Available);
        }
        Err(error) => return Err(error),
    };
    availability_opened(file)
}

#[cfg(unix)]
fn availability_opened(file: File) -> io::Result<Availability> {
    use std::os::fd::AsRawFd;

    require_regular(&file)?;
    ensure_cloexec(&file)?;
    let result =
        unsafe { platform::flock(file.as_raw_fd(), platform::LOCK_EX | platform::LOCK_NB) };
    if result == 0 {
        return Ok(Availability::Available);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(Availability::Held)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn require_regular(file: &File) -> io::Result<()> {
    if file.metadata()?.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "lock target is not a regular file",
        ))
    }
}

#[cfg(unix)]
fn ensure_cloexec(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let flags = unsafe { platform::fcntl(file.as_raw_fd(), platform::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if flags & platform::FD_CLOEXEC == 0 {
        return Err(io::Error::other(
            "lock descriptor was opened without CLOEXEC",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn try_acquire(_lock: &LockFile) -> io::Result<Acquisition> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock possession requires a Unix target",
    ))
}

#[cfg(not(unix))]
fn try_acquire_opened(_lock: &LockFile, _file: File) -> io::Result<Acquisition> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock possession requires a Unix target",
    ))
}

#[cfg(not(unix))]
fn acquire(_lock: &LockFile) -> io::Result<Possession> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock possession requires a Unix target",
    ))
}

#[cfg(not(unix))]
fn availability(_lock: &LockFile) -> io::Result<Availability> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock observation requires a Unix target",
    ))
}

#[cfg(not(unix))]
fn availability_opened(_file: File) -> io::Result<Availability> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "flock observation requires a Unix target",
    ))
}

#[cfg(unix)]
impl Drop for Possession {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;

        let _result = unsafe { platform::flock(self.file.as_raw_fd(), platform::LOCK_UN) };
    }
}

#[cfg(not(unix))]
impl Drop for Possession {
    fn drop(&mut self) {}
}

#[cfg(unix)]
mod platform {
    use std::os::raw::c_int;

    pub const F_GETFD: c_int = 1;
    pub const FD_CLOEXEC: c_int = 1;
    pub const LOCK_EX: c_int = 2;
    pub const LOCK_NB: c_int = 4;
    pub const LOCK_UN: c_int = 8;

    #[cfg(target_os = "macos")]
    pub const O_CLOEXEC: c_int = 0x01_00_00_00;
    #[cfg(target_os = "linux")]
    pub const O_CLOEXEC: c_int = 0x8_00_00;
    #[cfg(target_os = "macos")]
    pub const O_NOFOLLOW: c_int = 0x100;
    #[cfg(target_os = "linux")]
    pub const O_NOFOLLOW: c_int = 0x2_00_00;

    unsafe extern "C" {
        pub fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
        pub fn flock(descriptor: c_int, operation: c_int) -> c_int;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Acquisition, LockFile, LockName};
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn possession_is_exclusive_and_release_is_automatic() {
        let path = unique_lock_path("exclusive");
        let lock = LockFile::new(LockName::new("exclusive").expect("valid name"), &path);
        let first = acquired(lock.try_acquire().expect("first acquisition"));

        assert!(matches!(
            lock.try_acquire().expect("contending acquisition"),
            Acquisition::Contended
        ));

        drop(first);
        let second = acquired(lock.try_acquire().expect("acquisition after release"));
        drop(second);
        fs::remove_file(path).expect("remove test lock file");
    }

    #[test]
    fn lock_names_are_single_safe_path_components() {
        for name in [
            "",
            ".",
            "..",
            "../outside",
            "with/slash",
            "with\\backslash",
            "nul\0byte",
        ] {
            assert!(LockName::new(name).is_err(), "{name:?} must be refused");
        }
        for name in ["worker", "a.b", "two-words", "UPPER", "9"] {
            assert_eq!(
                LockName::new(name).expect("normal path component").as_str(),
                name
            );
        }
    }

    fn acquired(acquisition: Acquisition) -> super::Possession {
        match acquisition {
            Acquisition::Acquired(possession) => possession,
            Acquisition::Contended => panic!("lock should have been acquired"),
        }
    }

    pub(crate) fn unique_lock_path(stem: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must follow Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("sill-{stem}-{}-{nonce}.lock", process::id()))
    }
}
