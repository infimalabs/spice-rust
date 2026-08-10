//! Typed predecessor pre-commit bridge selection.

use crate::config::Root;

use super::PreCommitStep;

/// Return the sole Rust bridge only when the predecessor battery is disabled.
#[must_use]
pub fn predecessor_bridge(root: &Root) -> Option<&PreCommitStep> {
    let policy = root.policy();
    if policy.pre_commit_builtins() {
        return None;
    }
    let [bridge] = policy.pre_commit() else {
        return None;
    };
    Some(bridge)
}
