//! Resolve the repository state namespaces owned only by this runtime.
//!
//! Python Spice owns `.spice`; this runtime owns `.spice-rs`. Durable authority
//! is beneath the Git common directory, while each worktree's disposable inbox
//! rendering is beneath that worktree's own Git directory. Equal lane, task,
//! thread, or steering identifiers cannot make either implementation discover,
//! consume, acknowledge, or overwrite the other's state. The install appliance
//! remains in the already-disjoint `.spice-install` sibling.
//!
//! The same rule covers the per-worktree Git shadow. Python owns
//! `<git-dir>/agent.gitconfig`; [`crate::git_shadow`] owns
//! `<git-dir>/spice-rs-agent.gitconfig`. Neither implementation relies on
//! byte-compatible whole-file writes by the other.
//!
//! The ownership split imposes these rules:
//!
//! 1. No command in this port may import, convert, or rewrite the predecessor's
//!    durable state, and none may do so as an incidental effect of another verb.
//!    An in-place rewrite is still an in-place rewrite when it is called a
//!    migration.
//! 2. Every runtime-state path this port creates or writes is below one of its
//!    exclusive `.spice-rs` roots. [`StatePaths::owned_directories`] and
//!    [`StatePaths::owned_files`] inventory the fixed paths, while
//!    [`StatePaths::exclusive_directories`] makes every dynamically named child
//!    Rust-owned without extending ownership into a Python `.spice` root.
//! 3. The per-worktree Git shadow follows the same separation rule: this runtime
//!    publishes only `spice-rs-agent.gitconfig` and never Python's
//!    `agent.gitconfig`.
//! 4. A cross-system bridge is refused by default. The sole translator that
//!    exists, `spice config migrate-tasks`, runs only when an operator names an
//!    export path on the command line, reads that one file, and writes nothing.
//!
//! `tests/coexistence.rs` enforces the filesystem ownership boundary by identity:
//! it seeds Python's complete `.spice` root, runs the real Rust binary against
//! `.spice-rs`, and compares the predecessor bytes back. `tests/git_shadow.rs`
//! independently pins the Rust filename and denies publication at Python's
//! shadow path.
//!
//! # Why ownership is split but durable authority is common
//!
//! Separate Python and Rust roots make ownership a path-containment invariant
//! rather than a convention every leaf filename must preserve. Within the Rust
//! namespace, repository authority belongs to the resident spanning linked
//! worktrees. The disposable inbox is deliberately worktree-local so one agent
//! shell cannot inherit another worktree's rendering.
//!
//! * The **journal and all other durable runtime state** live below
//!   `<git-common-dir>/.spice-rs`. Python's databases live below its `.spice`
//!   sibling, so equal database, lane, task, or steering names still cannot
//!   resolve to the same path.
//! * **Config** is separate by name and anchor. This runtime reads tracked
//!   `spice-rust.toml` plus common-root `.spice-rs/config.toml`; Python owns
//!   tracked `spice.toml` and its own `.spice` configuration.
//! * **Steering** has no worktree-content authority. Directives and their ACK or
//!   NACK dispositions are facts in the common-root journal; `inbox/` is a
//!   disposable rendering beneath each worktree's Git-dir `.spice-rs` root. A
//!   tracked `<worktree>/.spice-rs` lookalike is ordinary repository content
//!   that neither authority surface opens.
//! * **Artifacts, learnings, and dynamically named locks** remain Rust-only
//!   because their parents are below the common exclusive `.spice-rs` root.
//!   `artifacts/tasks/<INCEPTION-STAMP>/` uses the board's sole task identity;
//!   the rendered handle never becomes a second filesystem authority.
//! * **Installed hooks, skills, and binaries** stay in the already-disjoint
//!   `.spice-install` appliance. The install receipt remains runtime state at
//!   the [`StatePaths::install_record`] path below `.spice-rs`.
//! * **Per-worktree process surfaces** are disposable or reconstructible: the
//!   Rust inbox and Git shadow. The shadows share an anchor, not a writable file:
//!   [`crate::git_shadow`] writes `<git-dir>/spice-rs-agent.gitconfig`; Python
//!   writes `<git-dir>/agent.gitconfig`. `tests/git_shadow.rs` uses a linked
//!   worktree to pin the per-worktree anchor, the Rust shadow bytes, and the
//!   absence of publication at Python's filename.
//!
//! Release-one `.spice-rust` state remains retired and is refused explicitly.
//! Rust data left beneath Python-owned `.spice` by an earlier layout is not
//! admitted as current state: reading it would recreate the routing ambiguity
//! this namespace boundary removes. `tests/coexistence.rs` proves native
//! commands instead leave the complete predecessor root byte-identical.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Sole repository-common directory owned by the current Spice runtime.
pub const STATE_DIRECTORY: &str = ".spice-rs";

