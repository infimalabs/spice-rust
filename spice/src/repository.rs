//! Resolve the repository-common state namespace owned only by this runtime.
//!
//! Python Spice owns `.spice`; this runtime owns `.spice-rs`. The roots are
//! siblings beneath the Git common directory, so equal lane, task, thread, or
//! steering identifiers cannot make either implementation discover, consume,
//! acknowledge, or overwrite the other's state. The install appliance remains
//! in the already-disjoint `.spice-install` sibling.
//!
//! The same rule covers the only writable file outside those roots. Python owns
//! `<git-dir>/agent.gitconfig`; [`crate::git_shadow`] owns
//! `<git-dir>/spice-rs-agent.gitconfig`. Neither implementation relies on
//! byte-compatible whole-file writes by the other.
//!
//! The policy that shared root imposes:
//!
//! 1. No command in this port may import, convert, or rewrite the predecessor's
//!    durable state, and none may do so as an incidental effect of another verb.
//!    An in-place rewrite is still an in-place rewrite when it is called a
//!    migration.
//! 2. The only paths this runtime creates or writes under that root are the ones
//!    [`StatePaths::owned_directories`] and [`StatePaths::owned_files`] name,
//!    plus whatever sits inside a [`StatePaths::exclusive_directories`] entry --
//!    a lock is named after whoever asked for it, so no list could enumerate one
//!    in advance. Everything else under the root is another system's property
//!    and is left exactly as found, byte for byte.
//! 3. A cross-system bridge is refused by default. The sole translator that
//!    exists, `spice config migrate-tasks`, runs only when an operator names an
//!    export path on the command line, reads that one file, and writes nothing.
//!
//! `tests/coexistence.rs` enforces 2 and 3 by identity rather than by reading
//! this comment: it seeds predecessor-owned files under a fixture root, runs the
//! binary, and compares the bytes back.
//!
//! # Why the root is shared rather than split
//!
//! Taking a distinct Rust-owned root beside the predecessor's was considered and
//! rejected. The two systems are already disjoint inside the one root, so a
//! split would buy a separation that is there, at the cost of moving every path
//! below and stranding the state written under the old name:
//!
//! * The **journal** is separate by filename. This port writes
//!   `data/spicejournal.sqlite3` and `data/spicefolds.sqlite3`. The predecessor
//!   writes `spiceacks`, `spicemaxims`, `spiceprojections`, `spiceteams` and
//!   `taskchampion` beside them. Neither opens a database the other writes, so
//!   an operator running both at once has no journal to cross-write -- the
//!   guarantee holds per file, and does not depend on the two never running
//!   together.
//! * **Config** is separate by anchor. `config.toml` here hangs off the common
//!   root, while the predecessor's worktree config hangs off `<git-dir>/.spice`
//!   and the root `spice.toml` schema stays the predecessor's.
//! * **Hooks** were separate before the question was asked: the installer owns
//!   `.spice-install/`, which is not under this root at all.
//! * **Worktree Git-dir state** is not shared, because this port keeps none.
//!   Every `.spice` here hangs off the Git *common* directory --
//!   [`Repository::admit_state`], `gate::state::current_state_path` and
//!   `crate::config::load::locations` all anchor there -- so `<git-dir>/.spice`
//!   is the predecessor's per-worktree config root alone, and no path in this
//!   port constructs it.
//!
//! Under the shared root, `artifacts/tasks/<INCEPTION-STAMP>/` is the Rust
//! artifact path. Its directory key is the same sole identity stored by the
//! board; the current rendered handle is never a second filesystem authority.
//!
//! # The one file both write outside the shared root
//!
//! `<git-dir>/agent.gitconfig` is written by both systems, at the per-worktree
//! Git directory `rev-parse --absolute-git-dir` reports and not under any
//! `.spice`. [`crate::git_shadow`] publishes it here and the predecessor's
//! `spice/agent/shadow.py` publishes it there. Neither contributes an entry to
//! it: each writes the whole file, so the bytes the later run leaves are the
//! only bytes left.
//!
//! That is safe today only because the two generate identical bytes and both
//! read the file back before writing, so whichever runs second finds its own
//! content already there and writes nothing. It stays safe only while they stay
//! agreed on every part of it: the filename, the `--absolute-git-dir` anchor,
//! the header line, the `[include]` and `[branch]` section order, the tab
//! indentation and trailing newline, the subsection quoting -- backslash then
//! quote, in that order -- and the `git config --system --edit` probe under
//! `GIT_EDITOR=echo` that fills in the include path. Change one of those on one
//! side and the two overwrite each other on every alternating run, each leaving
//! a file the other replaces, with nothing anywhere to notice it.
//!
//! Policy point 2 and `tests/coexistence.rs` do not reach this file: both are
//! scoped to `<git-common-dir>/.spice`, and this path is under neither. What
//! holds it is `tests/git_shadow.rs`, which pins the anchor and filename against
//! a linked worktree, where the per-worktree Git directory and the common one
//! are different paths. It also pins the complete bytes for a simple branch --
//! header, section order, tab indentation and trailing newline -- plus the
//! reachable double-quote subsection escaping. Git refuses backslashes in a
//! refname, so that first escape replacement is recorded at the writer as a
//! defensive predecessor-compatible arm rather than covered by a forged branch.
//!
//! Release-one `.spice-rust` state remains retired and is refused explicitly.
//! Previous shared-root state under `.spice` is not admitted as current state:
//! reading it would recreate the routing ambiguity this namespace boundary is
//! meant to remove. `tests/coexistence.rs` seeds that complete predecessor root
//! and proves native commands leave it byte-identical.

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
    common: PathBuf,
}

