//! Explicit configured topology for daemonless resource locking.

use std::error::Error;
#[cfg(unix)]
use std::ffi::{CString, OsStr};
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process;

use serde::{Deserialize, Serialize};

use crate::{
    Acquisition, Availability, ExitCode, LockConfig, LockFile, LockName, LockTarget, Possession,
};
#[cfg(unix)]
use crate::{Running, Spec};

const OWNER_BYTES_LIMIT: u64 = 64 * 1024;

/// Read-only rendering of the configured lock topology.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockReport {
    /// Repository path base used to resolve relative lock locations.
    pub root: String,
    /// Configured process statuses.
    pub exits: LockExitReport,
    /// Named pools in stable name order.
    pub pools: Vec<LockPoolReport>,
}

/// Process statuses attached to the three contention outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockExitReport {
    pub busy: i32,
    pub taken: i32,
    pub full: i32,
}

/// One configured lock target and every shard path it owns.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockPoolReport {
    pub name: String,
    pub kind: LockTargetKind,
    pub shards: u32,
    pub entries: Vec<LockShardReport>,
}

/// Whether one configured name resolves to a file or a shard directory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockTargetKind {
    File,
    Pool,
}

/// One configured shard path and its point-in-time kernel availability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockShardReport {
    pub shard: u32,
    pub path: String,
    pub availability: LockAvailability,
    pub owner: Option<LockOwnerReport>,
    pub owner_stale: bool,
}

/// Best-effort owner evidence written only after kernel possession succeeds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockOwnerReport {
    pub pid: u32,
    pub cwd: String,
}

/// Whether a nonblocking acquisition would have succeeded when observed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockAvailability {
    Available,
    Held,
}

#[cfg(unix)]
#[derive(Debug)]
struct DirectoryAuthority {
    directory: fs::File,
}

#[cfg(not(unix))]
#[derive(Debug)]
struct DirectoryAuthority;

/// Configured lock topology plus its admitted policy.
#[derive(Clone, Debug)]
pub struct LockTopology {
    root: PathBuf,
    config: LockConfig,
}

