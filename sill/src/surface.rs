//! Fixed repository-common topology for daemonless resource locking.

use std::error::Error;
#[cfg(unix)]
use std::ffi::{CString, OsStr};
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Acquisition, Availability, ExitCode, LockConfig, LockFile, LockName, Possession};

/// Read-only rendering of the configured lock topology.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockReport {
    /// Repository-common directory under which lock files live.
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

/// One configured lock pool and every fixed shard path it owns.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockPoolReport {
    pub name: String,
    pub shards: u32,
    pub entries: Vec<LockShardReport>,
}

/// One fixed shard path and its point-in-time kernel availability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockShardReport {
    pub shard: u32,
    pub path: String,
    pub availability: LockAvailability,
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
struct TopologyAuthority {
    directory: fs::File,
}

#[cfg(not(unix))]
#[derive(Debug)]
struct TopologyAuthority;

#[cfg(unix)]
#[derive(Debug)]
struct PoolAuthority {
    directory: fs::File,
}

#[cfg(not(unix))]
#[derive(Debug)]
struct PoolAuthority;

/// Repository-common lock topology plus its admitted policy.
#[derive(Clone, Debug)]
pub struct LockTopology {
    root: PathBuf,
    config: LockConfig,
}

impl LockTopology {
    /// Bind an effective configuration to its fixed state directory.
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
        let authority =
            TopologyAuthority::observe(&self.root).map_err(|source| LockSurfaceError::Inspect {
                path: self.root.clone(),
                source,
            })?;
        let pools = self
            .config
            .named()
            .map(|(name, shards)| {
                let pool = authority
                    .as_ref()
                    .map(|authority| authority.pool(name, false))
                    .transpose()
                    .map_err(|source| LockSurfaceError::Inspect {
                        path: self.root.join(name.as_str()),
                        source,
                    })?
                    .flatten();
                let entries = (0..shards.get())
                    .map(|shard| self.observe_shard(pool.as_ref(), name, shard))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LockPoolReport {
                    name: name.as_str().to_owned(),
                    shards: shards.get(),
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
    /// selected pool directory and advisory lock files as acquisition reaches
    /// them. Reporting the topology remains read-only.
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
        let shards = self
            .config
            .shards(&name)
            .ok_or_else(|| LockSurfaceError::UnknownName(name.as_str().to_owned()))?;
        if let Some(shard) = requested
            && shard >= shards.get()
        {
            return Err(LockSurfaceError::ShardOutOfRange {
                name: name.as_str().to_owned(),
                shard,
                shards: shards.get(),
            });
        }

        let pool_path = self.root.join(name.as_str());
        let authority =
            TopologyAuthority::prepare(&self.root).map_err(|source| LockSurfaceError::Prepare {
                path: self.root.clone(),
                source,
            })?;
        let pool = authority
            .pool(&name, true)
            .map_err(|source| LockSurfaceError::Prepare {
                path: pool_path.clone(),
                source,
            })?
            .ok_or_else(|| LockSurfaceError::Prepare {
                path: pool_path,
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "configured lock pool was not created",
                ),
            })?;
        if let Some(shard) = requested {
            return self
                .acquire_shard(&pool, name, shard)
                .map(|attempt| match attempt {
                    Some(held) => LockAttempt::Acquired(held),
                    None => LockAttempt::Contended {
                        kind: Contention::Taken,
                        code: self.config.exits().taken(),
                    },
                });
        }

        for shard in 0..shards.get() {
            if let Some(held) = self.acquire_shard(&pool, name.clone(), shard)? {
                return Ok(LockAttempt::Acquired(held));
            }
        }
        let (kind, code) = if shards.get() == 1 {
            (Contention::Busy, self.config.exits().busy())
        } else {
            (Contention::Full, self.config.exits().full())
        };
        Ok(LockAttempt::Contended { kind, code })
    }

    fn acquire_shard(
        &self,
        pool: &PoolAuthority,
        name: LockName,
        shard: u32,
    ) -> Result<Option<HeldShard>, LockSurfaceError> {
        let path = self.shard_path(&name, shard);
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
            .map_err(|source| LockSurfaceError::Acquire { path, source })?
        {
            Acquisition::Acquired(possession) => Ok(Some(HeldShard { shard, possession })),
            Acquisition::Contended => Ok(None),
        }
    }

    fn shard_path(&self, name: &LockName, shard: u32) -> PathBuf {
        self.root.join(name.as_str()).join(shard.to_string())
    }

    fn observe_shard(
        &self,
        pool: Option<&PoolAuthority>,
        name: &LockName,
        shard: u32,
    ) -> Result<LockShardReport, LockSurfaceError> {
        let path = self.shard_path(name, shard);
        let lock = LockFile::new(name.clone(), &path);
        let availability = match pool {
            None => Availability::Available,
            Some(pool) => {
                match pool
                    .shard(shard, false)
                    .map_err(|source| LockSurfaceError::Inspect {
                        path: path.clone(),
                        source,
                    })? {
                    None => Availability::Available,
                    Some(file) => lock.availability_opened(file).map_err(|source| {
                        LockSurfaceError::Inspect {
                            path: path.clone(),
                            source,
                        }
                    })?,
                }
            }
        };
        Ok(LockShardReport {
            shard,
            path: path.display().to_string(),
            availability: match availability {
                Availability::Available => LockAvailability::Available,
                Availability::Held => LockAvailability::Held,
            },
        })
    }
}

#[cfg(unix)]
impl TopologyAuthority {
    fn observe(root: &Path) -> io::Result<Option<Self>> {
        open_topology(root, false)
    }

