//! Identity of the exact tree a staged commit would record.

use std::ffi::OsStr;
use std::fmt;
use std::process::Command;

use crate::repository::Repository;

use super::{GIT, RunError};

/// Canonical Git object identity for a prospective staged commit tree.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CandidateTree(String);

impl CandidateTree {
    /// Canonical lowercase hexadecimal Git object identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CandidateTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub(super) fn capture_candidate_tree(
    repository: &Repository,
    index: Option<&OsStr>,
) -> Result<CandidateTree, RunError> {
    let mut command = Command::new(GIT);
    command
        .arg("-C")
        .arg(repository.worktree_root())
        .arg("write-tree");
    match index {
        Some(index) => {
            command.env("GIT_INDEX_FILE", index);
        }
        None => {
            command.env_remove("GIT_INDEX_FILE");
        }
    }
    let output = command
        .output()
        .map_err(|error| RunError::fault("gate::git-unavailable", error))?;
    if !output.status.success() {
        return Err(RunError::refusal(
            "gate::git-refused",
            format!(
                "{GIT} write-tree exited {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    parse_candidate_tree(&output.stdout)
}

fn parse_candidate_tree(output: &[u8]) -> Result<CandidateTree, RunError> {
    let identity = output.strip_suffix(b"\n").ok_or_else(|| {
        RunError::refusal(
            "gate::candidate-tree-refused",
            "Git write-tree output is not newline terminated",
        )
    })?;
    let canonical_length = matches!(identity.len(), 40 | 64);
    let canonical_hex = identity
        .iter()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !canonical_length || !canonical_hex {
        return Err(RunError::refusal(
            "gate::candidate-tree-refused",
            "Git write-tree output is not one canonical object identity",
        ));
    }
    let identity = std::str::from_utf8(identity)
        .map_err(|error| RunError::refusal("gate::candidate-tree-refused", error))?;
    Ok(CandidateTree(identity.to_owned()))
}

#[cfg(test)]
mod tests;