impl LockTopology {
    /// Bind an effective configuration to its repository-relative path base.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, config: &LockConfig) -> Self {
        Self {
            root: root.into(),
            config: config.clone(),
        }
    }

    /// Render every configured path and its availability without creating one.
    ///
    /// Existing shard files are opened only for a nonblocking kernel probe;
    /// absent shard files and directories remain absent.
    ///
    /// # Errors
    ///
    /// Returns a typed inspection failure when an existing shard cannot be
    /// opened or observed.
    pub fn report(&self) -> Result<LockReport, LockSurfaceError> {
        let exits = self.config.exits();
        let pools = self
            .config
            .named()
            .map(|(name, target)| {
                let location = self.resolve(target.location().as_path());
                let (kind, shards, entries) = match target {
                    LockTarget::File { .. } => (
                        LockTargetKind::File,
                        1,
                        vec![self.observe_path(name, 0, &location)?],
                    ),
                    LockTarget::Pool { shards, .. } => {
                        let authority =
                            DirectoryAuthority::observe(&location).map_err(|source| {
                                LockSurfaceError::Inspect {
                                    path: location.clone(),
                                    source,
                                }
                            })?;
                        let entries = (0..shards.get())
                            .map(|shard| {
                                let path = location.join(format!("{shard}.lock"));
                                self.observe_shard(authority.as_ref(), name, shard, path)
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        (LockTargetKind::Pool, shards.get(), entries)
                    }
                };
                Ok(LockPoolReport {
                    name: name.as_str().to_owned(),
                    kind,
                    shards,
                    entries,
                })
            })
            .collect::<Result<Vec<_>, LockSurfaceError>>()?;
        Ok(LockReport {
            root: self.root.display().to_string(),
            exits: LockExitReport {
                busy: exits.busy().get(),
                taken: exits.taken().get(),
                full: exits.full().get(),
            },
            pools,
        })
    }

    /// Try one selected shard, or the first free shard in stable order.
    ///
    /// This is the first operation that mutates the topology: it creates the
    /// configured parent directories and advisory lock files as acquisition
    /// reaches them. Reporting the topology remains read-only.
    ///
    /// # Errors
    ///
    /// Returns a typed selection, directory, or advisory-lock failure.
    pub fn try_acquire(
        &self,
        name: &str,
        requested: Option<u32>,
    ) -> Result<LockAttempt, LockSurfaceError> {
        let name = LockName::new(name.to_owned())
            .map_err(|_| LockSurfaceError::InvalidName(name.to_owned()))?;
        let target = self
            .config
            .target(&name)
            .ok_or_else(|| LockSurfaceError::UnknownName(name.as_str().to_owned()))?;
        let location = self.resolve(target.location().as_path());
        match target {
            LockTarget::File { .. } => {
                if let Some(shard) = requested {
                    return Err(LockSurfaceError::ShardOnFile {
                        name: name.as_str().to_owned(),
                        shard,
                    });
                }
                self.acquire_path(name, 0, &location)
                    .map(|held| match held {
                        Some(held) => LockAttempt::Acquired(held),
                        None => LockAttempt::Contended {
                            kind: Contention::Busy,
                            code: self.config.exits().busy(),
                        },
                    })
            }
            LockTarget::Pool { shards, .. } => {
                if let Some(shard) = requested
                    && shard >= shards.get()
                {
                    return Err(LockSurfaceError::ShardOutOfRange {
                        name: name.as_str().to_owned(),
                        shard,
                        shards: shards.get(),
                    });
                }
                let pool = DirectoryAuthority::prepare(&location).map_err(|source| {
                    LockSurfaceError::Prepare {
                        path: location.clone(),
                        source,
                    }
                })?;
                if let Some(shard) = requested {
                    return self
                        .acquire_shard(&pool, name, shard, location.join(format!("{shard}.lock")))
                        .map(|held| match held {
                            Some(held) => LockAttempt::Acquired(held),
                            None => LockAttempt::Contended {
                                kind: Contention::Taken,
                                code: self.config.exits().taken(),
                            },
                        });
                }
                for shard in 0..shards.get() {
                    let path = location.join(format!("{shard}.lock"));
                    if let Some(held) = self.acquire_shard(&pool, name.clone(), shard, path)? {
                        return Ok(LockAttempt::Acquired(held));
                    }
                }
                Ok(LockAttempt::Contended {
                    kind: Contention::Full,
                    code: self.config.exits().full(),
                })
            }
        }
    }

    fn acquire_shard(
        &self,
        pool: &DirectoryAuthority,
        name: LockName,
        shard: u32,
        path: PathBuf,
    ) -> Result<Option<HeldShard>, LockSurfaceError> {
        let lock = LockFile::new(name, &path);
        let file = pool
            .shard(shard, true)
            .map_err(|source| LockSurfaceError::Acquire {
                path: path.clone(),
                source,
            })?
            .ok_or_else(|| LockSurfaceError::Acquire {
                path: path.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, "lock shard was not created"),
            })?;
        match lock
            .try_acquire_opened(file)
            .map_err(|source| LockSurfaceError::Acquire {
                path: path.clone(),
                source,
            })? {
            Acquisition::Acquired(possession) => HeldShard::new(shard, possession)
                .map(Some)
                .map_err(|source| LockSurfaceError::Acquire { path, source }),
            Acquisition::Contended => Ok(None),
        }
    }

    fn acquire_path(
        &self,
        name: LockName,
        shard: u32,
        path: &Path,
    ) -> Result<Option<HeldShard>, LockSurfaceError> {
        let file = open_configured_file(path, true)
            .map_err(|source| LockSurfaceError::Prepare {
                path: path.to_path_buf(),
                source,
            })?
            .ok_or_else(|| LockSurfaceError::Prepare {
                path: path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::NotFound, "lock file was not created"),
            })?;
        let lock = LockFile::new(name, path);
        match lock
            .try_acquire_opened(file)
            .map_err(|source| LockSurfaceError::Acquire {
                path: path.to_path_buf(),
                source,
            })? {
            Acquisition::Acquired(possession) => HeldShard::new(shard, possession)
                .map(Some)
                .map_err(|source| LockSurfaceError::Acquire {
                    path: path.to_path_buf(),
                    source,
                }),
            Acquisition::Contended => Ok(None),
        }
    }

    fn observe_shard(
        &self,
        pool: Option<&DirectoryAuthority>,
        name: &LockName,
        shard: u32,
        path: PathBuf,
    ) -> Result<LockShardReport, LockSurfaceError> {
        let file = pool
            .map(|pool| pool.shard(shard, false))
            .transpose()
            .map_err(|source| LockSurfaceError::Inspect {
                path: path.clone(),
                source,
            })?
            .flatten();
        self.observed(name, shard, path, file)
    }

    fn observe_path(
        &self,
        name: &LockName,
        shard: u32,
        path: &Path,
    ) -> Result<LockShardReport, LockSurfaceError> {
        let file =
            open_configured_file(path, false).map_err(|source| LockSurfaceError::Inspect {
                path: path.to_path_buf(),
                source,
            })?;
        self.observed(name, shard, path.to_path_buf(), file)
    }

    fn observed(
        &self,
        name: &LockName,
        shard: u32,
        path: PathBuf,
        file: Option<fs::File>,
    ) -> Result<LockShardReport, LockSurfaceError> {
        let Some(file) = file else {
            return Ok(LockShardReport {
                shard,
                path: path.display().to_string(),
                availability: LockAvailability::Available,
                owner: None,
                owner_stale: false,
            });
        };
        let owner = read_owner(&file).map_err(|source| LockSurfaceError::Inspect {
            path: path.clone(),
            source,
        })?;
        let availability = LockFile::new(name.clone(), &path)
            .availability_opened(file)
            .map_err(|source| LockSurfaceError::Inspect {
                path: path.clone(),
                source,
            })?;
        Ok(LockShardReport {
            shard,
            path: path.display().to_string(),
            availability: match availability {
                Availability::Available => LockAvailability::Available,
                Availability::Held => LockAvailability::Held,
            },
            owner_stale: availability == Availability::Available && owner.is_some(),
            owner,
        })
    }

    fn resolve(&self, configured: &Path) -> PathBuf {
        if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            self.root.join(configured)
        }
    }
}

