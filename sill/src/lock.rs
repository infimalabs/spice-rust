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
        formatter.write_str("lock name must be one nonempty path component")
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
fn acquire(lock: &LockFile) -> io::Result<Possession> {
    acquire_with(lock, platform::LOCK_EX)
}

#[cfg(unix)]
fn acquire_with(lock: &LockFile, operation: std::os::raw::c_int) -> io::Result<Possession> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(platform::O_CLOEXEC)
        .open(&lock.path)?;
    let descriptor = file.as_raw_fd();

    let flags = unsafe { platform::fcntl(descriptor, platform::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if flags & platform::FD_CLOEXEC == 0 {
        return Err(io::Error::other(
            "lock descriptor was opened without CLOEXEC",
        ));
    }

    let result = unsafe { platform::flock(descriptor, operation) };
    if result == 0 {
        return Ok(Possession {
            lock: lock.clone(),
            file,
        });
    }
    Err(io::Error::last_os_error())
}

#[cfg(not(unix))]
fn try_acquire(_lock: &LockFile) -> io::Result<Acquisition> {
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
