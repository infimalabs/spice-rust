//! Archive sources protected by live worktrees and open merge resolutions.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use gate::Selector;
use wire::Digest;

use super::{ExactEvidenceDocument, load_exact_full_verifier_evidence, semantic};
use crate::gate::{GIT, prepare_strict_at};
use crate::repository::Repository;

pub(super) fn protected_sources(
    repository: &Repository,
    published: &Digest,
) -> Result<BTreeSet<String>, String> {
    let mut protected = BTreeSet::from([published.to_string()]);
    for root in linked_worktrees(repository)? {
        let linked = Repository::discover(&root)
            .map_err(|error| format!("resolve linked worktree {}: {error}", root.display()))?;
        let paths = linked
            .admit_state()
            .map_err(|error| format!("admit linked worktree {}: {error}", root.display()))?;
        match load_exact_full_verifier_evidence(&paths).map_err(|error| {
            format!(
                "load linked-worktree exact receipt {}: {error}",
                paths.full_verifier_evidence().display(),
            )
        })? {
            ExactEvidenceDocument::Loaded { evidence, .. } => {
                let semantic = semantic::SemanticEvidence::decode(evidence.semantic.as_bytes())
                    .map_err(|error| {
                        format!(
                            "validate linked-worktree semantic receipt {}: {error}",
                            paths.full_verifier_evidence().display(),
                        )
                    })?;
                protected.insert(semantic.source().to_string());
            }
            ExactEvidenceDocument::Missing => {}
            ExactEvidenceDocument::Unavailable(detail) => {
                return Err(format!(
                    "validate linked-worktree exact receipt {}: {detail}",
                    paths.full_verifier_evidence().display(),
                ));
            }
        }
        protect_open_merge_parents(&linked, &mut protected)?;
    }
    Ok(protected)
}

fn linked_worktrees(repository: &Repository) -> Result<Vec<PathBuf>, String> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repository.worktree_root())
        .args(["worktree", "list", "--porcelain", "-z"])
        .output()
        .map_err(|error| format!("enumerate linked worktrees: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "enumerate linked worktrees exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    parse_linked_worktrees(&output.stdout)
}

fn parse_linked_worktrees(document: &[u8]) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut head: Option<String> = None;
    for field in document.split(|byte| *byte == 0) {
        if field.is_empty() {
            if let Some(record) = path.take() {
                let record_head = head.take().ok_or_else(|| {
                    format!("worktree list record has no HEAD: {}", record.display())
                })?;
                if record_head.bytes().any(|byte| byte != b'0') {
                    if !canonical_object_id(&record_head) {
                        return Err(format!(
                            "worktree list has a noncanonical HEAD for {}: {record_head:?}",
                            record.display(),
                        ));
                    }
                    roots.push(record);
                }
            }
            continue;
        }
        let field = std::str::from_utf8(field)
            .map_err(|error| format!("worktree list contains non-UTF-8 state: {error}"))?;
        if let Some(worktree) = field.strip_prefix("worktree ") {
            if path.is_some() || worktree.is_empty() {
                return Err("worktree list contains a malformed worktree record".to_owned());
            }
            path = Some(PathBuf::from(worktree));
        } else if let Some(value) = field.strip_prefix("HEAD ") {
            if path.is_none() || head.replace(value.to_owned()).is_some() {
                return Err("worktree list contains a malformed HEAD record".to_owned());
            }
        } else if path.is_none() {
            return Err(format!(
                "worktree list record does not begin with a worktree path: {field:?}",
            ));
        }
    }
    if path.is_some() {
        return Err("worktree list final record lacks a NUL terminator".to_owned());
    }
    if roots.is_empty() {
        return Err("worktree list returned no owning worktree".to_owned());
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn protect_open_merge_parents(
    repository: &Repository,
    protected: &mut BTreeSet<String>,
) -> Result<(), String> {
    let merge_head_path = repository.git_dir().join("MERGE_HEAD");
    let merge_head = match fs::read_to_string(&merge_head_path) {
        Ok(merge_head) => merge_head,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "read open merge state {}: {error}",
                merge_head_path.display(),
            ));
        }
    };
    let heads = merge_head.lines().collect::<Vec<_>>();
    if heads.len() != 1 || !canonical_object_id(heads[0]) {
        return Err(format!(
            "open merge {} does not name exactly one canonical parent",
            merge_head_path.display(),
        ));
    }
    let head = canonical_revision(repository, "HEAD")?;
    for parent in [head.as_str(), heads[0]] {
        let prepared = prepare_strict_at(
            repository.worktree_root(),
            Selector::revision(parent.to_owned()),
        )
        .map_err(|error| {
            format!(
                "resolve open merge parent {parent} in {}: {}",
                repository.worktree_root().display(),
                error.detail(),
            )
        })?;
        protected.insert(prepared.source().to_string());
    }
    Ok(())
}

fn canonical_revision(repository: &Repository, revision: &str) -> Result<String, String> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repository.worktree_root())
        .args(["rev-parse", "--verify", revision])
        .output()
        .map_err(|error| format!("resolve {revision}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "resolve {revision} in {} exited {:?}: {}",
            repository.worktree_root().display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let revision = std::str::from_utf8(&output.stdout)
        .map_err(|error| format!("resolved {revision} is not UTF-8: {error}"))?
        .trim();
    if !canonical_object_id(revision) {
        return Err(format!("resolved revision is not canonical: {revision:?}"));
    }
    Ok(revision.to_owned())
}

fn canonical_object_id(object: &str) -> bool {
    matches!(object.len(), 40 | 64)
        && object
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_skips_the_retired_zero_head_slot_and_orders_roots() {
        let zero = "0".repeat(40);
        let first = "1".repeat(40);
        let second = "2".repeat(40);
        let document = format!(
            concat!(
                "worktree /git/common\0HEAD {}\0branch refs/heads/main\0\0",
                "worktree /work/z\0HEAD {}\0branch refs/heads/z\0\0",
                "worktree /work/a\0HEAD {}\0branch refs/heads/a\0\0",
            ),
            zero, second, first,
        );
        assert_eq!(
            parse_linked_worktrees(document.as_bytes()).expect("parse worktree inventory"),
            [PathBuf::from("/work/a"), PathBuf::from("/work/z")],
        );
    }

    #[test]
    fn inventory_refuses_missing_or_noncanonical_heads() {
        assert!(parse_linked_worktrees(b"worktree /work/a\0\0").is_err());
        assert!(parse_linked_worktrees(b"worktree /work/a\0HEAD not-an-id\0\0").is_err());
        assert!(
            parse_linked_worktrees(b"HEAD 1111111111111111111111111111111111111111\0\0").is_err()
        );
    }
}
