//! The published discovery record: reading it safely and outlasting its replacement.
//!
//! A resident publishes one document naming the socket it listens on, and
//! holds one lock for as long as it runs. Reading the document is therefore
//! never enough on its own: the bytes of a dead resident are still perfectly
//! well-formed, so every read here is paired with the lock that proves a
//! publisher alive, and with the identities of the file and its parent so a
//! record replaced underneath the client is seen rather than believed.

use super::*;

pub(super) fn selected_discovery_path(
    operator_discovery: &Path,
    connection: Connection,
) -> PathBuf {
    match connection {
        Connection::Operator => operator_discovery.to_path_buf(),
        Connection::Lane(lane) => operator_discovery
            .with_file_name("canvas-lanes")
            .join(lane.to_string()),
    }
}

/// The lock the resident that published a discovery record holds while it runs.
///
/// This is the same sibling derivation [`selected_discovery_path`] makes:
/// `repository.rs` builds both `canvas` and `resident` on the runtime
/// directory, so the operator discovery path names the lock beside it. The
/// operator path is used for lane records too, because one resident publishes
/// every lane surface under `canvas-lanes` and holds exactly one lock for all
/// of them; walking up from a lane record instead would have to know how deep
/// it sits.
pub(super) fn resident_lock_path(operator_discovery: &Path) -> PathBuf {
    operator_discovery.with_file_name("resident")
}

pub(super) struct DiscoveryRead {
    path: PathBuf,
    parent: PathBuf,
    resident_lock: PathBuf,
    parent_identity: FileIdentity,
    file_identity: FileIdentity,
    pub(super) value: canvas::Discovery,
}

impl DiscoveryRead {
    pub(super) fn open(path: &Path, resident_lock: &Path) -> ClientResult<Self> {
        Self::open_fault(path, resident_lock).map_err(Outcome::Fault)
    }

    #[cfg(test)]
    pub(super) fn open_with(
        path: &Path,
        resident_lock: &Path,
        before_open: impl FnOnce(),
    ) -> ClientResult<Self> {
        Self::open_fault_with(path, resident_lock, before_open).map_err(Outcome::Fault)
    }

    pub(super) fn open_fault(path: &Path, resident_lock: &Path) -> Result<Self, Fault> {
        Self::open_fault_with(path, resident_lock, || {})
    }

