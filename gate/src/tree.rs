//! Immutable Git tree selection for gate runs.
//!
//! A gate never measures the checkout. The bare selector captures one parsed
//! index, while a revision selector resolves one immutable commit identity.
//! Both sources pass through the same path, mode, object, and materialization
//! checks before a private read-only root is returned to the caller, named
//! together with the repository it was taken from so that an arm which must
//! observe that repository cannot be pointed at a different one.
//!
//! Each materialization is a real directory of real bytes below the temporary
//! directory, and a run that unwinds removes its own. A run that is *killed*
//! unwinds nothing, so every workspace is held under the advisory lock the
//! crate's `held_root` module supplies, for exactly as long as it is live. That
//! lock is what lets a later run tell a killed run's wreckage from a tree a live
//! gate is measuring, and reclaim only the first.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sill::{Acquisition, LockFile, Possession};

#[cfg(target_family = "unix")]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

mod error;
mod source;
#[cfg(test)]
mod tests;

use crate::held_root::{self, HeldRootRemoval};
pub use error::TreeError;
use source::{IndexSeal, RevisionSeal};

/// Which immutable Git source a gate should measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// The repository index as captured at selection time.
    Staged,
    /// The one commit named by this revision at selection time.
    Revision(String),
}

impl Selector {
    /// Select the staged index.
    #[must_use]
    pub const fn staged() -> Self {
        Self::Staged
    }

    /// Select one named revision.
    #[must_use]
    pub fn revision(revision: impl Into<String>) -> Self {
        Self::Revision(revision.into())
    }

    /// Materialize this source below a private immutable root.
    ///
    /// The returned root owns no checkout state and remains valid until the
    /// [`SelectedTree`] is dropped. Index and reference sources are rechecked
    /// after every selected object has been copied, so a moving source cannot
    /// publish a partial or mixed tree.
    ///
    /// # Errors
    ///
    /// Returns the named repository, revision, index, path, mode, object, or
    /// filesystem failure that prevented an immutable tree from being formed.
    pub fn materialize(&self, repository: &Path) -> Result<SelectedTree, TreeError> {
        self.materialize_with(repository, |_| {})
    }

    fn materialize_with(
        &self,
        repository: &Path,
        after_capture: impl FnOnce(&Path),
    ) -> Result<SelectedTree, TreeError> {
        let repo = open_repository(repository)?;
        let mut build = BuildRoot::create()?;

        let (items, source) = match self {
            Self::Staged => {
                let before = IndexSeal::capture(&repo)?;
                after_capture(&build.workspace);
                let index = before.open(&repo, &build.workspace)?;
                let items = index_items(&index)?;
                materialize_items(&repo, &build.root, &items)?;
                (items, SelectionSeal::Index(before))
            }
            Self::Revision(revision) => {
                let before = RevisionSeal::capture(&repo, revision)?;
                after_capture(&build.workspace);
                let commit_id = before.resolve(&repo, revision, &build.workspace)?;
                let items = revision_items(&repo, revision, commit_id)?;
                materialize_items(&repo, &build.root, &items)?;
                (
                    items,
                    SelectionSeal::Revision {
                        before,
                        revision: revision.clone(),
                    },
                )
            }
        };

        make_immutable(&build.root)?;
        verify_immutable(&build.root, &items)?;
        source.verify(&repo)?;
        Ok(build.finish(repository))
    }
}

/// One private materialization owned for the duration of a gate run.
///
/// The possession is kept in an [`Option`] so [`Drop`] can take it: the removal
/// this tree owes runs while its lock is still held, which is the invariant
/// `held_root::remove_held_root` carries in its signature.
#[derive(Debug)]
pub struct SelectedTree {
    workspace: PathBuf,
    root: PathBuf,
    repository: PathBuf,
    possession: Option<Possession>,
}

