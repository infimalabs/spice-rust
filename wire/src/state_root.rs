//! The one name of the Rust state root, and the one way to find it.
//!
//! The Git-common directory carries repository authority below this root, and
//! each linked worktree's own Git directory carries its worktree-local state
//! below a root of the same name. Every child keeps its relative name; nothing
//! outside this module spells the root.

#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

/// Name of the state root this runtime owns beneath a Git directory.
pub const STATE_DIRECTORY: &str = ".spice";

/// Suffix the root carried before this release owned [`STATE_DIRECTORY`].
///
/// A root by that older name is live state, not residue: every admission
/// refuses a symlinked component, so nothing can bridge the two names. The
/// install crossing moves such a root forward exactly once, while no resident
/// or lane holds it; until then [`state_root`] keeps the older name current so
/// no command opens a second root beside the live one.
pub const PREDECESSOR_STATE_SUFFIX: &str = "-rs";

/// The name the state root carried before it took [`STATE_DIRECTORY`].
#[must_use]
pub fn predecessor_state_directory() -> String {
    format!("{STATE_DIRECTORY}{PREDECESSOR_STATE_SUFFIX}")
}

/// The state root below `git_directory`.
///
/// [`STATE_DIRECTORY`] wins whenever something occupies that name. Only when
/// nothing does, and something occupies the predecessor name, is the
/// predecessor root the live one. Shape is not judged here: the admission
/// that opens the root refuses a link or a file at either name.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn state_root(git_directory: &Path) -> PathBuf {
    let current = git_directory.join(STATE_DIRECTORY);
    if occupied(&current) {
        return current;
    }
    let predecessor = git_directory.join(predecessor_state_directory());
    if occupied(&predecessor) {
        predecessor
    } else {
        current
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn occupied(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}
