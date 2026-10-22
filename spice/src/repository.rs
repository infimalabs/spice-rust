//! Resolve the repository state namespaces this runtime owns.
//!
//! Durable authority lives in [`STATE_DIRECTORY`] beneath the Git common
//! directory, and each linked worktree's own Git directory carries its
//! worktree-local state below a root of the same name. The install appliance
//! lives in the disjoint `.spice-install` sibling, and [`crate::git_shadow`]
//! publishes the per-worktree Git shadow as `<git-dir>/agent.gitconfig`.
//!
//! The layout imposes these rules:
//!
//! 1. Every runtime-state path this runtime creates or writes is below one of
//!    its exclusive state roots. [`StatePaths::owned_directories`] and
//!    [`StatePaths::owned_files`] inventory the fixed paths, while
//!    [`StatePaths::exclusive_directories`] makes every dynamically named child
//!    belong to this runtime.
//! 2. No verb reads, imports, converts, or rewrites state outside those roots,
//!    and none does so as an incidental effect of another verb. An in-place
//!    rewrite is still an in-place rewrite when it is called a migration.
//! 3. A bridge from foreign state is refused: no translator exists. `spice rtk
//!    gain` is a read-only inspector rather than a translator: an operator must
//!    name one agent UUID and only its current Git-common
//!    [`StatePaths::rtk_history`] database may be passed to RTK through the
//!    documented `RTK_DB_PATH` input. Worktree-local state is not inspected.
//!
//! # One root name
//!
//! The state root is [`STATE_DIRECTORY`] beneath the Git-common directory and
//! beneath each worktree's own Git directory, and no other name is
//! recognized. A directory by any other name is foreign state under rule 2:
//! nothing reads, moves, or deletes it.
//!
//! # Why durable authority is common
//!
//! One exclusive root makes ownership a path-containment invariant rather than
//! a convention every leaf filename must preserve. Within it, repository
//! authority belongs to the resident spanning linked worktrees.
//!
//! * The **journal and all other durable runtime state** live below the
//!   Git-common state root.
//! * **Config** is separate by name and anchor. This runtime reads tracked
//!   `spice.toml`, common-root `config.toml`, and each worktree's
//!   Git-directory `worktree.toml`, both below the state root.
//! * **Steering** has no worktree-content authority. Directives and their ACK or
//!   NAK dispositions are facts in the common-root journal. Hook media is
//!   projected beneath the Git-common state root and partitioned by lane,
//!   so an advertised path is stable and directly readable. A tracked
//!   `<worktree>/.spice` lookalike is ordinary repository content that
//!   neither authority surface opens.
//! * **Artifacts and dynamically named locks** stay below the common
//!   exclusive state root, which is their only parent.
//!   `artifacts/tasks/<BIRTH-STAMP>/` uses the board's sole task identity;
//!   the rendered handle never becomes a second filesystem authority.
//! * **Installed hooks, skills, and binaries** stay in the disjoint
//!   `.spice-install` appliance. The install receipt remains runtime state at
//!   the [`StatePaths::install_record`] path below the state root.
//! * The **per-worktree Git shadow** is disposable and reconstructible.
//!   [`crate::git_shadow`] writes `<git-dir>/agent.gitconfig`.
//!
//! State outside the current fixed namespaces is ignored. In particular,
//! obsolete layouts cannot affect current state admission or command startup.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Sole name of the state root below a Git directory, owned by this runtime.
pub use wire::STATE_DIRECTORY;

/// What every state-shape refusal expected to find.
#[must_use]
pub fn state_shape_expectation() -> String {
    format!("real fixed components beneath the canonical Git-common {STATE_DIRECTORY} directory")
}

/// One repository resolved to its worktree and shared Git directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    worktree: PathBuf,
    git: PathBuf,
    common: PathBuf,
}

/// Exact repository state paths admitted for this runtime's state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    runtime: PathBuf,
    worktree_runtime: PathBuf,
    worktree_locks: PathBuf,
    steering_delivery: PathBuf,
    steering_delivery_lock: PathBuf,
    steering_media: PathBuf,
    data: PathBuf,
    journal: PathBuf,
    gate_state: PathBuf,
    gate_state_lock: PathBuf,
    locks: PathBuf,
    proof: PathBuf,
    resident: PathBuf,
    resident_log: PathBuf,
    serve: PathBuf,
    degraded: PathBuf,
    tape: PathBuf,
    rtk: PathBuf,
    supervisors: PathBuf,
    artifact_data: PathBuf,
    artifacts: PathBuf,
    install_record: PathBuf,
    local_configuration: PathBuf,
    lane_serve: PathBuf,
}

