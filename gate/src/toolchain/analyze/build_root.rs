//! The directory one canvas analysis writes into, and the lock that says who holds it.
//!
//! Both compiling arms resolve a fixed sibling of the selected tree, named from
//! that tree and carrying no token the invocation minted. Two analyses of one
//! tree therefore name the same directory, and a root left behind by a run that
//! was killed before its own teardown is indistinguishable, by looking at it,
//! from a root a live analysis is writing into right now. An arm that could see
//! only the directory has to treat both alike -- refuse, and send an operator to
//! clean up by hand after every killed run.
//!
//! What tells them apart is the advisory lock in [`crate::held_root`], which
//! carries the invariant that a root is removed only under its own lock and why
//! that invariant is spelled in a signature. This module supplies the canvas
//! arm's half: the name of its lock, the label it refuses under, and the
//! workspace that holds one for the length of a run.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
use sill::LockFile;
use sill::{Acquisition, Possession};

use super::rust::{make_cache_writable, synchronize_source};
use crate::held_root::{held_root_lock, hold, remove_held_root};

/// How the canvas arm names itself in a refusal, and names its lock.
const CANVAS_LABEL: &str = "canvas analysis workspace";
const CANVAS_LOCK: &str = "canvas-analysis-workspace";
const CANVAS_SOURCE_LABEL: &str = "canvas analysis source cache";
const CANVAS_SOURCE_LOCK: &str = "canvas-analysis-source-cache";
const CANVAS_SOURCE_DIRECTORY: &str = "target/xtask-app-source";

/// The lock the canvas arm takes, so a fixture can hold the same one.
///
/// A fixture deriving the path itself would prove only that some lock contends;
/// this hands back the one [`CanvasWorkspace::create`] acquires.
#[cfg(test)]
pub(super) fn canvas_workspace_lock(root: &Path) -> Result<LockFile, String> {
    held_root_lock(root, CANVAS_LOCK, CANVAS_LABEL)
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
        let removal = remove_held_root(&self.root, &possession);
        if let Some(reason) = removal.reason(CANVAS_LABEL, &self.root) {
            self.removal.set(Some(reason));
        }
    }
}

/// One stable selected-source mirror held immutable for a Canvas analysis.
///
/// Cargo fingerprints include the workspace path. Keeping that path stable
/// lets the existing persistent WASM target and build directories do useful
/// work across commit-gate runs while the selected tree remains the sole input.
pub(super) struct CanvasSource<'a> {
    root: PathBuf,
    removal: &'a Cell<Option<String>>,
    possession: Option<Possession>,
}

impl<'a> CanvasSource<'a> {
    pub(super) fn create(
        selected: &Path,
        repository: &Path,
        removal: &'a Cell<Option<String>>,
    ) -> Result<Self, String> {
        let root = repository.join(CANVAS_SOURCE_DIRECTORY);
        let Some(parent) = root.parent() else {
            return Err(format!(
                "{CANVAS_SOURCE_LABEL} {} has no parent directory",
                root.display()
            ));
        };
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "{CANVAS_SOURCE_LABEL} parent {} could not be created: {error}",
                parent.display()
            )
        })?;
        let lock = held_root_lock(&root, CANVAS_SOURCE_LOCK, CANVAS_SOURCE_LABEL)?;
        let possession = match lock.try_acquire() {
            Ok(Acquisition::Acquired(possession)) => possession,
            Ok(Acquisition::Contended) => {
                return Err(format!(
                    "{CANVAS_SOURCE_LABEL} {} is in use by another canonical gate",
                    root.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "{CANVAS_SOURCE_LABEL} {} could not be locked: {error}",
                    root.display()
                ));
            }
        };
        synchronize_source(selected, &root).map_err(|error| {
            format!(
                "{CANVAS_SOURCE_LABEL} {} could not mirror {}: {error}",
                root.display(),
                selected.display()
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

impl Drop for CanvasSource<'_> {
    fn drop(&mut self) {
        if self.possession.is_none() {
            return;
        }
        if let Err(error) = make_cache_writable(&self.root) {
            self.removal.set(Some(format!(
                "{CANVAS_SOURCE_LABEL} {} could not be made writable after analysis: {error}",
                self.root.display()
            )));
        }
        drop(self.possession.take());
    }
}