    fn open_fault_with(
        path: &Path,
        resident_lock: &Path,
        before_open: impl FnOnce(),
    ) -> Result<Self, Fault> {
        let parent = path.parent().ok_or_else(|| {
            environment_fault_value(
                "canvas::discovery-unavailable",
                "canvas discovery has no repository runtime parent",
            )
        })?;
        let parent_metadata = secure_metadata(parent, DiscoveryKind::Runtime)?;
        let parent_identity = FileIdentity::from_metadata(&parent_metadata);
        let before = secure_metadata(path, DiscoveryKind::File)?;
        let file_identity = FileIdentity::from_metadata(&before);
        if file_identity.len > MAX_DISCOVERY_BYTES {
            return Err(protocol_fault_value(
                "canvas::discovery-malformed",
                "canvas discovery exceeds the current bounded document size",
            ));
        }
        before_open();
        let mut file = open_discovery(path)?;
        let opened = file.metadata().map_err(discovery_unavailable)?;
        require_secure(&opened, DiscoveryKind::File)?;
        if FileIdentity::from_metadata(&opened) != file_identity {
            return Err(discovery_replaced());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_DISCOVERY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(discovery_unavailable)?;
        if u64::try_from(bytes.len()).ok() != Some(file_identity.len) {
            return Err(discovery_replaced());
        }
        let value = wire::decode(&bytes).map_err(|_| {
            protocol_fault_value(
                "canvas::discovery-malformed",
                "canvas discovery does not match the current wire shape",
            )
        })?;
        let read = Self {
            path: path.to_path_buf(),
            parent: parent.to_path_buf(),
            resident_lock: resident_lock.to_path_buf(),
            parent_identity,
            file_identity,
            value,
        };
        read.verify_current_fault()?;
        Ok(read)
    }

    /// Refuse an endpoint whose publishing resident is no longer running.
    ///
    /// A discovery record is a claim about a process, not just a document, and
    /// nothing in its bytes expires. The kernel's `flock` on the resident lock
    /// is the one signal that does: it is released on any process death, so it
    /// answers for a resident that was killed exactly as it answers for one
    /// that exited through either orderly path. The lock's *contents* cannot
    /// stand in for it — they name a pid the operating system may already have
    /// reissued.
    ///
    /// This is the difference between refusing and dialling whatever now holds
    /// the port. The port is not the resident's to keep: a foreign server that
    /// binds it inherits a client that is about to present a capability token
    /// to a process with no concept of one.
    ///
    /// The probe never creates the lock file, so a runtime that has never had
    /// a resident reads as gone rather than as an error.
    ///
    /// A lock that cannot be probed at all is its own answer, and not the same
    /// one: the record may be perfectly good and its publisher perfectly alive.
    /// Naming it after the discovery file would send a reader to inspect bytes
    /// that are not the problem, so it keeps the lock's own reason. Either way
    /// the endpoint is refused — "unknown" is not permission to dial.
    ///
    /// Possession is necessary and not sufficient, so a held lock is handed to
    /// [`Self::require_publishing_holder`] rather than admitted here.
    pub(super) fn require_live_publisher(&self) -> Result<(), Fault> {
        let name = LockName::new("resident").expect("resident is a non-empty lock name");
        let availability = LockFile::new(name, &self.resident_lock)
            .availability()
            .map_err(|error| {
                environment_fault_value("canvas::publisher-unknown", &error.to_string())
            })?;
        match availability {
            Availability::Held => self.require_publishing_holder(),
            Availability::Available => Err(environment_fault_value(
                "canvas::publisher-gone",
                "canvas discovery outlived the resident that published it",
            )),
        }
    }

    /// Refuse a record the resident now holding the lock did not publish.
    ///
    /// A resident takes the lock on its first instant and only afterwards
    /// clears the records its predecessor left behind, so across that interval
    /// the kernel reports a live holder over a record it did not write and
    /// will never serve. Possession answers "is a resident running"; this
    /// answers the question the record actually poses, which is whether the
    /// resident running is the one that put this endpoint and capability on
    /// disk. The holder names its own generation inside the lock, and that is
    /// what makes the two comparable at all.
    ///
    /// This is a separate reason from a publisher that is gone. "Gone" sends a
    /// reader looking for a resident that died and left a record behind; here
    /// a resident is alive, starting, and about to publish its own. Reporting
    /// the second as the first would describe a healthy start as a corpse, and
    /// the remedies differ: nothing needs cleaning up, the caller needs to ask
    /// again once the record it will serve has landed.
    ///
    /// The lock is read after the kernel has answered rather than before, so a
    /// lock that cannot be read at all keeps the one reason that already
    /// covers it instead of earning a second.
    fn require_publishing_holder(&self) -> Result<(), Fault> {
        let holder = fs::read(&self.resident_lock).map_err(|error| {
            environment_fault_value("canvas::publisher-unknown", &error.to_string())
        })?;
        if spice::resident_lock::holder_epoch(&holder) == Some(self.value.epoch) {
            return Ok(());
        }
        Err(environment_fault_value(
            "canvas::publisher-replaced",
            "the resident holding the lock did not publish this canvas discovery",
        ))
    }

    pub(super) fn verify_current(&self) -> ClientResult<()> {
        self.verify_current_fault().map_err(Outcome::Fault)
    }

    pub(super) fn verify_current_fault(&self) -> Result<(), Fault> {
        let parent = secure_metadata(&self.parent, DiscoveryKind::Runtime)?;
        let file = secure_metadata(&self.path, DiscoveryKind::File)?;
        if !self
            .parent_identity
            .same_object(&FileIdentity::from_metadata(&parent))
            || FileIdentity::from_metadata(&file) != self.file_identity
        {
            return Err(discovery_replaced());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum DiscoveryKind {
    Runtime,
    File,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            len: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn same_object(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode && self.owner == other.owner
    }
}

#[cfg(not(unix))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[cfg(not(unix))]
impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }

    fn same_object(&self, other: &Self) -> bool {
        self == other
    }
}

pub(super) fn secure_metadata(path: &Path, kind: DiscoveryKind) -> Result<Metadata, Fault> {
    let metadata = fs::symlink_metadata(path).map_err(discovery_unavailable)?;
    require_secure(&metadata, kind)?;
    Ok(metadata)
}

#[cfg(unix)]
pub(super) fn require_secure(metadata: &Metadata, kind: DiscoveryKind) -> Result<(), Fault> {
    let shape_matches = match kind {
        DiscoveryKind::Runtime => metadata.file_type().is_dir(),
        DiscoveryKind::File => metadata.file_type().is_file(),
    };
    if !shape_matches {
        return Err(environment_fault_value(
            "canvas::discovery-insecure",
            "canvas discovery path is not a real repository-owned file",
        ));
    }
    let owner = rustix::process::geteuid().as_raw();
    if metadata.uid() != owner {
        return Err(environment_fault_value(
            "canvas::discovery-non-owner",
            "canvas discovery path is not owned by the effective user",
        ));
    }
    if matches!(kind, DiscoveryKind::File) && metadata.mode() & 0o077 != 0 {
        return Err(environment_fault_value(
            "canvas::discovery-insecure",
            "canvas discovery is readable or writable by group or other",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn require_secure(metadata: &Metadata, kind: DiscoveryKind) -> Result<(), Fault> {
    let shape_matches = match kind {
        DiscoveryKind::Runtime => metadata.file_type().is_dir(),
        DiscoveryKind::File => metadata.file_type().is_file(),
    };
    if !shape_matches {
        return Err(environment_fault_value(
            "canvas::discovery-insecure",
            "canvas discovery path is not a real repository-owned file",
        ));
    }
    Ok(())
}

pub(super) fn open_discovery(path: &Path) -> Result<File, Fault> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path).map_err(discovery_unavailable)
}

/// One direct wake on the runtime directory, re-armed before each read.
///
/// A kernel vnode watch replaces the FSEvents stream: arming a stream blocks
/// until the events daemon schedules it, seconds behind a busy build.
pub(super) struct DiscoveryObserver {
    runtime: PathBuf,
    changes: Receiver<io::Result<()>>,
}

impl DiscoveryObserver {
    pub(super) fn arm(resident_lock: &Path) -> Option<Self> {
        let runtime = resident_lock.parent()?.to_path_buf();
        let changes = Self::watch(&runtime)?;
        Some(Self { runtime, changes })
    }

    fn watch(runtime: &Path) -> Option<Receiver<io::Result<()>>> {
        let observer = crate::resident::observer::DirectObserver::arm(runtime).ok()?;
        let (changed, changes) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("spice-discovery-wait".to_owned())
            .spawn(move || {
                let _ = changed.send(observer.wait());
            })
            .ok()?;
        Some(changes)
    }

    pub(super) fn wait(&mut self, deadline: Instant) -> DiscoveryWait {
        let Ok(remaining) = remaining(deadline) else {
            return DiscoveryWait::Deadline;
        };
        match self.changes.recv_timeout(remaining) {
            Ok(Ok(())) => match Self::watch(&self.runtime) {
                Some(changes) => {
                    self.changes = changes;
                    DiscoveryWait::Changed
                }
                None => DiscoveryWait::Failed,
            },
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => DiscoveryWait::Failed,
            Err(RecvTimeoutError::Timeout) => DiscoveryWait::Deadline,
        }
    }
}

pub(super) enum DiscoveryWait {
    Changed,
    Deadline,
    Failed,
}