#[cfg(unix)]
impl DirectoryAuthority {
    fn observe(path: &Path) -> io::Result<Option<Self>> {
        open_absolute_directory(path, false)
            .map(|directory| directory.map(|directory| Self { directory }))
    }

    fn prepare(path: &Path) -> io::Result<Self> {
        open_absolute_directory(path, true)?
            .map(|directory| Self { directory })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "configured lock directory was not created",
                )
            })
    }
}

#[cfg(unix)]
impl DirectoryAuthority {
    fn shard(&self, shard: u32, create: bool) -> io::Result<Option<fs::File>> {
        open_lock_file(
            &self.directory,
            OsStr::new(&format!("{shard}.lock")),
            create,
        )
    }
}

#[cfg(not(unix))]
impl DirectoryAuthority {
    fn observe(_path: &Path) -> io::Result<Option<Self>> {
        Err(unsupported_authority())
    }

    fn prepare(_path: &Path) -> io::Result<Self> {
        Err(unsupported_authority())
    }

    fn shard(&self, _shard: u32, _create: bool) -> io::Result<Option<fs::File>> {
        Err(unsupported_authority())
    }
}

#[cfg(unix)]
fn open_absolute_directory(path: &Path, create: bool) -> io::Result<Option<fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;

    if !path.is_absolute() {
        return Err(invalid_topology_path());
    }
    let mut directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(Path::new("/"))?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let Some(next) = open_directory_component(&directory, name, create)? else {
                    return Ok(None);
                };
                directory = next;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(invalid_topology_path());
            }
        }
    }
    Ok(Some(directory))
}

#[cfg(unix)]
fn open_configured_file(path: &Path, create: bool) -> io::Result<Option<fs::File>> {
    let parent = path.parent().ok_or_else(invalid_topology_path)?;
    let name = path.file_name().ok_or_else(invalid_topology_path)?;
    let Some(directory) = open_absolute_directory(parent, create)? else {
        return Ok(None);
    };
    open_lock_file(&directory, name, create)
}

#[cfg(not(unix))]
fn open_configured_file(_path: &Path, _create: bool) -> io::Result<Option<fs::File>> {
    Err(unsupported_authority())
}