/// Worktree-local files admitted for one steering hook delivery decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringDeliveryPaths {
    state: PathBuf,
    lock: PathBuf,
    projections: PathBuf,
}

impl SteeringDeliveryPaths {
    /// Durable exact-output suppression record.
    #[must_use]
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Cross-process possession serializing delivery decisions.
    #[must_use]
    pub fn lock(&self) -> &Path {
        &self.lock
    }

    /// Stable, lane-private media projections for agent hooks.
    #[must_use]
    pub fn projections(&self) -> &Path {
        &self.projections
    }
}

impl StatePaths {
    fn new(runtime: PathBuf, worktree_runtime: PathBuf) -> Self {
        let data = runtime.join("data");
        let artifact_data = runtime.join("artifacts");
        let worktree_locks = worktree_runtime.join("locks");
        Self {
            journal: data.join("spicejournal.sqlite3"),
            gate_state: data.join("gate-state.json"),
            gate_state_lock: data.join("gate-state.lock"),
            locks: runtime.join("locks"),
            proof: runtime.join("proof"),
            resident: runtime.join("resident"),
            resident_log: runtime.join("resident.log"),
            serve: runtime.join("serve"),
            degraded: runtime.join("degraded"),
            tape: runtime.join("tape"),
            rtk: runtime.join("rtk"),
            supervisors: runtime.join("supervisors"),
            artifacts: Self::task_artifacts_under(&runtime).0,
            install_record: runtime.join("install.json"),
            local_configuration: runtime.join("config.toml"),
            lane_serve: runtime.join("serve-lanes"),
            steering_delivery: worktree_runtime.join("steering-delivery.json"),
            steering_delivery_lock: worktree_locks.join("steering-delivery"),
            steering_media: runtime.join("steering-media"),
            artifact_data,
            runtime,
            worktree_runtime,
            worktree_locks,
            data,
        }
    }

    /// Task-addressed artifact bytes and the lock serializing their store,
    /// under the shared runtime root `runtime`.
    #[must_use]
    pub fn task_artifacts_under(runtime: &Path) -> (PathBuf, PathBuf) {
        (
            runtime.join("artifacts").join("tasks"),
            runtime.join("locks").join("task-artifacts.lock"),
        )
    }

    /// Sole repository-common runtime root.
    #[must_use]
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// State root beneath this worktree's own Git directory.
    #[must_use]
    pub fn worktree_runtime(&self) -> &Path {
        &self.worktree_runtime
    }

    /// Parent of locks that serialize only one linked worktree's activity.
    #[must_use]
    pub fn worktree_locks(&self) -> &Path {
        &self.worktree_locks
    }

    /// Durable exact-output suppression record for worktree hook delivery.
    #[must_use]
    pub fn steering_delivery(&self) -> &Path {
        &self.steering_delivery
    }

    /// Cross-process possession serializing worktree hook delivery decisions.
    #[must_use]
    pub fn steering_delivery_lock(&self) -> &Path {
        &self.steering_delivery_lock
    }

    /// Repository-stable root for lane-private steering media projections.
    #[must_use]
    pub fn steering_media(&self) -> &Path {
        &self.steering_media
    }

    /// Parent of the Rust authority database.
    #[must_use]
    pub fn data(&self) -> &Path {
        &self.data
    }

    /// Sole Rust authority database.
    #[must_use]
    pub fn journal(&self) -> &Path {
        &self.journal
    }

    /// Current Rust gate-state ledger shared by every linked worktree.
    #[must_use]
    pub fn gate_state(&self) -> &Path {
        &self.gate_state
    }

    /// Publication lock paired with the current Rust gate-state ledger.
    #[must_use]
    pub fn gate_state_lock(&self) -> &Path {
        &self.gate_state_lock
    }

    /// Parent of daemonless resource locks.
    #[must_use]
    pub fn locks(&self) -> &Path {
        &self.locks
    }

    /// Parent of detached proof receipts.
    #[must_use]
    pub fn proof(&self) -> &Path {
        &self.proof
    }

    /// Resident possession file.
    #[must_use]
    pub fn resident(&self) -> &Path {
        &self.resident
    }