impl SelectedTree {
    /// Root beneath which every selected path is normalized and relative.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Repository this selection was materialized from.
    ///
    /// Carried with the tree because the root alone cannot name it: the root is
    /// a private copy in a scratch directory, and anything that must observe the
    /// live repository the identity was taken from has no other way to reach the
    /// right one. A caller that supplied it separately could supply a different
    /// one, and the observation would be labelled with a tree it never saw.
    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }
}

impl Drop for SelectedTree {
    fn drop(&mut self) {
        if let Some(possession) = self.possession.take() {
            let _ = release_workspace(&self.workspace, &possession);
        }
    }
}

/// How gate tree selection names itself in a refusal, and names its locks.
const WORKSPACE_LABEL: &str = "gate tree workspace";
const WORKSPACE_LOCK: &str = "gate-tree-workspace";

/// The prefix every gate tree workspace name begins with.
const WORKSPACE_PREFIX: &str = "spice-gate-tree-";

#[derive(Debug)]
struct BuildRoot {
    workspace: PathBuf,
    root: PathBuf,
    possession: Option<Possession>,
}

impl BuildRoot {
    fn create() -> Result<Self, TreeError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = std::env::temp_dir();
        reclaim_abandoned(&temporary);
        for _ in 0..128 {
            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            let workspace =
                temporary.join(format!("{WORKSPACE_PREFIX}{}-{nonce}", std::process::id()));
            let Some(possession) = possess_workspace(&workspace) else {
                continue;
            };
            match create_private_workspace(&workspace) {
                Ok(()) => {
                    let root = workspace.join("root");
                    let build = Self {
                        workspace,
                        root,
                        possession: Some(possession),
                    };
                    private_directory(&build.workspace)?;
                    fs::create_dir(&build.root).map_err(|source| {
                        TreeError::io("creating a selected tree root", &build.root, source)
                    })?;
                    private_directory(&build.root)?;
                    return Ok(build);
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(TreeError::io(
                        "creating a selected tree workspace",
                        &workspace,
                        source,
                    ));
                }
            }
        }
        Err(TreeError::io(
            "creating a selected tree workspace",
            &temporary,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "selected tree name attempts exhausted",
            ),
        ))
    }

    /// Hand the workspace, and the lock that guards it, to the selected tree.
    ///
    /// Taking the possession is what stops this build root's own [`Drop`] from
    /// removing a workspace the caller is about to be handed: a build root that
    /// has been finished no longer possesses anything to remove.
    fn finish(&mut self, repository: &Path) -> SelectedTree {
        SelectedTree {
            workspace: self.workspace.clone(),
            root: self.root.clone(),
            repository: repository.to_path_buf(),
            possession: self.possession.take(),
        }
    }
}

/// Take one workspace name's lock, and clear anything a dead run left under it.
///
/// `None` means "mint another name": either a live run holds this one, or what
/// it left behind could not be removed. Both are answered by moving to the next
/// nonce, which is what this loop did for a colliding name before any lock
/// existed.
fn possess_workspace(workspace: &Path) -> Option<Possession> {
    let lock = workspace_lock(workspace).ok()?;
    let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() else {
        return None;
    };
    match reclaim_workspace(workspace, &possession) {
        HeldRootRemoval::Removed => Some(possession),
        HeldRootRemoval::Failed(_) => None,
    }
}

/// The lock guarding one gate tree workspace, as every reclaim resolves it.
fn workspace_lock(workspace: &Path) -> Result<LockFile, String> {
    held_root::held_root_lock(workspace, WORKSPACE_LOCK, WORKSPACE_LABEL)
}

