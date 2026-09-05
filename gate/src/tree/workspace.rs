//! Private materialization workspace ownership and abandoned-root reclamation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_family = "unix")]
use std::os::unix::fs::MetadataExt as _;

use sill::{Acquisition, LockFile, Possession};

use super::{
    SelectedFiles, SelectedTree, SelectionSeal, StagedAuthority, TreeError,
    create_private_workspace, make_removable, private_directory,
};
use crate::held_root::{self, HeldRootRemoval};
use crate::state::RenameMap;

/// How gate tree selection names itself in a refusal, and names its locks.
const WORKSPACE_LABEL: &str = "gate tree workspace";
const WORKSPACE_LOCK: &str = "gate-tree-workspace";

/// The prefix every gate tree workspace name begins with.
const WORKSPACE_PREFIX: &str = "spice-gate-tree-";
const WORKSPACE_PARENT_PREFIX: &str = "spice-gate-workspaces-";

#[derive(Debug)]
pub(super) struct BuildRoot {
    pub(super) workspace: PathBuf,
    pub(super) root: PathBuf,
    possession: Option<Possession>,
}

impl BuildRoot {
    pub(super) fn create() -> Result<Self, TreeError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = workspace_parent()?;
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
    pub(super) fn finish(
        &mut self,
        repository: &Path,
        source: SelectionSeal,
        files: SelectedFiles,
        renames: RenameMap,
        staged_authority: Option<StagedAuthority>,
        publishes_state: bool,
    ) -> SelectedTree {
        SelectedTree {
            workspace: self.workspace.clone(),
            root: self.root.clone(),
            repository: repository.to_path_buf(),
            source,
            files,
            renames,
            staged_authority,
            publishes_state,
            possession: self.possession.take(),
        }
    }
}

pub(super) fn workspace_parent() -> Result<PathBuf, TreeError> {
    let temporary = std::env::temp_dir();
    #[cfg(target_family = "unix")]
    let owner = {
        // SAFETY: geteuid has no preconditions and observes this process only.
        unsafe { libc::geteuid() }
    };
    #[cfg(not(target_family = "unix"))]
    let owner = std::process::id();
    let parent = temporary.join(format!("{WORKSPACE_PARENT_PREFIX}{owner}"));
    let created = match create_private_workspace(&parent) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(source) => {
            return Err(TreeError::io(
                "creating the selected-tree workspace parent",
                &parent,
                source,
            ));
        }
    };
    validate_workspace_parent(&parent, owner)?;
    if created {
        reclaim_abandoned(&temporary);
    }
    Ok(parent)
}

fn validate_workspace_parent(path: &Path, owner: u32) -> Result<(), TreeError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        TreeError::io(
            "inspecting the selected-tree workspace parent",
            path,
            source,
        )
    })?;
    #[cfg(target_family = "unix")]
    let valid = metadata.is_dir() && metadata.uid() == owner && metadata.mode() & 0o777 == 0o700;
    #[cfg(not(target_family = "unix"))]
    let valid = metadata.is_dir();
    if valid {
        return Ok(());
    }
    Err(TreeError::io(
        "validating the selected-tree workspace parent",
        path,
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "workspace parent must be an owner-private real directory",
        ),
    ))
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
pub(super) fn workspace_lock(workspace: &Path) -> Result<LockFile, String> {
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
pub(super) fn reclaim_abandoned(temporary: &Path) {
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
pub(super) fn release_workspace(workspace: &Path, possession: &Possession) -> HeldRootRemoval {
    let removal = reclaim_workspace(workspace, possession);
    if matches!(removal, HeldRootRemoval::Removed) {
        let _ = fs::remove_file(possession.lock().path());
    }
    removal
}

impl Drop for BuildRoot {
    fn drop(&mut self) {
        if let Some(possession) = self.possession.take() {
            let _ = release_workspace(&self.workspace, &possession);
        }
    }
}