/// Exact repository-common paths admitted for Rust-owned state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatePaths {
    runtime: PathBuf,
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
    fn new(runtime: PathBuf) -> Self {
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
            inbox: runtime.join("inbox"),
            learnings: runtime.join("learnings"),
            artifact_data,
            runtime,
            data,
        }
    }

    /// Sole repository-common runtime root.
    #[must_use]
    pub fn runtime(&self) -> &Path {
        &self.runtime
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

    /// Parent of per-lane steering inboxes.
    ///
    /// Python's inbox is beneath its `.spice` root; this one is beneath the
    /// Rust-only `.spice-rs` root.
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
    /// absent: refusing to start because a lane surface is missing would make a
    /// normal cold start an error.
    fn admitted_directories(&self) -> [&Path; 8] {
        [
            self.runtime(),
            self.data(),
            self.locks(),
            self.proof(),
            self.degraded(),
            self.tape(),
            &self.artifact_data,
            self.artifacts(),
        ]
    }

    /// Every named directory this runtime writes under its private state root.
    ///
    /// Ownership extends past what the runtime admits at startup: a name this
    /// port writes is its own whether or not a wrong shape there should abort a
    /// start. Deriving this list from the narrower admission list keeps the two
    /// registries aligned.
    #[must_use]
    pub fn owned_directories(&self) -> [&Path; 11] {
        let [
            runtime,
            data,
            locks,
            proof,
            degraded,
            tape,
            artifact_data,
            artifacts,
        ] = self.admitted_directories();
        [
            runtime,
            data,
            locks,
            proof,
            degraded,
            tape,
            artifact_data,
            artifacts,
            self.lane_canvas(),
            self.inbox(),
            self.learnings(),
        ]
    }

    /// Root whose entire contents belong to this runtime.
    ///
    /// A single exclusive root is the architectural boundary: new child names
    /// cannot accidentally become Python-owned merely because a registry below
    /// forgot to enumerate them.
    #[must_use]
    pub fn exclusive_directories(&self) -> [&Path; 1] {
        [self.runtime()]
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
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(directory(&marker, DirectoryKind::Git)?)?,
                    });
                }
                Ok(kind) if kind.is_file() => {
                    let git = gitdir_pointer(&marker)?;
                    return Ok(Self {
                        worktree: cursor,
                        common: common_directory(git)?,
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
        let paths = StatePaths::new(self.common.join(STATE_DIRECTORY));
        for path in paths.admitted_directories() {
            admit(path, StateShape::Directory)?;
        }
        for path in paths.admitted_files() {
            admit(path, StateShape::RegularFile)?;
        }
        Ok(paths)
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

/// Why Rust-owned repository state was not confined to its fixed namespace.
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
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn main_and_nested_repositories_resolve_the_nearest_common_state() {
        let scratch = Scratch::new();
        let outer = scratch.join("outer repository");
        let inner = outer.join("nested repository");
        fs::create_dir_all(outer.join(".git")).expect("create outer Git directory");
        fs::create_dir_all(inner.join(".git")).expect("create inner Git directory");
        let start = inner.join("source tree").join("deep");
        fs::create_dir_all(&start).expect("create nested start");

        let repository = Repository::discover(&start).expect("resolve nearest repository");
        let worktree = fs::canonicalize(&inner).expect("canonical worktree root");
        let common = fs::canonicalize(inner.join(".git")).expect("canonical common directory");

        assert_eq!(repository.worktree_root(), worktree);
        assert_eq!(repository.common_dir(), common);
        let state = repository
            .admit_state()
            .expect("admit absent current state");
        assert_eq!(state.runtime(), common.join(".spice-rs"));
        assert_eq!(state.data(), common.join(".spice-rs/data"));
        assert_eq!(
            state.journal(),
            common.join(".spice-rs/data/spicejournal.sqlite3")
        );
        assert_eq!(
            state.folds(),
            common.join(".spice-rs/data/spicefolds.sqlite3")
        );
        assert_eq!(state.locks(), common.join(".spice-rs/locks"));
        assert_eq!(state.proof(), common.join(".spice-rs/proof"));
        assert_eq!(state.resident(), common.join(".spice-rs/resident"));
        assert_eq!(state.canvas(), common.join(".spice-rs/canvas"));
        assert_eq!(state.degraded(), common.join(".spice-rs/degraded"));
        assert_eq!(state.tape(), common.join(".spice-rs/tape"));
        assert_eq!(
            proof::Layout::new(
                repository.worktree_root(),
                state.proof(),
                wire::manifest::ReleaseTarget::Aarch64AppleDarwin,
            )
            .payload_path(),
            worktree.join("dist/stage/targets/aarch64-apple-darwin/payload.json")
        );
    }

    #[test]
    fn linked_worktree_resolves_relative_gitdir_and_commondir_with_spaces() {
        let scratch = Scratch::new();
        let origin = scratch.join("origin repository");
        let common = origin.join(".git");
        let git = common.join("worktrees").join("lane one");
        let worktree = scratch.join("linked worktree");
        fs::create_dir_all(&git).expect("create linked Git directory");
        fs::create_dir_all(worktree.join("nested")).expect("create linked worktree");
        fs::write(
            worktree.join(".git"),
            b"gitdir: ../origin repository/.git/worktrees/lane one\n",
        )
        .expect("write Git directory pointer");
        fs::write(git.join("commondir"), b"../..\n").expect("write common pointer");

        let repository =
            Repository::discover(worktree.join("nested")).expect("resolve linked worktree");

        assert_eq!(
            repository.worktree_root(),
            fs::canonicalize(&worktree).expect("canonical linked worktree")
        );
        assert_eq!(
            repository.common_dir(),
            fs::canonicalize(&common).expect("canonical common directory")
        );
        let state = repository
            .admit_state()
            .expect("admit linked current state");
        assert_eq!(
            state.runtime(),
            fs::canonicalize(&common)
                .expect("canonical Rust state parent")
                .join(".spice-rs")
        );
        assert_eq!(
            proof::Layout::new(
                repository.worktree_root(),
                state.proof(),
                wire::manifest::ReleaseTarget::X86_64UnknownLinuxMusl,
            )
            .payload_path(),
            fs::canonicalize(&worktree)
                .expect("canonical linked worktree")
                .join("dist/stage/targets/x86_64-unknown-linux-musl/payload.json")
        );

        let absolute_git = origin.join("absolute git directory");
        let absolute_worktree = scratch.join("absolute linked worktree");
        fs::create_dir(&absolute_git).expect("create absolute Git directory");
        fs::create_dir(&absolute_worktree).expect("create absolute linked worktree");
        fs::write(
            absolute_worktree.join(".git"),
            format!("gitdir: {}\n", absolute_git.display()),
        )
        .expect("write absolute Git pointer");
        fs::write(
            absolute_git.join("commondir"),
            format!("{}\n", common.display()),
        )
        .expect("write absolute common pointer");

        assert_eq!(
            Repository::discover(absolute_worktree)
                .expect("resolve absolute pointers")
                .common_dir(),
            fs::canonicalize(&common).expect("canonical absolute common directory")
        );
    }

    #[test]
    fn retired_release_one_state_refuses_before_current_state_is_touched() {
        let scratch = Scratch::new();
        let root = case(&scratch, "retired release-one state");
        let common = root.join(".git");
        fs::create_dir(&common).expect("create common Git directory");
        let current = common.join(STATE_DIRECTORY);
        let retired = common.join(RETIRED_STATE_DIRECTORY);
        fs::create_dir_all(retired.join("proof/run/target"))
            .expect("create release-one proof tree");
        fs::write(
            retired.join("proof/run/target/receipt.json"),
            b"release-one proof bytes\n",
        )
        .expect("plant release-one proof");
        let repository = Repository::discover(&root).expect("discover repository");

        let error = repository
            .admit_state()
            .expect_err("retired release-one state must refuse");

        assert_eq!(error.release(), Some(RETIRED_STATE_RELEASE));
        assert_eq!(error.path(), retired);
        assert!(error.to_string().contains("archive or remove"));
        assert_eq!(
            fs::read(retired.join("proof/run/target/receipt.json")).unwrap(),
            b"release-one proof bytes\n"
        );
        assert!(!current.exists());
    }

    #[cfg(unix)]
    #[test]
    fn state_admission_refuses_every_link_at_its_exact_component() {
        use std::os::unix::fs::symlink;

        let scratch = Scratch::new();
        for (label, relative) in [
            ("runtime", Path::new("")),
            ("data", Path::new("data")),
            ("journal", Path::new("data/spicejournal.sqlite3")),
            ("folds", Path::new("data/spicefolds.sqlite3")),
            ("locks", Path::new("locks")),
            ("proof", Path::new("proof")),
            ("resident", Path::new("resident")),
            ("canvas", Path::new("canvas")),
            ("degraded", Path::new("degraded")),
            ("tape", Path::new("tape")),
        ] {
            let (repository, runtime) = state_case(&scratch, &format!("linked-{label}"));
            let outside = scratch.join(format!("outside-{label}"));
            fs::create_dir(&outside).expect("create outside link target");
            let linked = state_component(&runtime, relative);
            if let Some(parent) = linked.parent() {
                fs::create_dir_all(parent).expect("create linked component parent");
            }
            symlink(&outside, &linked).expect("plant linked state component");

            let error = repository
                .admit_state()
                .expect_err("linked state component must refuse");
            assert!(
                matches!(&error, StateError::Linked { path } if path == &linked),
                "{label} refused as {error:?}"
            );
            assert_eq!(error.path(), linked);
        }
    }

    #[test]
    fn state_admission_refuses_non_directory_parents_and_non_regular_leaves() {
        let scratch = Scratch::new();
        for (label, relative) in [
            ("runtime", Path::new("")),
            ("data", Path::new("data")),
            ("locks", Path::new("locks")),
            ("proof", Path::new("proof")),
            ("degraded", Path::new("degraded")),
            ("tape", Path::new("tape")),
        ] {
            let (repository, runtime) = state_case(&scratch, &format!("file-{label}"));
            let path = state_component(&runtime, relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create malformed state parent");
            }
            fs::write(&path, b"not a directory").expect("plant non-directory state parent");

            let error = repository
                .admit_state()
                .expect_err("non-directory state parent must refuse");
            assert!(
                matches!(&error, StateError::NotDirectory { path: observed } if observed == &path),
                "{label} refused as {error:?}"
            );
        }

        for (label, relative) in [
            ("journal", Path::new("data/spicejournal.sqlite3")),
            ("folds", Path::new("data/spicefolds.sqlite3")),
            ("resident", Path::new("resident")),
            ("canvas", Path::new("canvas")),
        ] {
            let (repository, runtime) = state_case(&scratch, &format!("directory-{label}"));
            let path = runtime.join(relative);
            fs::create_dir_all(&path).expect("plant non-regular state leaf");

            let error = repository
                .admit_state()
                .expect_err("non-regular state leaf must refuse");
            let exact_path = matches!(
                &error,
                StateError::NotRegularFile { path: observed } if observed == &path
            );
            assert!(exact_path, "{label} refused as {error:?}");
        }
    }

    #[test]
    fn every_repository_shape_failure_is_named_at_its_path() {
        let scratch = Scratch::new();

        let no_repository = scratch.join("no repository").join("nested");
        fs::create_dir_all(&no_repository).expect("create ordinary directory");
        assert!(matches!(
            Repository::discover(&no_repository),
            Err(ResolveError::NoRepository { start })
                if start == fs::canonicalize(&no_repository).expect("canonical start")
        ));

        let malformed = case(&scratch, "malformed gitdir");
        fs::write(malformed.join(".git"), b"gitdir: target\nsecond line\n")
            .expect("write malformed pointer");
        assert!(matches!(
            Repository::discover(&malformed),
            Err(ResolveError::MalformedGitdir { path }) if path == malformed.join(".git")
        ));

        let missing = case(&scratch, "missing gitdir");
        fs::write(missing.join(".git"), b"gitdir: absent\n").expect("write missing pointer");
        assert!(matches!(
            Repository::discover(&missing),
            Err(ResolveError::MissingGitdir { path }) if path == missing.join("absent")
        ));

        let not_directory = case(&scratch, "file gitdir");
        fs::write(not_directory.join("target"), b"ordinary file").expect("write Git target");
        fs::write(not_directory.join(".git"), b"gitdir: target\n").expect("write Git pointer");
        assert!(matches!(
            Repository::discover(&not_directory),
            Err(ResolveError::GitdirNotDirectory { path }) if path == not_directory.join("target")
        ));

        let malformed_common = case(&scratch, "malformed commondir");
        fs::create_dir(malformed_common.join(".git")).expect("create Git directory");
        fs::write(
            malformed_common.join(".git/commondir"),
            b"target\nsecond line\n",
        )
        .expect("write malformed common pointer");
        assert!(matches!(
            Repository::discover(&malformed_common),
            Err(ResolveError::MalformedCommondir { path })
                if path == malformed_common.join(".git/commondir")
        ));

        let missing_common = case(&scratch, "missing commondir");
        fs::create_dir(missing_common.join(".git")).expect("create Git directory");
        fs::write(missing_common.join(".git/commondir"), b"absent\n")
            .expect("write missing common pointer");
        assert!(matches!(
            Repository::discover(&missing_common),
            Err(ResolveError::MissingCommondir { path })
                if path == missing_common.join(".git/absent")
        ));

        let common_file = case(&scratch, "file commondir");
        fs::create_dir(common_file.join(".git")).expect("create Git directory");
        fs::write(common_file.join(".git/target"), b"ordinary file").expect("write common target");
        fs::write(common_file.join(".git/commondir"), b"target\n").expect("write common pointer");
        assert!(matches!(
            Repository::discover(&common_file),
            Err(ResolveError::CommondirNotDirectory { path })
                if path == common_file.join(".git/target")
        ));

        let absent_start = scratch.join("absent start");
        assert!(matches!(
            Repository::discover(&absent_start),
            Err(ResolveError::Filesystem { path, .. }) if path == absent_start
        ));

        let file_start = scratch.join("file start");
        fs::write(&file_start, b"ordinary file").expect("write non-directory start");
        assert!(matches!(
            Repository::discover(&file_start),
            Err(ResolveError::StartNotDirectory { path }) if path == file_start
        ));
    }

    fn case(scratch: &Scratch, name: &str) -> PathBuf {
        let path = scratch.join(name);
        fs::create_dir(&path).expect("create refusal case");
        path
    }

    fn state_case(scratch: &Scratch, name: &str) -> (Repository, PathBuf) {
        let root = case(scratch, name);
        let common = root.join(".git");
        fs::create_dir(&common).expect("create state-case Git directory");
        let repository = Repository::discover(&root).expect("discover state-case repository");
        (repository, common.join(STATE_DIRECTORY))
    }

    fn state_component(runtime: &Path, relative: &Path) -> PathBuf {
        if relative.as_os_str().is_empty() {
            runtime.to_path_buf()
        } else {
            runtime.join(relative)
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "spice-repository-resolver-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create repository scratch");
            Self(fs::canonicalize(path).expect("canonical repository scratch"))
        }

        fn join(&self, path: impl AsRef<Path>) -> PathBuf {
            self.0.join(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove repository scratch");
        }
    }
}
