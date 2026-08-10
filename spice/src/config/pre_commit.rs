//! Typed predecessor pre-commit bridge declarations.

use serde::{Deserialize, Serialize};

use crate::config::Root;

use super::{nonempty_argv, nonempty_string, unique_names};

/// Return the sole Rust bridge only when the predecessor battery is disabled.
///
/// This keeps the two halves of the handoff inseparable for typed consumers:
/// enabling the Python builtins or declaring zero or multiple commands makes
/// the bridge unavailable.
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

/// One current predecessor pre-commit command admitted by tracked policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreCommitStep {
    #[serde(deserialize_with = "nonempty_string")]
    label: String,
    #[serde(deserialize_with = "nonempty_argv")]
    run: Vec<String>,
    #[serde(default, skip_serializing_if = "PreCommitScopes::is_empty")]
    scopes: PreCommitScopes,
}

impl PreCommitStep {
    /// Human-readable identity shown by the predecessor gate.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Exact executable and arguments, retained in authored order.
    #[must_use]
    pub fn run(&self) -> &[String] {
        &self.run
    }

    /// Optional path selection; an empty selection means every commit.
    #[must_use]
    pub const fn scopes(&self) -> &PreCommitScopes {
        &self.scopes
    }
}

/// Current path selection carried by a predecessor pre-commit command.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreCommitScopes {
    #[serde(deserialize_with = "unique_names")]
    paths: Vec<String>,
}

impl PreCommitScopes {
    /// Authored path selectors, empty when the command is unscoped.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}