#[cfg(unix)]
fn open_directory_component(
    parent: &fs::File,
    component: &OsStr,
    create: bool,
) -> io::Result<Option<fs::File>> {
    match open_directory_at(parent, component) {
        Ok(directory) => Ok(Some(directory)),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_directory_at(parent, component)?;
            open_directory_at(parent, component).map(Some)
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("lock directory component {component:?} refuses symlinks: {error}"),
        )),
    }
}

#[cfg(unix)]
fn open_directory_at(parent: &fs::File, component: &OsStr) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let component = path_component(component)?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if descriptor == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { fs::File::from_raw_fd(descriptor) })
    }
}

#[cfg(unix)]
fn create_directory_at(parent: &fs::File, component: &OsStr) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let component = path_component(component)?;
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), component.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[cfg(unix)]
fn open_lock_file(parent: &fs::File, name: &OsStr, create: bool) -> io::Result<Option<fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = path_component(name)?;
    let mut flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    if create {
        flags |= libc::O_CREAT;
    }
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) };
    if descriptor == -1 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound && !create {
            Ok(None)
        } else {
            Err(error)
        }
    } else {
        Ok(Some(unsafe { fs::File::from_raw_fd(descriptor) }))
    }
}

#[cfg(unix)]
fn path_component(component: &OsStr) -> io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(component.as_bytes()).map_err(|_| invalid_topology_path())
}

#[cfg(unix)]
fn invalid_topology_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "lock target must resolve to an absolute path of normal components",
    )
}

fn owner() -> io::Result<LockOwnerReport> {
    Ok(LockOwnerReport {
        pid: process::id(),
        cwd: std::env::current_dir()?.display().to_string(),
    })
}

fn write_owner(file: &fs::File) -> io::Result<()> {
    let bytes = serde_json::to_vec(&owner()?).map_err(io::Error::other)?;
    file.set_len(0)?;
    let mut writer = file;
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    file.sync_data()
}

fn clear_owner(file: &fs::File) -> io::Result<()> {
    file.set_len(0)?;
    file.sync_data()
}

fn read_owner(file: &fs::File) -> io::Result<Option<LockOwnerReport>> {
    let length = file.metadata()?.len();
    if length == 0 || length > OWNER_BYTES_LIMIT {
        return Ok(None);
    }
    let mut reader = file;
    reader.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(length as usize);
    reader.take(OWNER_BYTES_LIMIT).read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes).ok())
}

#[cfg(not(unix))]
fn unsupported_authority() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "directory-authority lock paths require a Unix target",
    )
}

/// One nonblocking pool acquisition.
#[derive(Debug)]
pub enum LockAttempt {
    Acquired(HeldShard),
    Contended { kind: Contention, code: ExitCode },
}

/// Possession of one exact configured shard.
#[derive(Debug)]
pub struct HeldShard {
    shard: u32,
    possession: Possession,
}

impl HeldShard {
    fn new(shard: u32, possession: Possession) -> io::Result<Self> {
        write_owner(possession.file())?;
        Ok(Self { shard, possession })
    }

    #[must_use]
    pub const fn shard(&self) -> u32 {
        self.shard
    }

    #[must_use]
    pub const fn possession(&self) -> &Possession {
        &self.possession
    }

    /// Spawn a child that retains kernel possession if this process dies abruptly.
    ///
    /// The child and descendants inherit the lock descriptor across `exec`. The
    /// descriptor remains close-on-exec in this process and in unrelated spawns.
    #[cfg(unix)]
    pub fn spawn_protected(&mut self, spec: Spec) -> io::Result<Running> {
        let running = crate::spawn::spawn_inheriting(spec, self.possession.file())?;
        self.possession.retain_until_last_close();
        Ok(running)
    }
}

impl Drop for HeldShard {
    fn drop(&mut self) {
        let _ = clear_owner(self.possession.file());
    }
}

/// Why an otherwise valid lock request could not acquire a shard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Contention {
    Busy,
    Taken,
    Full,
}

impl Contention {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Busy => "lock::busy",
            Self::Taken => "lock::taken",
            Self::Full => "lock::full",
        }
    }
}