const RETIRED_STATE_DIRECTORY: &str = ".spice-rust";
const RETIRED_STATE_RELEASE: &str = "release-one";

/// One repository resolved to its worktree and shared Git directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Repository {
    worktree: PathBuf,
    git: PathBuf,
    common: PathBuf,
}

/// Exact repository state paths admitted for Rust-owned state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    runtime: PathBuf,
    worktree_runtime: PathBuf,
    data: PathBuf,
    journal: PathBuf,
    folds: PathBuf,
    gate_state: PathBuf,
    gate_state_lock: PathBuf,
    locks: PathBuf,
    proof: PathBuf,
    resident: PathBuf,
    canvas: PathBuf,
    degraded: PathBuf,
    tape: PathBuf,
    artifact_data: PathBuf,
    artifacts: PathBuf,
    install_record: PathBuf,
    local_configuration: PathBuf,
    lane_canvas: PathBuf,
    inbox: PathBuf,
    learnings: PathBuf,
}

impl StatePaths {
    fn new(runtime: PathBuf, worktree_runtime: PathBuf) -> Self {
        let data = runtime.join("data");
        let artifact_data = runtime.join("artifacts");
        Self {
            journal: data.join("spicejournal.sqlite3"),
            folds: data.join("spicefolds.sqlite3"),
            gate_state: data.join("gate-state.json"),
            gate_state_lock: data.join("gate-state.lock"),
            locks: runtime.join("locks"),
            proof: runtime.join("proof"),
            resident: runtime.join("resident"),
            canvas: runtime.join("canvas"),
            degraded: runtime.join("degraded"),
            tape: runtime.join("tape"),
            artifacts: artifact_data.join("tasks"),
            install_record: runtime.join("install.json"),
            local_configuration: runtime.join("config.toml"),
            lane_canvas: runtime.join("canvas-lanes"),
            inbox: worktree_runtime.join("inbox"),
            learnings: runtime.join("learnings"),
            artifact_data,
            runtime,
            worktree_runtime,
            data,
        }
    }

