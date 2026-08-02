//! Elastic bounds and durable gate state.
//!
//! A run loads one logical ledger, computes all latch and claim changes in
//! memory, and publishes them only through [`PendingState::accept`]. The
//! current ledger lives below Git's common directory, so linked worktrees see
//! one authority. When that ledger is absent, the five Python-owned sticky
//! ledgers and its shared claim ledger are translated in memory exactly once;
//! an accepted run makes that import durable.

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

/// Current Rust-owned ledger schema.
pub const STATE_LEDGER_VERSION: u64 = 1;
/// Current immutable state-transition receipt schema.
pub const STATE_RECEIPT_VERSION: u64 = 1;
/// Release that owns the imported Python state spellings.
pub const PREDECESSOR_STATE_RELEASE: &str = "0.31.0";
/// Lifetime of one peer flex claim.
pub const PEER_CLAIM_TTL_SECONDS: u64 = 6 * 60 * 60;

const CURRENT_STATE_RELEASE: &str = "current-rust";
const CURRENT_STATE_DIRECTORY: &str = ".spice-rust";
const CURRENT_STATE_DATA_DIRECTORY: &str = "data";
const CURRENT_STATE_FILE: &str = "gate-state.json";
const CURRENT_STATE_LOCK_FILE: &str = "gate-state.lock";
const PREDECESSOR_CLAIMS_FILE: &str = "flex-slice-claims.json";

/// The Rust-owned gate-state path below one canonical Git common directory.
#[must_use]
pub fn current_state_path(common_git_directory: &Path) -> PathBuf {
    common_git_directory
        .join(CURRENT_STATE_DIRECTORY)
        .join(CURRENT_STATE_DATA_DIRECTORY)
        .join(CURRENT_STATE_FILE)
}

include!("state/bound.rs");
include!("state/ledger.rs");
include!("state/transaction.rs");
include!("state/migration.rs");

#[cfg(test)]
mod fixtures;