/// Remove every gate tree workspace no live run holds.
///
/// A workspace name carries the identifier of the process that minted it, so no
/// other *live* run ever resolves the same one, and a run that unwinds removes
/// its own. A dead run's identifier is eventually reissued, which is why
/// [`possess_workspace`] reclaims a minted name before building under it rather
/// than trusting that a fresh nonce means a fresh directory.
///
/// A run that is killed unwinds nothing, and what it leaves is a directory
/// indistinguishable, by looking at it, from one a live gate is measuring out of
/// right now. The lock beside each workspace is what separates them: a lock no
/// one holds is a workspace whose owner is dead.
///
/// Only a workspace with a lock file already beside it is removed, and that
/// restriction is load bearing rather than an optimization. A gate built before
/// this mechanism minted workspaces under the same prefix and took no lock at
/// all, so an unlocked directory is not evidence of a dead owner; on a machine
/// where several checkouts share one temporary directory it is most likely a
/// live run of an older build, and removing it would delete the tree that run
/// is measuring. Those are left for an operator, and every workspace this build
/// mints is reclaimable because it is locked before it exists.
///
/// A lock whose workspace is absent is considered separately. That shape can
/// be residue from a run killed after removing its workspace, but it is also the
/// shape of a live run between locking a fresh name and creating its directory.
/// The lock is therefore unlinked only while this sweep possesses it and a
/// second absence check proves that no directory appeared during acquisition.
fn reclaim_abandoned(temporary: &Path) {
    let Ok(listing) = fs::read_dir(temporary) else {
        return;
    };
    let candidates: Vec<PathBuf> = listing.flatten().map(|entry| entry.path()).collect();
    for workspace in candidates
        .iter()
        .filter(|path| is_workspace_directory(path))
    {
        let Ok(lock) = workspace_lock(workspace) else {
            continue;
        };
        // Acquiring creates the lock file, so a workspace that never had one is
        // asked about rather than locked into existence.
        if !lock.path().try_exists().unwrap_or(false) {
            continue;
        }
        if let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() {
            let _ = release_workspace(workspace, &possession);
        }
    }
    for lock_path in candidates {
        reclaim_parentless_lock(&lock_path);
    }
}

/// Remove one gate tree lock that has no workspace and no live holder.
fn reclaim_parentless_lock(lock_path: &Path) {
    let Some(workspace) = workspace_named_by_lock(lock_path) else {
        return;
    };
    if !matches!(workspace.try_exists(), Ok(false)) || !is_workspace_lock_file(lock_path) {
        return;
    }
    let Ok(lock) = workspace_lock(&workspace) else {
        return;
    };
    if lock.path() != lock_path {
        return;
    }
    let Ok(Acquisition::Acquired(possession)) = lock.try_acquire() else {
        return;
    };
    if matches!(workspace.try_exists(), Ok(false)) {
        let _ = fs::remove_file(possession.lock().path());
    }
}

/// Resolve the workspace named by one regular gate tree lock candidate.
fn workspace_named_by_lock(lock_path: &Path) -> Option<PathBuf> {
    let name = lock_path.file_name()?.to_str()?;
    let workspace = name.strip_suffix(".lock")?;
    workspace
        .starts_with(WORKSPACE_PREFIX)
        .then(|| lock_path.with_file_name(workspace))
}