    /// Owner-only diagnostics from the detached Rust resident.
    #[must_use]
    pub fn resident_log(&self) -> &Path {
        &self.resident_log
    }

    /// Owner-only app-server discovery file.
    #[must_use]
    pub fn serve(&self) -> &Path {
        &self.serve
    }

    /// Parent of degraded lane markers.
    #[must_use]
    pub fn degraded(&self) -> &Path {
        &self.degraded
    }

    /// Parent of lane tapes.
    #[must_use]
    pub fn tape(&self) -> &Path {
        &self.tape
    }

    /// Parent of per-lane RTK history databases.
    #[must_use]
    pub fn rtk(&self) -> &Path {
        &self.rtk
    }

    /// Task-addressed sidecar artifact store.
    #[must_use]
    pub fn artifacts(&self) -> &Path {
        &self.artifacts
    }

    /// Receipt `spice install --apply` leaves behind.
    ///
    /// `crate::install` spells this name a second time, as a path relative to
    /// the common directory, because a plan describes its operations before a
    /// [`StatePaths`] exists to resolve them against.
    #[must_use]
    pub fn install_record(&self) -> &Path {
        &self.install_record
    }

    /// Machine-local configuration this port reads and republishes.
    #[must_use]
    pub fn local_configuration(&self) -> &Path {
        &self.local_configuration
    }

    /// Parent of per-lane app-server discovery files.
    #[must_use]
    pub fn lane_serve(&self) -> &Path {
        &self.lane_serve
    }