    /// Sole repository-common runtime root.
    #[must_use]
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// Rust-only state root beneath this worktree's own Git directory.
    #[must_use]
    pub fn worktree_runtime(&self) -> &Path {
        &self.worktree_runtime
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

    /// Disposable fold materialization database.
    #[must_use]
    pub fn folds(&self) -> &Path {
        &self.folds
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

    /// Owner-only canvas discovery file.
    #[must_use]
    pub fn canvas(&self) -> &Path {
        &self.canvas
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

    /// Task-addressed sidecar artifact store.
    #[must_use]
    pub fn artifacts(&self) -> &Path {
        &self.artifacts
    }

    /// Receipt `spice install --apply` leaves behind.
    ///
    /// `crate::install` spells this name a second time, as a path relative to
    /// the common directory, because a plan describes its operations before a
    /// [`StatePaths`] exists to resolve them against. `tests/coexistence.rs`
    /// closes that gap by identity rather than by comparing the two literals:
    /// it applies a real plan and requires the receipt to land here.
    #[must_use]
    pub fn install_record(&self) -> &Path {
        &self.install_record
    }

    /// Machine-local configuration this port reads and republishes.
    #[must_use]
    pub fn local_configuration(&self) -> &Path {
        &self.local_configuration
    }

    /// Parent of per-lane canvas discovery files.
    #[must_use]
    pub fn lane_canvas(&self) -> &Path {
        &self.lane_canvas
    }

    /// Parent of steering inboxes for lanes bound to this worktree.
    ///
    /// Python's inbox is beneath its `.spice` root; this one is beneath the
    /// Rust-only `.spice-rs` root in the worktree's own Git directory rather
    /// than the Git-common authority root.
    #[must_use]
    pub fn inbox(&self) -> &Path {
        &self.inbox
    }

    /// Rust-owned bounded operational learnings, partitioned by project stem.
    ///
    /// The predecessor's similarly named store is worktree-visible at
    /// `<worktree>/.spice/learnings`; this directory is below the Git-common
    /// state root. The equal leaf name therefore does not create shared files.
    #[must_use]
    pub fn learnings(&self) -> &Path {
        &self.learnings
    }

    /// Directories whose shape this runtime admits before it does any work.
    ///
    /// Admission is narrower than ownership on purpose. A path belongs here
    /// only if a wrong shape at startup should stop the runtime, so the
    /// directories the resident creates and clears as it runs are deliberately
    /// absent. The worktree state root and inbox are exceptions because a link
    /// or wrong-shaped parent there would redirect repository-delivered prose.
    fn admitted_directories(&self) -> Vec<&Path> {
        let mut paths = vec![
            self.runtime(),
            self.data(),
            self.locks(),
            self.proof(),
            self.degraded(),
            self.tape(),
            &self.artifact_data,
            self.artifacts(),
        ];
        for path in [self.worktree_runtime(), self.inbox()] {
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
        for path in [self.lane_canvas(), self.learnings()] {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// Root whose entire contents belong to this runtime.
    ///
    /// The common authority root and, for a linked worktree, its distinct
    /// worktree root are the architectural boundary. New child names cannot
    /// accidentally become Python-owned merely because a registry below forgot
    /// to enumerate them.
    #[must_use]
    pub fn exclusive_directories(&self) -> Vec<&Path> {
        let mut roots = vec![self.runtime()];
        if self.worktree_runtime() != self.runtime() {
            roots.push(self.worktree_runtime());
        }
        roots
    }

    /// Files whose shape this runtime admits before it does any work.
    fn admitted_files(&self) -> [&Path; 6] {
        [
            self.journal(),
            self.folds(),
            self.gate_state(),
            self.gate_state_lock(),
            self.resident(),
            self.canvas(),
        ]
    }

    /// Every fixed regular file this runtime writes under its private root.
    ///
    /// The install receipt and the machine-local configuration belong here even
    /// though neither is authority state. Neither is admitted, because
    /// an operator repairing a bad `config.toml` needs the runtime to reach the
    /// refusal that names it rather than stop at a shape check.
    #[must_use]
    pub fn owned_files(&self) -> [&Path; 8] {
        let [
            journal,
            folds,
            gate_state,
            gate_state_lock,
            resident,
            canvas,
        ] = self.admitted_files();
        [
            journal,
            folds,
            gate_state,
            gate_state_lock,
            resident,
            canvas,
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
    /// This is also why the port owes no equivalent of the predecessor's
    /// worktree re-exec. Its `_reexec_dev_command_for_worktree_checkout`
    /// re-runs a `dev` command under the worktree's own `.venv`, so an
    /// installed console script cannot import one checkout's package while
    /// acting on another's files. Nothing here resolves code or data from
    /// the binary's own location: data is baked by `include_str!`, and the
    /// gate's own `current_exe` read digests the running binary instead of
    /// loading beside it. One installed binary is shared by every worktree
    /// on purpose, and a caller-supplied start is what keeps the worktree
    /// it acts on unambiguous.
    ///
    /// Unambiguous is not current, and only the first of those is settled
    /// here. The start fixes which worktree is acted on; nothing fixes which
    /// build acts. [`crate::install::installed_binary`] resolves one file
    /// under the common directory that every linked worktree shares, and
    /// `INSTALL.md` requires it to hash equal to the installing lane's own
    /// `target/debug/spice`, so the code running here is whichever lane
    /// installed last. The predecessor bounds that same mismatch to one
    /// subcommand family, re-execing only when `argv[:1] == ["dev"]`; one
    /// shared binary is bounded to nothing and serves every verb.
    ///
    /// Its remedy still does not transfer. Exec-ing this lane's own
    /// `target/debug/spice` would fix the build to this checkout, but only
    /// as far as its last `cargo build`, trading a shared binary whose
    /// contents were digest-checked at install for a private one that
    /// silently lags its own source. That is not the correction to reach for.
    ///
    /// The hazard above is the state after the first install, not today's.
    /// Today the worktrees on this machine share one common directory and it
    /// holds no `.spice-install` at all, so no lane has installed, and
    /// `core.hooksPath` still names the predecessor's scripts. Nothing
    /// diverges yet; the divergence begins at the first install.
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

    /// Admit every fixed component of the current state namespace.
    ///
    /// An absent namespace is valid and remains absent. Existing directory
    /// parents and regular-file leaves are inspected without following links.
    /// Any release-one `.spice-rust` entry refuses before these current paths
    /// are returned; operators archive or remove that retired tree manually.
    pub fn admit_state(&self) -> Result<StatePaths, StateError> {
        refuse_retired_state(&self.common)?;
        let paths = StatePaths::new(
            self.common.join(STATE_DIRECTORY),
            self.git.join(STATE_DIRECTORY),
        );
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
    /// Steering is the repair surface for a malformed disposable inbox, so it
    /// cannot require that inbox to be well-shaped before it reaches the
    /// resident. The common runtime, resident possession, operator discovery,
    /// and per-lane discovery parent remain independently shape-checked.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed or unavailable client-path component.
    pub fn admit_resident_client(&self) -> Result<PathBuf, StateError> {
        refuse_retired_state(&self.common)?;
        let paths = StatePaths::new(
            self.common.join(STATE_DIRECTORY),
            self.git.join(STATE_DIRECTORY),
        );
        for path in [paths.runtime(), paths.lane_canvas()] {
            admit(path, StateShape::Directory)?;
        }
        for path in [paths.resident(), paths.canvas()] {
            admit(path, StateShape::RegularFile)?;
        }
        Ok(paths.canvas().to_path_buf())
    }

    /// Admit resident startup state while retaining a blocked inbox for repair.
    ///
    /// Every authority component keeps the full state-shape contract. The
    /// disposable inbox alone may be a real non-directory entry so startup can
    /// replay its parked delivery and serve `steer repair`; a symbolic link is
    /// still forbidden because it could redirect prose outside the worktree.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed or unavailable authority component, or a
    /// symbolic-link inbox.
    pub fn admit_resident_state(&self) -> Result<StatePaths, StateError> {
        refuse_retired_state(&self.common)?;
        let paths = StatePaths::new(
            self.common.join(STATE_DIRECTORY),
            self.git.join(STATE_DIRECTORY),
        );
        for path in paths
            .admitted_directories()
            .into_iter()
            .filter(|path| *path != paths.inbox())
        {
            admit(path, StateShape::Directory)?;
        }
        admit_unlinked(paths.inbox())?;
        for path in paths.admitted_files() {
            admit(path, StateShape::RegularFile)?;
        }
        Ok(paths)
    }

    /// Admit the worktree-local inbox delivery path without requiring it to
    /// already be a directory.
    ///
    /// A real file at this path is a delivery failure that steering records as
    /// parked and `steer repair` can remove. A linked path could redirect
    /// operator prose outside repository authority and therefore still
    /// refuses before publication.
    ///
    /// # Errors
    ///
    /// Returns an unavailable or malformed state-root error, or refuses a
    /// symbolic-link inbox.
    pub fn admit_inbox_delivery(&self) -> Result<PathBuf, StateError> {
        refuse_retired_state(&self.common)?;
        let paths = StatePaths::new(
            self.common.join(STATE_DIRECTORY),
            self.git.join(STATE_DIRECTORY),
        );
        admit(paths.worktree_runtime(), StateShape::Directory)?;
        let inbox = paths.inbox();
        admit_unlinked(inbox)?;
        Ok(inbox.to_path_buf())
    }
}

fn refuse_retired_state(common: &Path) -> Result<(), StateError> {
    let path = common.join(RETIRED_STATE_DIRECTORY);
    match fs::symlink_metadata(&path) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StateError::filesystem(&path, source)),
        Ok(_) => Err(StateError::RetiredState {
            path,
            release: RETIRED_STATE_RELEASE,
        }),
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

fn admit_unlinked(path: &Path) -> Result<(), StateError> {
    match fs::symlink_metadata(path) {
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StateError::filesystem(path, source)),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StateError::Linked {
            path: path.to_path_buf(),
        }),
        Ok(_) => Ok(()),
    }
}

/// Failure to admit the fixed Rust-owned repository namespaces.
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
    /// A retired state root remains and must be handled by the operator.
    RetiredState {
        path: PathBuf,
        release: &'static str,
    },
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
            | Self::Filesystem { path, .. }
            | Self::RetiredState { path, .. } => path,
        }
    }

    /// Release owning a refused predecessor spelling, when applicable.
    #[must_use]
    pub const fn release(&self) -> Option<&'static str> {
        match self {
            Self::RetiredState { release, .. } => Some(release),
            _ => None,
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
            Self::RetiredState { path, release } => write!(
                formatter,
                concat!(
                    "retired state root belongs to release={} at {}; archive or remove it ",
                    "after preserving evidence before starting current Spice"
                ),
                release,
                path.display()
            ),
        }
    }
}

impl Error for StateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::Linked { .. }
            | Self::NotDirectory { .. }
            | Self::NotRegularFile { .. }
            | Self::RetiredState { .. } => None,
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
