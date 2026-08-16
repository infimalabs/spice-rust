//! Elastic bounds and durable gate state.
//!
//! A run loads one logical ledger, computes all latch and claim changes in
//! memory, and publishes them only through [`PendingState::accept`]. The
//! current ledger lives below Git's common directory, so linked worktrees see
//! one authority. When that ledger is absent, a run starts from empty current
//! state. Predecessor-owned ledgers are outside this module's read and write
//! surface and remain untouched indefinitely.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use blake2::Blake2bVar;
use blake2::digest::{Update, VariableOutput};
use serde::{Deserialize, Serialize};
use sill::{LockFile, LockName};
use wire::{Digest, Domain};

/// Current ledger schema.
pub const STATE_LEDGER_VERSION: u64 = 1;
/// Current immutable state-transition receipt schema.
pub const STATE_RECEIPT_VERSION: u64 = 2;
/// Lifetime of one peer flex claim.
pub const PEER_CLAIM_TTL_SECONDS: u64 = 6 * 60 * 60;

const CURRENT_STATE_RELEASE: &str = "current";
const CURRENT_STATE_DIRECTORY: &str = ".spice-rs";
const CURRENT_STATE_DATA_DIRECTORY: &str = "data";
const CURRENT_STATE_FILE: &str = "gate-state.json";
const CURRENT_STATE_LOCK_FILE: &str = "gate-state.lock";

/// The current gate-state path below one canonical Git common directory.
#[must_use]
pub fn current_state_path(common_git_directory: &Path) -> PathBuf {
    common_git_directory
        .join(CURRENT_STATE_DIRECTORY)
        .join(CURRENT_STATE_DATA_DIRECTORY)
        .join(CURRENT_STATE_FILE)
}

mod bound;
pub use bound::*;
mod ledger;
pub use ledger::*;
mod transaction;
pub use transaction::*;
mod migration;
pub use migration::*;

#[cfg(test)]
mod fixtures;