    /// Directories whose shape this runtime admits before it does any work.
    ///
    /// Admission is narrower than ownership on purpose. A path belongs here
    /// only if a wrong shape at startup should stop the runtime, so directories
    /// the resident creates and clears as it runs are deliberately absent.
    fn admitted_directories(&self) -> Vec<&Path> {
        let mut paths = vec![
            self.runtime(),
            self.data(),
            self.locks(),
            self.proof(),
            self.degraded(),
            self.tape(),
            self.rtk(),
            self.supervisors(),
            &self.artifact_data,
            self.artifacts(),
        ];
        for path in [
            self.worktree_runtime(),
            self.worktree_locks(),
            self.steering_media(),
        ] {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// Every named directory this runtime writes under its private state roots.
    ///
    /// Ownership extends past what the runtime admits at startup: a name this
    /// port writes is its own whether or not a wrong shape there should abort a
    /// start. Deriving this list from the narrower admission list keeps the two
    /// registries aligned.
    #[must_use]
    pub fn owned_directories(&self) -> Vec<&Path> {
        let mut paths = self.admitted_directories();
        let lane_serve = self.lane_serve();
        if !paths.contains(&lane_serve) {
            paths.push(lane_serve);
        }
        paths
    }

    /// Root whose entire contents belong to this runtime.
    ///
    /// The common authority root and, for a linked worktree, its distinct
    /// worktree root are the architectural boundary. New child names cannot
    /// fall outside this runtime's ownership merely because a registry below
    /// forgot to enumerate them.
    #[must_use]
    pub fn exclusive_directories(&self) -> Vec<&Path> {
        let mut roots = vec![self.runtime()];
        if self.worktree_runtime() != self.runtime() {
            roots.push(self.worktree_runtime());
        }
        roots
    }

    /// Files whose shape this runtime admits before it does any work.
    fn admitted_files(&self) -> [&Path; 7] {
        [
            self.journal(),
            self.gate_state(),
            self.gate_state_lock(),
            self.resident(),
            self.resident_log(),
            self.serve(),
            self.steering_delivery(),
        ]
    }

    /// Every fixed regular file this runtime writes under its private root.
    ///
    /// The install receipt and the machine-local configuration belong here even
    /// though neither is authority state. Neither is admitted, because
    /// an operator repairing a bad `config.toml` needs the runtime to reach the
    /// refusal that names it rather than stop at a shape check.
    #[must_use]
    pub fn owned_files(&self) -> [&Path; 9] {
        let [
            journal,
            gate_state,
            gate_state_lock,
            resident,
            resident_log,
            serve,
            steering_delivery,
        ] = self.admitted_files();
        [
            journal,
            gate_state,
            gate_state_lock,
            resident,
            resident_log,
            serve,
            steering_delivery,
            self.install_record(),
            self.local_configuration(),
        ]
    }
}

impl Repository {
    /// Resolves the nearest repository containing `start`.
    ///
    /// The caller supplies the starting directory so this library acquires no
    /// ambient working directory. A normal worktree owns a `.git` directory;
    /// a linked worktree owns a `.git` pointer whose target may in turn carry
    /// Git's `commondir` file. Those are the only admitted shapes.
    ///
    /// Nothing here resolves code or data from the binary's own location:
    /// data is baked by `include_str!`, and the gate's own `current_exe` read
    /// digests the running binary instead of loading beside it. One installed
    /// binary is shared by every worktree on purpose, and a caller-supplied
    /// start is what keeps the worktree it acts on unambiguous.
    ///
    /// Unambiguous is not current. The start fixes which worktree is acted on;
    /// nothing fixes which build acts. [`crate::install::installed_binary`]
    /// resolves one file under the common directory that every linked
    /// worktree shares, and `INSTALL.md` requires it to hash equal to the
    /// installing lane's own `target/debug/spice`, so the code running here is
    /// whichever lane installed last. Exec-ing this lane's own
    /// `target/debug/spice` would fix the build to this checkout only as far
    /// as its last `cargo build`, trading a shared binary whose contents were
    /// digest-checked at install for a private one that silently lags its own
    /// source.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, ResolveError> {
        let requested = start.as_ref();
        let start = fs::canonicalize(requested)
            .map_err(|source| ResolveError::filesystem(requested, source))?;
        if !metadata(&start)?.is_dir() {
            return Err(ResolveError::StartNotDirectory { path: start });
        }

        let mut cursor = start.clone();
        loop {
            let marker = cursor.join(".git");
            match fs::metadata(&marker) {
                Ok(kind) if kind.is_dir() => {
                    let git = directory(&marker, DirectoryKind::Git)?;
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(git.clone())?,
                        git,
                    });
                }
                Ok(kind) if kind.is_file() => {
                    let git = gitdir_pointer(&marker)?;
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(git.clone())?,
                        git,
                    });
                }
                Ok(_) => return Err(ResolveError::MalformedGitdir { path: marker }),
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(ResolveError::filesystem(&marker, source)),
            }

            if !cursor.pop() {
                return Err(ResolveError::NoRepository { start });
            }
        }
    }

    /// Canonical root of the nearest worktree containing the requested start.
    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.worktree
    }

    /// Canonical Git directory belonging only to this worktree.
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        &self.git
    }

    /// Canonical Git directory shared by every linked worktree.
    #[must_use]
    pub fn common_dir(&self) -> &Path {
        &self.common
    }

    /// The Git-common state root: [`STATE_DIRECTORY`] beneath the Git-common
    /// directory.
    #[must_use]
    pub fn state_root(&self) -> PathBuf {
        wire::state_root(&self.common)
    }

    /// The state root beneath this worktree's own Git directory.
    #[must_use]
    pub fn worktree_state_root(&self) -> PathBuf {
        wire::state_root(&self.git)
    }

    fn state_paths(&self) -> StatePaths {
        StatePaths::new(self.state_root(), self.worktree_state_root())
    }

    /// Admit every fixed component of the current state namespace.
    ///
    /// An absent namespace is valid and remains absent. Existing directory
    /// parents and regular-file leaves are inspected without following links.
    /// Entries outside the current fixed namespace are not inspected.
    pub fn admit_state(&self) -> Result<StatePaths, StateError> {
        let paths = self.state_paths();
        for path in paths.admitted_directories() {
            admit(path, StateShape::Directory)?;
        }
        for path in paths.admitted_files() {
            admit(path, StateShape::RegularFile)?;
        }
        Ok(paths)
    }

    /// Admit only the state components needed to contact the current resident.
    ///
    /// The common runtime, resident possession, operator discovery, and
    /// per-lane discovery parent are independently shape-checked.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed or unavailable client-path component.
    pub fn admit_resident_client(&self) -> Result<PathBuf, StateError> {
        let paths = self.state_paths();
        for path in [paths.runtime(), paths.lane_serve()] {
            admit(path, StateShape::Directory)?;
        }
        for path in [paths.resident(), paths.serve()] {
            admit(path, StateShape::RegularFile)?;
        }
        Ok(paths.serve().to_path_buf())
    }

    /// Admit only the state components needed to suppress duplicate hook output.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed or unavailable delivery-state component.
    pub fn admit_steering_delivery(&self) -> Result<SteeringDeliveryPaths, StateError> {
        let paths = self.state_paths();
        for path in [
            paths.runtime(),
            paths.worktree_runtime(),
            paths.worktree_locks(),
        ] {
            admit(path, StateShape::Directory)?;
        }
        admit(paths.steering_media(), StateShape::Directory)?;
        admit(paths.steering_delivery(), StateShape::RegularFile)?;
        Ok(SteeringDeliveryPaths {
            state: paths.steering_delivery().to_path_buf(),
            lock: paths.steering_delivery_lock().to_path_buf(),
            projections: paths.steering_media().to_path_buf(),
        })
    }

    /// Admit the complete resident state surface.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed or unavailable authority component.
    pub fn admit_resident_state(&self) -> Result<StatePaths, StateError> {
        self.admit_state()
    }
}

