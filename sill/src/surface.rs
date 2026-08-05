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

#[cfg(unix)]
pub(crate) mod process_state;
#[cfg(unix)]
use process_state::{process_absent, process_group_terminal, process_terminal};

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LockOwnerEvidence {
    pid: u32,
    cwd: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    owner_released: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_guardian: Option<u32>,
}

impl LockOwnerEvidence {
    fn report(&self) -> Option<LockOwnerReport> {
        (!self.owner_released).then(|| LockOwnerReport {
            pid: self.pid,
            cwd: self.cwd.clone(),
        })
    }

    /// What this record says about possession that outlived the owner naming it.
    #[cfg(unix)]
    fn guardian_state(&self) -> GuardianState {
        let finished = self.protected_group.is_some_and(process_group_terminal)
            && (self.owner_released || process_terminal(self.pid));
        if !finished {
            return GuardianState::Unbounded;
        }
        match self.protected_guardian {
            Some(guardian) if !process_absent(guardian) => GuardianState::Pending,
            Some(_) => GuardianState::Departed,
            None => GuardianState::Unbounded,
        }
    }
}

/// How a record's protected topology bounds a reader that found the lock held.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuardianState {
    /// A guardian is still holding possession for work that has already ended,
    /// so waiting for it costs only the moment it takes to notice.
    Pending,
    /// The guardian named here is gone, and a guardian releases what it held on
    /// its way out, so whatever the probe saw was not this record's possession.
    Departed,
    /// Nothing here bounds a wait: the protected work may still be running, or
    /// no guardian was ever named to end it.
    Unbounded,
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
        match synchronized_acquire(&lock, file).map_err(|source| LockSurfaceError::Acquire {
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
        match synchronized_acquire(&lock, file).map_err(|source| LockSurfaceError::Acquire {
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
        let owner_evidence = read_owner(&file).map_err(|source| LockSurfaceError::Inspect {
            path: path.clone(),
            source,
        })?;
        let availability = synchronized_availability(
            &LockFile::new(name.clone(), &path),
            file,
            owner_evidence.as_ref(),
        )
        .map_err(|source| LockSurfaceError::Inspect {
            path: path.clone(),
            source,
        })?;
        let owner = owner_evidence.as_ref().and_then(LockOwnerEvidence::report);
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

#[cfg(unix)]
fn synchronized_acquire(lock: &LockFile, file: fs::File) -> io::Result<Acquisition> {
    let owner = read_owner(&file)?;
    match lock.try_acquire_opened(file.try_clone()?)? {
        Acquisition::Acquired(possession) => Ok(Acquisition::Acquired(possession)),
        Acquisition::Contended => resolve_refusal(
            guardian_state(owner.as_ref()),
            Acquisition::Contended,
            || {
                lock.acquire_opened(file.try_clone()?)
                    .map(Acquisition::Acquired)
            },
            || lock.try_acquire_opened(file.try_clone()?),
        ),
    }
}

/// Answer a refusal the kernel already returned against what the record bounds.
///
/// A guardian still holding possession for work that has finished is worth
/// waiting out. A guardian that departed between the probe and this reading
/// released what the probe saw, so the question goes back to the kernel rather
/// than being quoted from a holder that no longer exists. Nothing else bounds
/// the wait, and the refusal as collected stands.
///
/// Both answers are taken as callables because the departure this distinguishes
/// occupies only the moment between collecting a refusal and reading the record
/// against it. No caller can hold that window open, so the choice is made
/// decidable here instead of being observable only by reproducing the race.
#[cfg(unix)]
fn resolve_refusal<T>(
    state: GuardianState,
    collected: T,
    wait: impl FnOnce() -> io::Result<T>,
    reask: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    match state {
        GuardianState::Pending => wait(),
        GuardianState::Departed => reask(),
        GuardianState::Unbounded => Ok(collected),
    }
}

/// What one optional owner record says about waiting for the lock it describes.
#[cfg(unix)]
fn guardian_state(owner: Option<&LockOwnerEvidence>) -> GuardianState {
    owner.map_or(GuardianState::Unbounded, LockOwnerEvidence::guardian_state)
}

#[cfg(not(unix))]
fn synchronized_acquire(lock: &LockFile, file: fs::File) -> io::Result<Acquisition> {
    lock.try_acquire_opened(file)
}

#[cfg(unix)]
fn synchronized_availability(
    lock: &LockFile,
    file: fs::File,
    owner: Option<&LockOwnerEvidence>,
) -> io::Result<Availability> {
    let availability = lock.availability_opened(file.try_clone()?)?;
    if availability == Availability::Available {
        return Ok(availability);
    }
    resolve_refusal(
        guardian_state(owner),
        availability,
        || {
            drop(lock.acquire_opened(file.try_clone()?)?);
            Ok(Availability::Available)
        },
        || lock.availability_opened(file.try_clone()?),
    )
}

#[cfg(not(unix))]
fn synchronized_availability(
    lock: &LockFile,
    file: fs::File,
    _owner: Option<&LockOwnerEvidence>,
) -> io::Result<Availability> {
    lock.availability_opened(file)
}

fn owner(protected: Option<(u32, u32)>, owner_released: bool) -> io::Result<LockOwnerEvidence> {
    Ok(LockOwnerEvidence {
        pid: process::id(),
        cwd: std::env::current_dir()?.display().to_string(),
        owner_released,
        protected_group: protected.map(|(group, _)| group),
        protected_guardian: protected.map(|(_, guardian)| guardian),
    })
}

fn write_owner(
    file: &fs::File,
    protected: Option<(u32, u32)>,
    owner_released: bool,
) -> io::Result<()> {
    let record =
        serde_json::to_vec(&owner(protected, owner_released)?).map_err(io::Error::other)?;
    let spanned = file.metadata()?.len().min(OWNER_BYTES_LIMIT);
    let bytes = replacement_record(record, spanned);
    let mut writer = file;
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    file.sync_data()
}

/// One record covering every byte the file already spans, ready to be written whole.
///
/// An empty file is how release is spelled, so emptying the file before writing
/// leaves a window in which the lock states that nothing holds it while a
/// guardian still does. A reader inside that window refuses an acquisition the
/// evidence would have granted, and an owner killed inside it freezes the window
/// open for as long as the file survives. Padding the record out to the span it
/// replaces lets one write supersede the whole record, so every reader sees
/// either the record before it or the record after it.
fn replacement_record(mut record: Vec<u8>, spanned: u64) -> Vec<u8> {
    record.push(b'\n');
    let spanned = usize::try_from(spanned).unwrap_or(usize::MAX);
    record.resize(record.len().max(spanned), b' ');
    record
}

fn clear_owner(file: &fs::File) -> io::Result<()> {
    file.set_len(0)?;
    file.sync_data()
}

fn read_owner(file: &fs::File) -> io::Result<Option<LockOwnerEvidence>> {
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
    #[cfg(unix)]
    protected: Option<(u32, u32)>,
}

impl HeldShard {
    fn new(shard: u32, possession: Possession) -> io::Result<Self> {
        write_owner(possession.file(), None, false)?;
        Ok(Self {
            shard,
            possession,
            #[cfg(unix)]
            protected: None,
        })
    }

    #[must_use]
    pub const fn shard(&self) -> u32 {
        self.shard
    }

    #[must_use]
    pub const fn possession(&self) -> &Possession {
        &self.possession
    }

    /// Spawn a child whose process group retains possession if this process dies.
    ///
    /// An independent guardian holds the descriptor until every member of the
    /// child's new process group is terminally absent.
    #[cfg(unix)]
    pub fn spawn_protected(&mut self, spec: Spec) -> io::Result<Running> {
        let file = self.possession.file();
        let running = crate::spawn::spawn_guarded(spec, file, &mut |group, guardian| {
            write_owner(file, Some((group, guardian)), false)
        })?;
        let guardian = running
            .guardian_id()
            .expect("protected spawn retains a guardian");
        self.protected = Some((running.id(), guardian));
        self.possession.retain_until_last_close();
        Ok(running)
    }
}

impl Drop for HeldShard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some((group, guardian)) = self.protected
            && !process_absent(guardian)
        {
            let _ = write_owner(self.possession.file(), Some((group, guardian)), true);
            return;
        }
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
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::process_state::{
        process_group_absent, process_group_terminal, process_terminal, process_zombie,
    };
    use super::{
        Contention, GuardianState, LockAttempt, LockAvailability, LockOwnerEvidence,
        LockSurfaceError, LockTargetKind, LockTopology, replacement_record, resolve_refusal,
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

    /// A departed guardian must send the question back to the kernel, and only
    /// a departed guardian may. The difference is otherwise visible only inside
    /// the moment between collecting a refusal and reading the record against
    /// it, so what each state does is asserted here rather than raced for.
    #[test]
    fn only_a_departed_guardian_puts_a_collected_refusal_back_to_the_kernel() {
        fn resolved(state: GuardianState) -> (&'static str, u32, u32) {
            let waited = Cell::new(0);
            let reasked = Cell::new(0);
            let answer = resolve_refusal(
                state,
                "collected",
                || {
                    waited.set(waited.get() + 1);
                    Ok("waited")
                },
                || {
                    reasked.set(reasked.get() + 1);
                    Ok("reasked")
                },
            );
            (
                answer.expect("every state answers"),
                waited.get(),
                reasked.get(),
            )
        }

        assert_eq!(resolved(GuardianState::Departed), ("reasked", 0, 1));
        assert_eq!(resolved(GuardianState::Pending), ("waited", 1, 0));
        assert_eq!(resolved(GuardianState::Unbounded), ("collected", 0, 0));
    }

    #[test]
    fn guardian_synchronization_separates_a_pending_guardian_from_a_departed_one() {
        let mut departed = Command::new("/usr/bin/true");
        departed.process_group(0);
        let mut departed = departed.spawn().expect("departed process launches");
        let pid = departed.id();
        assert!(departed.wait().expect("departed process reaps").success());

        let completed = LockOwnerEvidence {
            pid,
            cwd: "/departed".to_owned(),
            owner_released: false,
            protected_group: Some(pid),
            protected_guardian: Some(std::process::id()),
        };
        assert_eq!(completed.guardian_state(), GuardianState::Pending);

        let live_owner = LockOwnerEvidence {
            pid: std::process::id(),
            cwd: "/current".to_owned(),
            owner_released: false,
            protected_group: Some(pid),
            protected_guardian: Some(std::process::id()),
        };
        assert_eq!(live_owner.guardian_state(), GuardianState::Unbounded);

        let released_owner = LockOwnerEvidence {
            owner_released: true,
            ..live_owner
        };
        assert!(released_owner.report().is_none());
        assert_eq!(released_owner.guardian_state(), GuardianState::Pending);

        let unguarded = LockOwnerEvidence {
            pid,
            cwd: "/departed".to_owned(),
            owner_released: false,
            protected_group: None,
            protected_guardian: None,
        };
        assert_eq!(unguarded.guardian_state(), GuardianState::Unbounded);

        let departed_guardian = LockOwnerEvidence {
            pid,
            cwd: "/departed".to_owned(),
            owner_released: false,
            protected_group: Some(pid),
            protected_guardian: Some(pid),
        };
        // The guardian is gone rather than merely uninteresting, and a guardian
        // releases what it held on its way out. Collapsing this into the same
        // answer as an unbounded record is what made a reader quote a refusal
        // its holder had already withdrawn.
        assert_eq!(departed_guardian.guardian_state(), GuardianState::Departed);
    }

    #[test]
    fn a_replacement_record_supersedes_every_byte_the_previous_one_spanned() {
        let record = replacement_record(br#"{"pid":7}"#.to_vec(), 40);
        assert_eq!(record.len(), 40, "the write covers the whole span");
        assert!(record.starts_with(b"{\"pid\":7}\n"));
        assert!(
            record[10..].iter().all(|byte| *byte == b' '),
            "the tail is whitespace a reader can ignore"
        );
        let padded: serde_json::Value =
            serde_json::from_slice(&record).expect("padded record reads");
        assert_eq!(padded["pid"], 7);

        // A record longer than the span it replaces is written whole, and a
        // fresh file spans nothing at all; neither case may clip the record.
        assert_eq!(
            replacement_record(br#"{"pid":7}"#.to_vec(), 4),
            b"{\"pid\":7}\n"
        );
        assert_eq!(
            replacement_record(br#"{"pid":7}"#.to_vec(), 0),
            b"{\"pid\":7}\n"
        );
    }

    #[test]
    fn unreaped_zombie_process_and_group_are_terminal() {
        let mut command = Command::new("/usr/bin/true");
        command.process_group(0);
        let mut child = command.spawn().expect("zombie fixture launches");
        let pid = child.id();
        let mut information = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
        // SAFETY: waitid observes this child without reaping it because WNOWAIT is set.
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    information.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT,
                )
            },
            0
        );

        assert!(process_zombie(pid));
        assert!(process_terminal(pid));
        // The group's only remaining member is that zombie, and signal zero
        // still resolves the group while it goes unreaped, so a terminal answer
        // here cannot have come from the group having gone absent. It has to
        // come from enumerating the member and finding it terminal, which is
        // also what pins the PID reading of the platform enumeration: reading
        // its answer as a byte count would divide the single member away and
        // leave the group identity to answer for itself, which it refuses to do.
        assert!(!process_group_absent(pid));
        assert!(process_group_terminal(pid));
        assert!(child.wait().expect("zombie fixture reaps").success());
        assert!(process_group_absent(pid));
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
