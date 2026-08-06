//! The directory one analysis writes into, and the lock that says who holds it.
//!
//! Both compiling arms resolve a fixed sibling of the selected tree, named from
//! that tree and carrying no token the invocation minted. Two analyses of one
//! tree therefore name the same directory, and a root left behind by a run that
//! was killed before its own teardown is indistinguishable, by looking at it,
//! from a root a live analysis is writing into right now. An arm that could see
//! only the directory has to treat both alike -- refuse, and send an operator to
//! clean up by hand after every killed run.
//!
//! What tells them apart is an advisory lock at `<root>.lock`, held for the life
//! of the workspace, and the invariant is that **an arm removes a build root
//! only while it possesses that root's lock**. [`remove_build_root`] takes a
//! [`Possession`] rather than a path, so the invariant is carried by the
//! signature and a removal cannot be spelled without the lock in hand, and
//! [`hold`] is the only way to obtain one. Contention is therefore not a removal
//! that gets refused; it is a removal that never becomes reachable, because a
//! contended arm returns before it owns anything to tear down.
//!
//! `flock` grants per-open-file-description, so a second acquisition contends
//! even from inside this process; that is what lets a fixture hold a build root
//! and drive the decision directly rather than having to lose a race to observe
//! it.
//!
//! The lock is kept beside the root rather than inside it, so removing the root
//! cannot remove the evidence that it was held. Each arm passes its own label
//! and lock name, so every refusal names the arm the operator ran.

use std::cell::Cell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sill::{Acquisition, LockFile, LockName, Possession};

/// How the canvas arm names itself in a refusal, and names its lock.
const CANVAS_LABEL: &str = "canvas analysis workspace";
const CANVAS_LOCK: &str = "canvas-analysis-workspace";

/// What one attempt to remove a build root did, so no attempt is discarded.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum BuildRootRemoval {
    /// The root is gone, whether this attempt unlinked it or found it absent.
    Removed,
    /// The root still exists and the reason is carried rather than dropped.
    Failed(String),
}

impl BuildRootRemoval {
    /// The operator-facing reason, or `None` when there is nothing to report.
    ///
    /// A root that cannot be removed refuses every later analysis of that tree,
    /// because [`hold`] reclaims before it hands back a possession and fails
    /// with this same text. The report when the removal first fails and the
    /// refusal an operator eventually meets name the same path on purpose: one
    /// is the warning, the other is the consequence.
    pub(super) fn reason(&self, label: &str, root: &Path) -> Option<String> {
        match self {
            Self::Removed => None,
            Self::Failed(error) => Some(format!(
                "{label} {} could not be removed: {error}; every later analysis of this tree \
                 refuses until it is gone",
                root.display()
            )),
        }
    }
}

/// Remove a build root whose lock this caller already possesses.
///
/// Taking [`Possession`] rather than a path alone is the guard: there is no way
/// to spell a removal without first holding the lock, so a root a live analysis
/// holds is unreachable here rather than merely unlucky. Absence counts as
/// removed; every other refusal is carried back out.
pub(super) fn remove_build_root(root: &Path, _possession: &Possession) -> BuildRootRemoval {
    match fs::remove_dir_all(root) {
        Ok(()) => BuildRootRemoval::Removed,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BuildRootRemoval::Removed,
        Err(error) => BuildRootRemoval::Failed(error.to_string()),
    }
}

/// The lock guarding one build root, kept beside the root rather than inside
/// it so removing the root cannot remove the evidence that it was held.
pub(super) fn build_root_lock(root: &Path, lock: &str, label: &str) -> Result<LockFile, String> {
    let Some(name) = root.file_name() else {
        return Err(format!("{label} {} has no name to lock", root.display()));
    };
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    let path = root.with_file_name(lock_name);
    let name =
        LockName::new(lock).map_err(|error| format!("{label} lock name was refused: {error}"))?;
    Ok(LockFile::new(name, path))
}

/// The lock the canvas arm takes, so a fixture can hold the same one.
///
/// A fixture deriving the path itself would prove only that some lock contends;
/// this hands back the one [`CanvasWorkspace::create`] acquires.
#[cfg(test)]
pub(super) fn canvas_workspace_lock(root: &Path) -> Result<LockFile, String> {
    build_root_lock(root, CANVAS_LOCK, CANVAS_LABEL)
}

/// Take this root's lock, and reclaim whatever a killed analysis left in it.
///
/// The returned [`Possession`] is the arm's proof that no live analysis holds
/// this root, which is what makes reclaiming safe: a root whose lock is free was
/// left by a run that died before its own teardown. A contended lock is a live
/// analysis, and this returns before the caller owns anything to tear down.
pub(super) fn hold(root: &Path, lock: &str, label: &str) -> Result<Possession, String> {
    let lock = build_root_lock(root, lock, label)?;
    let possession = match lock.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => possession,
        Ok(Acquisition::Contended) => {
            return Err(format!(
                "{label} {} is in use by another analysis of this tree",
                root.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "{label} {} could not be locked: {error}",
                root.display()
            ));
        }
    };
    match remove_build_root(root, &possession).reason(label, root) {
        Some(reason) => Err(reason),
        None => Ok(possession),
    }
}

/// A sibling directory one canvas analysis holds for the length of its run.
///
/// The possession is kept in an [`Option`] so [`Drop`] can take it: the removal
/// this workspace owes runs while the lock is still held, and the reason it
/// produces goes into the caller's cell rather than being discarded, because
/// `Drop` has no other way to report.
pub(super) struct CanvasWorkspace<'a> {
    root: PathBuf,
    removal: &'a Cell<Option<String>>,
    possession: Option<Possession>,
}

impl<'a> CanvasWorkspace<'a> {
    pub(super) fn create(
        selected: &Path,
        removal: &'a Cell<Option<String>>,
    ) -> Result<Self, String> {
        let Some(name) = selected.file_name() else {
            return Err(format!(
                "canvas selected tree {selected:?} has no name for an analysis workspace"
            ));
        };
        let mut workspace_name = name.to_os_string();
        workspace_name.push(".spice-canvas-analysis");
        let root = selected.with_file_name(workspace_name);
        let possession = hold(&root, CANVAS_LOCK, CANVAS_LABEL)?;
        fs::create_dir(&root).map_err(|error| {
            format!(
                "{CANVAS_LABEL} {} could not be created: {error}",
                root.display()
            )
        })?;
        Ok(Self {
            root,
            removal,
            possession: Some(possession),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for CanvasWorkspace<'_> {
    fn drop(&mut self) {
        let Some(possession) = self.possession.take() else {
            return;
        };
        let removal = remove_build_root(&self.root, &possession);
        if let Some(reason) = removal.reason(CANVAS_LABEL, &self.root) {
            self.removal.set(Some(reason));
        }
    }
}