/// Why a lock request could not reach an acquisition decision.
#[derive(Debug)]
pub enum LockSurfaceError {
    InvalidName(String),
    UnknownName(String),
    ShardOnFile {
        name: String,
        shard: u32,
    },
    ShardOutOfRange {
        name: String,
        shard: u32,
        shards: u32,
    },
    Prepare {
        path: PathBuf,
        source: io::Error,
    },
    Acquire {
        path: PathBuf,
        source: io::Error,
    },
    Inspect {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for LockSurfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "lock name is invalid: {name:?}"),
            Self::UnknownName(name) => write!(formatter, "lock is not configured: {name:?}"),
            Self::ShardOnFile { name, shard } => write!(
                formatter,
                "single-file lock {name:?} does not accept shard {shard}"
            ),
            Self::ShardOutOfRange {
                name,
                shard,
                shards,
            } => write!(
                formatter,
                "lock {name:?} shard {shard} is outside configured capacity {shards}"
            ),
            Self::Prepare { path, source } => {
                write!(formatter, "cannot prepare {}: {source}", path.display())
            }
            Self::Acquire { path, source } => {
                write!(formatter, "cannot acquire {}: {source}", path.display())
            }
            Self::Inspect { path, source } => {
                write!(formatter, "cannot inspect {}: {source}", path.display())
            }
        }
    }
}