#[derive(Clone, Copy)]
enum StateShape {
    Directory,
    RegularFile,
}

fn admit(path: &Path, expected: StateShape) -> Result<(), StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(StateError::filesystem(path, source)),
    };
    if metadata.file_type().is_symlink() {
        return Err(StateError::Linked {
            path: path.to_path_buf(),
        });
    }
    match expected {
        StateShape::Directory if !metadata.is_dir() => Err(StateError::NotDirectory {
            path: path.to_path_buf(),
        }),
        StateShape::RegularFile if !metadata.is_file() => Err(StateError::NotRegularFile {
            path: path.to_path_buf(),
        }),
        StateShape::Directory | StateShape::RegularFile => Ok(()),
    }
}

/// Failure to admit the fixed repository state namespaces.
#[derive(Debug)]
pub enum StateError {
    /// An existing state component is a symbolic link.
    Linked { path: PathBuf },
    /// An existing state parent is not a directory.
    NotDirectory { path: PathBuf },
    /// An existing state leaf is not a regular file.
    NotRegularFile { path: PathBuf },
    /// A component could not be inspected without following it.
    Filesystem { path: PathBuf, source: io::Error },
}

impl StateError {
    fn filesystem(path: &Path, source: io::Error) -> Self {
        Self::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    }

    /// Exact component whose admission failed.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Linked { path }
            | Self::NotDirectory { path }
            | Self::NotRegularFile { path }
            | Self::Filesystem { path, .. } => path,
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Linked { path } => {
                write!(
                    formatter,
                    "Rust state component is linked: {}",
                    path.display()
                )
            }
            Self::NotDirectory { path } => write!(
                formatter,
                "Rust state parent is not a directory: {}",
                path.display()
            ),
            Self::NotRegularFile { path } => write!(
                formatter,
                "Rust state leaf is not a regular file: {}",
                path.display()
            ),
            Self::Filesystem { path, source } => write!(
                formatter,
                "Rust state component could not be inspected at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for StateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::Linked { .. } | Self::NotDirectory { .. } | Self::NotRegularFile { .. } => None,
        }
    }
}

fn gitdir_pointer(marker: &Path) -> Result<PathBuf, ResolveError> {
    let line = one_line(marker, PointerKind::Gitdir)?;
    let Some(value) = line.strip_prefix("gitdir: ") else {
        return Err(ResolveError::MalformedGitdir {
            path: marker.to_path_buf(),
        });
    };
    if value.is_empty() {
        return Err(ResolveError::MalformedGitdir {
            path: marker.to_path_buf(),
        });
    }
    let target = resolve_relative(marker.parent().expect(".git has a parent"), value);
    directory(&target, DirectoryKind::Git)
}

fn common_directory(git: PathBuf) -> Result<PathBuf, ResolveError> {
    let pointer = git.join("commondir");
    match fs::metadata(&pointer) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(git),
        Err(source) => return Err(ResolveError::filesystem(&pointer, source)),
        Ok(kind) if !kind.is_file() => {
            return Err(ResolveError::MalformedCommondir { path: pointer });
        }
        Ok(_) => {}
    }

    let line = one_line(&pointer, PointerKind::Commondir)?;
    if line.is_empty() {
        return Err(ResolveError::MalformedCommondir { path: pointer });
    }
    directory(&resolve_relative(&git, &line), DirectoryKind::Common)
}

fn one_line(path: &Path, kind: PointerKind) -> Result<String, ResolveError> {
    let mut bytes = fs::read(path).map_err(|source| ResolveError::filesystem(path, source))?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() || bytes.contains(&b'\n') || bytes.contains(&b'\r') {
        return Err(kind.malformed(path));
    }
    String::from_utf8(bytes).map_err(|_| kind.malformed(path))
}