/// Whether this path is a regular gate tree lock, without following a link.
fn is_workspace_lock_file(path: &Path) -> bool {
    workspace_named_by_lock(path).is_some()
        && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Whether this path is a directory named as a gate tree workspace.
///
/// The lock file beside a workspace shares its prefix, so the kind is checked
/// rather than assumed, and it is checked without following a symlink: a link
/// planted under this name would otherwise be dereferenced by the very call
/// deciding whether to remove it.
fn is_workspace_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with(WORKSPACE_PREFIX)
        && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Remove one workspace whose lock this caller possesses.
///
/// A materialized tree is sealed read-only, and a sealed directory cannot be
/// read in order to remove what is under it, so the seal is lifted first.
/// Lifting it is safe only because the possession already proves that no live
/// run is measuring this tree.
fn reclaim_workspace(workspace: &Path, possession: &Possession) -> HeldRootRemoval {
    make_removable(workspace);
    held_root::remove_held_root(workspace, possession)
}

/// Remove one workspace and then the lock that guarded it.
///
/// The lock is kept beside the workspace rather than inside it so that removing
/// the workspace cannot remove the evidence that it was held. That evidence has
/// nothing left to describe once the workspace is gone, and a workspace name
/// carries a fresh nonce per run, so a lock retained for every run ever made
/// would accumulate exactly what this reclaim exists to prevent.
///
/// The order is what keeps that safe. The lock is unlinked last, and only on a
/// removal that succeeded, so this never leaves a workspace standing with no
/// lock beside it -- which is the one state [`reclaim_abandoned`] declines to
/// act on, and so the one state a workspace could never be reclaimed from.
fn release_workspace(workspace: &Path, possession: &Possession) -> HeldRootRemoval {
    let removal = reclaim_workspace(workspace, possession);
    if matches!(removal, HeldRootRemoval::Removed) {
        let _ = fs::remove_file(possession.lock().path());
    }
    removal
}

fn create_private_workspace(path: &Path) -> io::Result<()> {
    #[cfg(target_family = "unix")]
    {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }
    #[cfg(not(target_family = "unix"))]
    {
        fs::create_dir(path)
    }
}

impl Drop for BuildRoot {
    fn drop(&mut self) {
        if let Some(possession) = self.possession.take() {
            let _ = release_workspace(&self.workspace, &possession);
        }
    }
}

/// The Git directories one live repository publishes durable state below.
///
/// Both are named because a linked worktree has two: its own directory, which
/// is where a predecessor wrote per-worktree state, and the common directory
/// every linked worktree shares, which is where one authority lives. A caller
/// that resolved only one of them would either give every worktree a private
/// ledger or give them all one worktree's imports.
pub(crate) fn git_directories(repository: &Path) -> Result<(PathBuf, PathBuf), TreeError> {
    let repo = open_repository(repository)?;
    Ok((repo.path().to_path_buf(), repo.common_dir().to_path_buf()))
}

/// Capture the one stable set of regular paths named by the current Git index.
///
/// The index sources are sealed before they are opened and rechecked after
/// every entry is admitted, just as staged-tree materialization does. Object
/// identities are deliberately discarded: a live contract measures current
/// checkout bytes, while the index supplies only the closed set of path names.
pub(crate) fn tracked_paths(repository: &Path) -> Result<Vec<String>, TreeError> {
    let repo = open_repository(repository)?;
    let seal = IndexSeal::capture(&repo)?;
    let build = BuildRoot::create()?;
    let index = seal.open(&repo, &build.workspace)?;
    let items = index_items(&index)?;
    seal.verify(&repo)?;
    Ok(items.into_iter().map(|item| item.path).collect())
}

pub(crate) fn open_repository(path: &Path) -> Result<gix::Repository, TreeError> {
    let options = gix::open::Options::isolated()
        .strict_config(true)
        .config_overrides(["gitoxide.objects.noReplace=true", "index.skipHash=false"]);
    gix::open_opts(path, options).map_err(|source| TreeError::Repository {
        path: path.to_path_buf(),
        evidence: source.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileMode {
    Regular,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    path: String,
    id: gix::ObjectId,
    mode: FileMode,
}

enum SelectionSeal {
    Index(IndexSeal),
    Revision {
        before: RevisionSeal,
        revision: String,
    },
}

impl SelectionSeal {
    fn verify(&self, repo: &gix::Repository) -> Result<(), TreeError> {
        match self {
            Self::Index(before) => before.verify(repo)?,
            Self::Revision { before, revision } => {
                if RevisionSeal::capture(repo, revision).ok().as_ref() != Some(before) {
                    return Err(TreeError::RevisionChanged {
                        revision: revision.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn index_items(index: &gix::index::File) -> Result<Vec<Item>, TreeError> {
    let mut items = Vec::with_capacity(index.entries().len());
    for entry in index.entries() {
        let path = normalized_path(entry.path(index))?;
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(TreeError::UnmergedIndex {
                path,
                stage: entry.stage_raw(),
            });
        }
        let mode = if entry.mode == gix::index::entry::Mode::FILE {
            FileMode::Regular
        } else if entry.mode == gix::index::entry::Mode::FILE_EXECUTABLE {
            FileMode::Executable
        } else if entry.mode == gix::index::entry::Mode::SYMLINK {
            return Err(TreeError::Symlink { path });
        } else if entry.mode == gix::index::entry::Mode::COMMIT {
            return Err(TreeError::Submodule { path });
        } else {
            return Err(TreeError::UnsupportedMode {
                path,
                mode: entry.mode.bits(),
            });
        };
        items.push(Item {
            path,
            id: entry.id,
            mode,
        });
    }
    normalize_items(items)
}

fn resolve_revision(repo: &gix::Repository, revision: &str) -> Result<gix::ObjectId, TreeError> {
    refuse_ambiguous_reference(repo, revision)?;
    let options = gix::revision::spec::parse::Options {
        refs_hint: gix::revision::spec::parse::RefsHint::Fail,
        object_kind_hint: None,
    };
    let spec = gix::revision::Spec::from_bstr(revision, repo, options).map_err(|source| {
        TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: source.to_string(),
        }
    })?;
    spec.single()
        .map(gix::Id::detach)
        .ok_or_else(|| TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: "the revision names a range rather than one object".to_owned(),
        })
}

fn refuse_ambiguous_reference(repo: &gix::Repository, revision: &str) -> Result<(), TreeError> {
    let atom_end = revision
        .char_indices()
        .find_map(|(index, character)| "~^:@".contains(character).then_some(index))
        .unwrap_or(revision.len());
    let atom = &revision[..atom_end];
    if atom.is_empty() || atom == "HEAD" || atom.starts_with("refs/") {
        return Ok(());
    }

    let candidates = [
        format!("refs/{atom}"),
        format!("refs/tags/{atom}"),
        format!("refs/heads/{atom}"),
        format!("refs/remotes/{atom}"),
        format!("refs/remotes/{atom}/HEAD"),
    ];
    let mut matched = Vec::new();
    for candidate in candidates {
        match repo.try_find_reference(&candidate) {
            Ok(Some(_)) => matched.push(candidate),
            Ok(None) => {}
            Err(source) => {
                return Err(TreeError::RevisionUnresolved {
                    revision: revision.to_owned(),
                    evidence: source.to_string(),
                });
            }
        }
    }
    if matched.len() > 1 {
        return Err(TreeError::RevisionUnresolved {
            revision: revision.to_owned(),
            evidence: format!(
                "the reference name is ambiguous among {}",
                matched.join(", ")
            ),
        });
    }
    Ok(())
}

fn revision_items(
    repo: &gix::Repository,
    revision: &str,
    commit_id: gix::ObjectId,
) -> Result<Vec<Item>, TreeError> {
    let commit = verified_object(repo, commit_id, revision)?;
    if commit.kind != gix::objs::Kind::Commit {
        return Err(TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
            actual: commit.kind.to_string(),
        });
    }
    let commit = commit
        .try_into_commit()
        .map_err(|source| TreeError::RevisionNotCommit {
            revision: revision.to_owned(),
            actual: source.actual.to_string(),
        })?;
    let tree_id =
        commit
            .tree_id()
            .map(gix::Id::detach)
            .map_err(|source| TreeError::MalformedObject {
                object: commit_id.to_string(),
                evidence: source.to_string(),
            })?;

    let mut items = Vec::new();
    collect_tree(repo, tree_id, "", &mut items)?;
    normalize_items(items)
}

fn collect_tree(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    prefix: &str,
    items: &mut Vec<Item>,
) -> Result<(), TreeError> {
    let label = if prefix.is_empty() { "." } else { prefix };
    let object = verified_object(repo, tree_id, label)?;
    if object.kind != gix::objs::Kind::Tree {
        return Err(TreeError::UnexpectedObject {
            path: label.to_owned(),
            object: tree_id.to_string(),
            expected: "tree",
            actual: object.kind.to_string(),
        });
    }
    let tree = object
        .try_into_tree()
        .map_err(|source| TreeError::UnexpectedObject {
            path: label.to_owned(),
            object: tree_id.to_string(),
            expected: "tree",
            actual: source.actual.to_string(),
        })?;
    let entries = tree
        .iter()
        .map(|entry| {
            entry
                .map(|entry| (entry.filename().to_vec(), entry.object_id(), entry.mode()))
                .map_err(|source| TreeError::MalformedObject {
                    object: tree_id.to_string(),
                    evidence: source.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for (name, id, mode) in entries {
        let name = normalized_component(&name)?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        match mode.value() {
            0o040000 => collect_tree(repo, id, &path, items)?,
            0o100644 => items.push(Item {
                path,
                id,
                mode: FileMode::Regular,
            }),
            0o100755 => items.push(Item {
                path,
                id,
                mode: FileMode::Executable,
            }),
            0o120000 => return Err(TreeError::Symlink { path }),
            0o160000 => return Err(TreeError::Submodule { path }),
            unsupported => {
                return Err(TreeError::UnsupportedMode {
                    path,
                    mode: u32::from(unsupported),
                });
            }
        }
    }
    Ok(())
}

fn normalize_items(mut items: Vec<Item>) -> Result<Vec<Item>, TreeError> {
    items.sort_by(|left, right| left.path.cmp(&right.path));
    for pair in items.windows(2) {
        let left = &pair[0].path;
        let right = &pair[1].path;
        if left == right
            || right
                .strip_prefix(left)
                .is_some_and(|tail| tail.starts_with('/'))
        {
            return Err(TreeError::PathCollision {
                first: left.clone(),
                second: right.clone(),
            });
        }
    }
    Ok(items)
}

fn normalized_path(path: &[u8]) -> Result<String, TreeError> {
    let path = std::str::from_utf8(path).map_err(|_| TreeError::UnnameablePath {
        path: String::from_utf8_lossy(path).into_owned(),
    })?;
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains(['\\', '\n', '\r', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(TreeError::UnnameablePath {
            path: path.to_owned(),
        });
    }
    Ok(path.to_owned())
}

pub(crate) fn normalized_component(component: &[u8]) -> Result<String, TreeError> {
    let component = std::str::from_utf8(component).map_err(|_| TreeError::UnnameablePath {
        path: String::from_utf8_lossy(component).into_owned(),
    })?;
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains(['/', '\\', '\n', '\r', '\0'])
    {
        return Err(TreeError::UnnameablePath {
            path: component.to_owned(),
        });
    }
    Ok(component.to_owned())
}

fn materialize_items(repo: &gix::Repository, root: &Path, items: &[Item]) -> Result<(), TreeError> {
    let mut directories = BTreeSet::new();
    directories.insert(root.to_path_buf());
    for item in items {
        let destination = root.join(&item.path);
        let parent = destination
            .parent()
            .expect("a normalized relative path has a parent below its root");
        if directories.insert(parent.to_path_buf()) {
            fs::create_dir_all(parent).map_err(|source| {
                TreeError::io("creating a selected tree directory", parent, source)
            })?;
            private_directory_chain(root, parent)?;
        }

        let object = verified_object(repo, item.id, &item.path)?;
        if object.kind != gix::objs::Kind::Blob {
            return Err(TreeError::UnexpectedObject {
                path: item.path.clone(),
                object: item.id.to_string(),
                expected: "blob",
                actual: object.kind.to_string(),
            });
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|source| {
                TreeError::io("creating a selected tree file", &destination, source)
            })?;
        file.write_all(&object.data).map_err(|source| {
            TreeError::io("writing a selected tree file", &destination, source)
        })?;
        drop(file);
        immutable_file(&destination, item.mode)?;
    }
    Ok(())
}

pub(crate) fn verified_object<'repo>(
    repo: &'repo gix::Repository,
    id: gix::ObjectId,
    path: &str,
) -> Result<gix::Object<'repo>, TreeError> {
    let object = repo
        .find_object(id)
        .map_err(|source| TreeError::MissingObject {
            path: path.to_owned(),
            object: id.to_string(),
            evidence: source.to_string(),
        })?;
    let actual =
        gix::objs::compute_hash(id.kind(), object.kind, &object.data).map_err(|source| {
            TreeError::MalformedObject {
                object: id.to_string(),
                evidence: source.to_string(),
            }
        })?;
    if actual != id {
        return Err(TreeError::ObjectChanged {
            path: path.to_owned(),
            expected: id.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(object)
}

fn private_directory(path: &Path) -> Result<(), TreeError> {
    #[cfg(target_family = "unix")]
    let permissions = fs::Permissions::from_mode(0o700);
    #[cfg(not(target_family = "unix"))]
    let permissions = {
        let mut permissions = fs::metadata(path)
            .map_err(|source| TreeError::io("inspecting a private directory", path, source))?
            .permissions();
        permissions.set_readonly(false);
        permissions
    };
    fs::set_permissions(path, permissions)
        .map_err(|source| TreeError::io("making a selected directory private", path, source))
}

fn private_directory_chain(root: &Path, directory: &Path) -> Result<(), TreeError> {
    let relative = directory
        .strip_prefix(root)
        .expect("selected directory remains below its private root");
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        private_directory(&current)?;
    }
    Ok(())
}

fn create_private_directory_tree(root: &Path, directory: &Path) -> Result<(), TreeError> {
    fs::create_dir_all(directory).map_err(|source| {
        TreeError::io("creating captured source directories", directory, source)
    })?;
    private_directory_chain(root, directory)
}

fn write_private_source(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), TreeError> {
    let parent = path
        .parent()
        .expect("a captured source file has a parent below its root");
    create_private_directory_tree(root, parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| TreeError::io("creating a captured source file", path, source))?;
    file.write_all(bytes)
        .map_err(|source| TreeError::io("writing a captured source file", path, source))?;
    drop(file);
    immutable_file(path, FileMode::Regular)
}

fn immutable_file(path: &Path, mode: FileMode) -> Result<(), TreeError> {
    #[cfg(target_family = "unix")]
    let permissions = fs::Permissions::from_mode(match mode {
        FileMode::Regular => 0o400,
        FileMode::Executable => 0o500,
    });
    #[cfg(not(target_family = "unix"))]
    let permissions = {
        let mut permissions = fs::metadata(path)
            .map_err(|source| TreeError::io("inspecting a selected file", path, source))?
            .permissions();
        permissions.set_readonly(true);
        permissions
    };
    fs::set_permissions(path, permissions)
        .map_err(|source| TreeError::io("making a selected file immutable", path, source))
}

fn make_immutable(root: &Path) -> Result<(), TreeError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        let directory = directories[index].clone();
        index += 1;
        let listing = fs::read_dir(&directory)
            .map_err(|source| TreeError::io("sealing a selected directory", &directory, source))?;
        for entry in listing {
            let entry = entry.map_err(|source| {
                TreeError::io("reading a selected directory", &directory, source)
            })?;
            if entry
                .file_type()
                .map_err(|source| {
                    TreeError::io("inspecting a selected path", &entry.path(), source)
                })?
                .is_dir()
            {
                directories.push(entry.path());
            }
        }
    }
    for directory in directories.into_iter().rev() {
        #[cfg(target_family = "unix")]
        let permissions = fs::Permissions::from_mode(0o500);
        #[cfg(not(target_family = "unix"))]
        let permissions = {
            let mut permissions = fs::metadata(&directory)
                .map_err(|source| {
                    TreeError::io("inspecting a selected directory", &directory, source)
                })?
                .permissions();
            permissions.set_readonly(true);
            permissions
        };
        fs::set_permissions(&directory, permissions).map_err(|source| {
            TreeError::io("making a selected directory immutable", &directory, source)
        })?;
    }
    Ok(())
}

fn verify_immutable(root: &Path, items: &[Item]) -> Result<(), TreeError> {
    let expected_files = items
        .iter()
        .map(|item| item.path.clone())
        .collect::<BTreeSet<_>>();
    let mut expected_directories = BTreeSet::from([String::new()]);
    for item in items {
        let mut parent = Path::new(&item.path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            expected_directories.insert(path.to_string_lossy().into_owned());
            parent = path.parent();
        }
    }
    let mut observed_files = BTreeSet::new();
    let mut observed_directories = BTreeSet::new();
    let mut directories = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = directories.pop() {
        observed_directories.insert(relative.clone());
        verify_directory(
            &directory,
            if relative.is_empty() { "." } else { &relative },
        )?;
        let listing = fs::read_dir(&directory).map_err(|source| {
            TreeError::io("verifying a selected directory", &directory, source)
        })?;
        for entry in listing {
            let entry = entry.map_err(|source| {
                TreeError::io("reading a selected directory", &directory, source)
            })?;
            let name = entry.file_name().into_string().map_err(|name| {
                TreeError::MaterializationChanged {
                    path: directory.join(name).display().to_string(),
                }
            })?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let file_type = entry.file_type().map_err(|source| {
                TreeError::io("inspecting a selected path", &entry.path(), source)
            })?;
            if file_type.is_dir() {
                directories.push((entry.path(), path));
            } else if file_type.is_file() {
                observed_files.insert(path);
            } else {
                return Err(TreeError::MaterializationChanged { path });
            }
        }
    }
    if expected_directories != observed_directories {
        let path = expected_directories
            .symmetric_difference(&observed_directories)
            .next()
            .cloned()
            .unwrap_or_else(|| "selected tree".to_owned());
        return Err(TreeError::MaterializationChanged { path });
    }
    if expected_files != observed_files {
        let path = expected_files
            .symmetric_difference(&observed_files)
            .next()
            .cloned()
            .unwrap_or_else(|| "selected tree".to_owned());
        return Err(TreeError::MaterializationChanged { path });
    }

    for item in items {
        let path = root.join(&item.path);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| TreeError::io("verifying a selected file", &path, source))?;
        if !metadata.is_file() {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
        let bytes = fs::read(&path)
            .map_err(|source| TreeError::io("verifying selected bytes", &path, source))?;
        let actual = gix::objs::compute_hash(item.id.kind(), gix::objs::Kind::Blob, &bytes)
            .map_err(|_| TreeError::MaterializationChanged {
                path: item.path.clone(),
            })?;
        if actual != item.id {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
        #[cfg(target_family = "unix")]
        {
            let expected = match item.mode {
                FileMode::Regular => 0o400,
                FileMode::Executable => 0o500,
            };
            if metadata.mode() & 0o777 != expected {
                return Err(TreeError::MaterializationChanged {
                    path: item.path.clone(),
                });
            }
        }
        #[cfg(not(target_family = "unix"))]
        if !metadata.permissions().readonly() {
            return Err(TreeError::MaterializationChanged {
                path: item.path.clone(),
            });
        }
    }
    Ok(())
}

fn verify_directory(path: &Path, relative: &str) -> Result<(), TreeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| TreeError::io("verifying a selected directory", path, source))?;
    if !metadata.is_dir() {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    #[cfg(target_family = "unix")]
    if metadata.mode() & 0o777 != 0o500 {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    #[cfg(not(target_family = "unix"))]
    if !metadata.permissions().readonly() {
        return Err(TreeError::MaterializationChanged {
            path: relative.to_owned(),
        });
    }
    Ok(())
}

fn make_removable(root: &Path) {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return;
    };
    if metadata.is_dir() {
        set_owner_writable(root, &metadata);
        if let Ok(listing) = fs::read_dir(root) {
            for entry in listing.flatten() {
                make_removable(&entry.path());
            }
        }
    } else if metadata.is_file() {
        set_owner_writable(root, &metadata);
    }
}

fn set_owner_writable(path: &Path, metadata: &fs::Metadata) {
    #[cfg(target_family = "unix")]
    {
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(target_family = "unix"))]
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}