    fn prepare(root: &Path) -> io::Result<Self> {
        open_topology(root, true)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "configured lock topology was not created",
            )
        })
    }

    fn pool(&self, name: &LockName, create: bool) -> io::Result<Option<PoolAuthority>> {
        let Some(directory) =
            open_directory_component(&self.directory, OsStr::new(name.as_str()), create)?
        else {
            return Ok(None);
        };
        Ok(Some(PoolAuthority { directory }))
    }
}

#[cfg(not(unix))]
impl TopologyAuthority {
    fn observe(_root: &Path) -> io::Result<Option<Self>> {
        Err(unsupported_authority())
    }

    fn prepare(_root: &Path) -> io::Result<Self> {
        Err(unsupported_authority())
    }

    fn pool(&self, _name: &LockName, _create: bool) -> io::Result<Option<PoolAuthority>> {
        Err(unsupported_authority())
    }
}

#[cfg(unix)]
impl PoolAuthority {
    fn shard(&self, shard: u32, create: bool) -> io::Result<Option<fs::File>> {
        open_shard(&self.directory, &shard.to_string(), create)
    }
}

#[cfg(not(unix))]
impl PoolAuthority {
    fn shard(&self, _shard: u32, _create: bool) -> io::Result<Option<fs::File>> {
        Err(unsupported_authority())
    }
}

#[cfg(unix)]
fn open_topology(root: &Path, create: bool) -> io::Result<Option<TopologyAuthority>> {
    let (anchor, components) = topology_components(root)?;
    let anchor = fs::canonicalize(anchor)?;
    let mut directory = open_absolute_directory(&anchor)?;
    for component in components {
        let Some(next) = open_directory_component(&directory, component, create)? else {
            return Ok(None);
        };
        directory = next;
    }
    Ok(Some(TopologyAuthority { directory }))
}

#[cfg(unix)]
fn topology_components(root: &Path) -> io::Result<(&Path, [&OsStr; 2])> {
    let topology = root.file_name().ok_or_else(invalid_topology_path)?;
    let state = root.parent().ok_or_else(invalid_topology_path)?;
    let state_name = state.file_name().ok_or_else(invalid_topology_path)?;
    let anchor = state.parent().ok_or_else(invalid_topology_path)?;
    if !anchor.is_absolute() {
        return Err(invalid_topology_path());
    }
    Ok((anchor, [state_name, topology]))
}

#[cfg(unix)]
fn open_absolute_directory(path: &Path) -> io::Result<fs::File> {
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
                directory =
                    open_directory_component(&directory, name, false)?.ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("lock topology anchor is missing at {}", path.display()),
                        )
                    })?;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(invalid_topology_path());
            }
        }
    }
    Ok(directory)
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
fn open_shard(parent: &fs::File, shard: &str, create: bool) -> io::Result<Option<fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let shard = path_component(OsStr::new(shard))?;
    let mut flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    if create {
        flags |= libc::O_CREAT;
    }
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), shard.as_ptr(), flags, 0o600) };
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
        "lock topology must be an absolute path with a two-component state suffix",
    )
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
    #[must_use]
    pub const fn shard(&self) -> u32 {
        self.shard
    }

    #[must_use]
    pub const fn possession(&self) -> &Possession {
        &self.possession
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
            Self::InvalidName(_) | Self::UnknownName(_) | Self::ShardOutOfRange { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Contention, LockAttempt, LockAvailability, LockSurfaceError, LockTopology};
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
            Self(path)
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
        let config = LockConfig::new(
            BUSY_EXIT,
            TAKEN_EXIT,
            FULL_EXIT,
            [("single".to_owned(), 1), ("pool".to_owned(), 2)],
        )
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
                .map(|pool| (pool.name.as_str(), pool.shards))
                .collect::<Vec<_>>(),
            [("pool", 2), ("single", 1)]
        );
        assert_eq!(
            report.pools[0]
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            [
                root.join("pool/0").display().to_string(),
                root.join("pool/1").display().to_string(),
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
        assert!(root.join("pool/0").is_file());
        assert!(
            !root.join("pool/1").exists(),
            "observing an absent shard created it"
        );

        drop(held);
        let released = topology.report().expect("released topology is observable");
        let pool = released
            .pools
            .iter()
            .find(|pool| pool.name == "pool")
            .expect("pool is reported");
        assert_eq!(pool.entries[0].availability, LockAvailability::Available);
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
        assert!(!root.exists(), "a refused selection created lock state");
    }
}