fn resolve_relative(parent: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        parent.join(path)
    }
}

fn directory(path: &Path, kind: DirectoryKind) -> Result<PathBuf, ResolveError> {
    match fs::metadata(path) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Err(kind.missing(path)),
        Err(source) => return Err(ResolveError::filesystem(path, source)),
        Ok(metadata) if !metadata.is_dir() => return Err(kind.not_directory(path)),
        Ok(_) => {}
    }
    fs::canonicalize(path).map_err(|source| ResolveError::filesystem(path, source))
}

fn metadata(path: &Path) -> Result<fs::Metadata, ResolveError> {
    fs::metadata(path).map_err(|source| ResolveError::filesystem(path, source))
}

#[derive(Clone, Copy)]
enum DirectoryKind {
    Git,
    Common,
}

impl DirectoryKind {
    fn missing(self, path: &Path) -> ResolveError {
        match self {
            Self::Git => ResolveError::MissingGitdir {
                path: path.to_path_buf(),
            },
            Self::Common => ResolveError::MissingCommondir {
                path: path.to_path_buf(),
            },
        }
    }

    fn not_directory(self, path: &Path) -> ResolveError {
        match self {
            Self::Git => ResolveError::GitdirNotDirectory {
                path: path.to_path_buf(),
            },
            Self::Common => ResolveError::CommondirNotDirectory {
                path: path.to_path_buf(),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum PointerKind {
    Gitdir,
    Commondir,
}

impl PointerKind {
    fn malformed(self, path: &Path) -> ResolveError {
        match self {
            Self::Gitdir => ResolveError::MalformedGitdir {
                path: path.to_path_buf(),
            },
            Self::Commondir => ResolveError::MalformedCommondir {
                path: path.to_path_buf(),
            },
        }
    }
}

/// Why repository-common state could not be resolved exactly.
#[derive(Debug)]
pub enum ResolveError {
    /// The supplied starting path exists but is not a directory.
    StartNotDirectory { path: PathBuf },
    /// No `.git` entry exists at the start or any ancestor.
    NoRepository { start: PathBuf },
    /// A `.git` file is not one exact `gitdir: PATH` line.
    MalformedGitdir { path: PathBuf },
    /// A `.git` pointer names a path that does not exist.
    MissingGitdir { path: PathBuf },
    /// A `.git` pointer names something other than a directory.
    GitdirNotDirectory { path: PathBuf },
    /// A `commondir` file is not one nonempty path line.
    MalformedCommondir { path: PathBuf },
    /// A `commondir` pointer names a path that does not exist.
    MissingCommondir { path: PathBuf },
    /// A `commondir` pointer names something other than a directory.
    CommondirNotDirectory { path: PathBuf },
    /// A filesystem operation failed before a repository fact could be read.
    Filesystem { path: PathBuf, source: io::Error },
}

impl ResolveError {
    /// The one reason every command reports when it cannot resolve the
    /// repository containing its working directory; the detail is this
    /// error's own wording.
    pub const REASON: &'static str = "repository::unresolved";

    fn filesystem(path: &Path, source: io::Error) -> Self {
        Self::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StartNotDirectory { path } => {
                write!(
                    formatter,
                    "repository start is not a directory: {}",
                    path.display()
                )
            }
            Self::NoRepository { start } => {
                write!(formatter, "no Git repository contains {}", start.display())
            }
            Self::MalformedGitdir { path } => write!(
                formatter,
                "Git directory pointer is malformed: {}",
                path.display()
            ),
            Self::MissingGitdir { path } => {
                write!(formatter, "Git directory is missing: {}", path.display())
            }
            Self::GitdirNotDirectory { path } => write!(
                formatter,
                "Git directory target is not a directory: {}",
                path.display()
            ),
            Self::MalformedCommondir { path } => write!(
                formatter,
                "Git common-directory pointer is malformed: {}",
                path.display()
            ),
            Self::MissingCommondir { path } => write!(
                formatter,
                "Git common directory is missing: {}",
                path.display()
            ),
            Self::CommondirNotDirectory { path } => write!(
                formatter,
                "Git common-directory target is not a directory: {}",
                path.display()
            ),
            Self::Filesystem { path, source } => write!(
                formatter,
                "repository filesystem access failed at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for ResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "repository_tests.rs"]
mod tests;

mod lanes;