impl Error for LockSurfaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Prepare { source, .. }
            | Self::Acquire { source, .. }
            | Self::Inspect { source, .. } => Some(source),
            Self::InvalidName(_)
            | Self::UnknownName(_)
            | Self::ShardOnFile { .. }
            | Self::ShardOutOfRange { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        Contention, LockAttempt, LockAvailability, LockSurfaceError, LockTargetKind, LockTopology,
    };
    use crate::LockConfig;

    const BUSY_EXIT: u64 = 71;
    const TAKEN_EXIT: u64 = 72;
    const FULL_EXIT: u64 = 73;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sill-surface-{label}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("exclusive scratch root is created");
            Self(fs::canonicalize(path).expect("scratch root canonicalizes"))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn topology(root: &Path) -> LockTopology {
        let config: LockConfig = toml::from_str(&format!(
            concat!(
                "busy = {}\n",
                "taken = {}\n",
                "full = {}\n",
                "[single]\npath = \"single.lock\"\n",
                "[pool]\ndirectory = \"pool\"\nshards = 2\n",
            ),
            BUSY_EXIT, TAKEN_EXIT, FULL_EXIT,
        ))
        .expect("fixture configuration is admitted");
        LockTopology::new(root, &config)
    }

    #[test]
    fn report_is_stable_and_does_not_materialize_the_topology() {
        let scratch = Scratch::new("report");
        let root = scratch.path().join("state/locks");
        let report = topology(&root).report().expect("topology is observable");

        assert_eq!(report.root, root.display().to_string());
        assert_eq!(report.exits.busy, BUSY_EXIT as i32);
        assert_eq!(report.exits.taken, TAKEN_EXIT as i32);
        assert_eq!(report.exits.full, FULL_EXIT as i32);
        assert_eq!(
            report
                .pools
                .iter()
                .map(|pool| (pool.name.as_str(), pool.kind, pool.shards))
                .collect::<Vec<_>>(),
            [
                ("pool", LockTargetKind::Pool, 2),
                ("single", LockTargetKind::File, 1),
            ]
        );
        assert_eq!(
            report.pools[0]
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            [
                root.join("pool/0.lock").display().to_string(),
                root.join("pool/1.lock").display().to_string(),
            ]
        );
        assert!(
            report
                .pools
                .iter()
                .flat_map(|pool| &pool.entries)
                .all(|entry| entry.availability == LockAvailability::Available)
        );
        assert!(!root.exists(), "reporting created lock state");
    }

    #[test]
    fn report_observes_held_and_released_shards_without_creating_missing_ones() {
        let scratch = Scratch::new("availability");
        let root = scratch.path().join("locks");
        let topology = topology(&root);
        let held = match topology
            .try_acquire("pool", Some(0))
            .expect("selected shard is observable")
        {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh shard was contended"),
        };

        let report = topology.report().expect("held topology is observable");
        let pool = report
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported");
        assert_eq!(pool.entries[0].availability, LockAvailability::Held);
        assert_eq!(pool.entries[1].availability, LockAvailability::Available);
        assert!(root.join("pool/0.lock").is_file());
        assert!(
            !root.join("pool/1.lock").exists(),
            "observing an absent shard created it"
        );
        assert_eq!(
            pool.entries[0].owner.as_ref().map(|owner| owner.pid),
            Some(std::process::id())
        );
        assert!(!pool.entries[0].owner_stale);

        drop(held);
        let released = topology.report().expect("released topology is observable");
        let pool = released
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported");
        assert_eq!(pool.entries[0].availability, LockAvailability::Available);
        assert_eq!(pool.entries[0].owner, None);
        assert!(!pool.entries[0].owner_stale);
    }

    #[test]
    fn stale_owner_evidence_never_becomes_kernel_possession() {
        let scratch = Scratch::new("stale-owner");
        let root = scratch.path().join("locks");
        let path = root.join("pool/0.lock");
        fs::create_dir_all(path.parent().expect("stale owner parent"))
            .expect("stale owner parent creates");
        fs::write(&path, br#"{"pid":4242,"cwd":"/departed"}"#)
            .expect("stale owner evidence writes");
        let topology = topology(&root);

        let report = topology.report().expect("stale evidence is observable");
        let entry = &report
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported")
            .entries[0];
        assert_eq!(entry.availability, LockAvailability::Available);
        assert_eq!(entry.owner.as_ref().map(|owner| owner.pid), Some(4242));
        assert!(entry.owner_stale);

        let held = match topology
            .try_acquire("pool", Some(0))
            .expect("stale evidence does not contend")
        {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("stale evidence became possession"),
        };
        let current = topology.report().expect("replacement owner is observable");
        let entry = &current
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported")
            .entries[0];
        assert_eq!(entry.availability, LockAvailability::Held);
        assert_eq!(
            entry.owner.as_ref().map(|owner| owner.pid),
            Some(std::process::id())
        );
        assert!(!entry.owner_stale);
        drop(held);
    }

    #[test]
    fn stable_selection_and_each_contention_class_use_configured_codes() {
        let scratch = Scratch::new("contention");
        let topology = topology(&scratch.path().join("locks"));

        let first = match topology.try_acquire("pool", None).expect("first shard") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh pool was contended"),
        };
        assert_eq!(first.shard(), 0);
        let second = match topology.try_acquire("pool", None).expect("second shard") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("second shard was contended"),
        };
        assert_eq!(second.shard(), 1);

        assert!(matches!(
            topology.try_acquire("pool", Some(0)).expect("chosen contention"),
            LockAttempt::Contended {
                kind: Contention::Taken,
                code
            } if code.get() == TAKEN_EXIT as i32
        ));
        assert!(matches!(
            topology.try_acquire("pool", None).expect("pool exhaustion"),
            LockAttempt::Contended {
                kind: Contention::Full,
                code
            } if code.get() == FULL_EXIT as i32
        ));

        let single = match topology.try_acquire("single", None).expect("single lock") {
            LockAttempt::Acquired(held) => held,
            LockAttempt::Contended { .. } => panic!("fresh single lock was contended"),
        };
        assert!(matches!(
            topology.try_acquire("single", None).expect("single contention"),
            LockAttempt::Contended {
                kind: Contention::Busy,
                code
            } if code.get() == BUSY_EXIT as i32
        ));

        drop(first);
        assert!(matches!(
            topology.try_acquire("pool", Some(0)).expect("released shard"),
            LockAttempt::Acquired(held) if held.shard() == 0
        ));
        drop(second);
        drop(single);
    }

    #[test]
    fn invalid_unknown_and_out_of_range_requests_do_not_create_state() {
        let scratch = Scratch::new("selection");
        let root = scratch.path().join("locks");
        let topology = topology(&root);

        assert!(matches!(
            topology.try_acquire("../outside", None),
            Err(LockSurfaceError::InvalidName(_))
        ));
        assert!(matches!(
            topology.try_acquire("missing", None),
            Err(LockSurfaceError::UnknownName(_))
        ));
        assert!(matches!(
            topology.try_acquire("pool", Some(2)),
            Err(LockSurfaceError::ShardOutOfRange { .. })
        ));
        assert!(matches!(
            topology.try_acquire("single", Some(0)),
            Err(LockSurfaceError::ShardOnFile { .. })
        ));
        assert!(!root.exists(), "a refused selection created lock state");
    }
}
