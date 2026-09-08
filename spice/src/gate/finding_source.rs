//! Typed Git authority retained with focused gate findings.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::process::Command;

use ::gate::Selector;
use wire::intent::{GateFindingMainline, GateFindingSource, GateFindingSourceKind};
use wire::{Digest, Domain};

use super::{GIT, RunError, git_os};
use crate::repository::Repository;

const MAINLINE_SYMBOLIC_REF: &str = "refs/remotes/origin/HEAD";

/// Repository and immutable Git selection captured around one focused report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateFindingAnchor {
    repository: Digest,
    selection: Digest,
    anchor: String,
    kind: GateFindingSourceKind,
    mainline: GateFindingMainline,
}

impl GateFindingAnchor {
    /// Bind the anchor to the selected tree retained by the focused report.
    #[must_use]
    pub fn bind(self, tree: Digest) -> GateFindingSource {
        GateFindingSource {
            repository: self.repository,
            selection: self.selection,
            tree,
            anchor: self.anchor,
            kind: self.kind,
            mainline: Some(self.mainline),
        }
    }
}

/// Capture the repository, current selection, and commit anchor for a report.
///
/// Callers capture this immediately before and after the focused run and only
/// publish the report when both values match. The selected tree digest is then
/// bound with [`GateFindingAnchor::bind`].
///
/// # Errors
///
/// Returns a typed refusal when the Git source or index cannot be read as one
/// stable canonical selection.
pub fn capture_gate_finding_anchor(
    repository: &Repository,
    selector: &Selector,
) -> Result<GateFindingAnchor, RunError> {
    let repository_authority = Digest::compute(
        Domain::GateRepositoryAuthority,
        repository.common_dir().as_os_str().as_bytes(),
    );
    let (kind, anchor, selected) = match selector {
        Selector::Staged => {
            let anchor = canonical_commit(repository, "HEAD")?;
            let index_path = repository.git_dir().join("index");
            let index = fs::read(&index_path).map_err(|error| {
                RunError::fault(
                    "gate::finding-index-unavailable",
                    format!("{}: {error}", index_path.display()),
                )
            })?;
            (GateFindingSourceKind::StagedIndex, anchor, index)
        }
        Selector::Revision(revision) => (
            GateFindingSourceKind::Revision,
            canonical_commit(repository, revision)?,
            Vec::new(),
        ),
    };
    let mut selection_evidence = match kind {
        GateFindingSourceKind::StagedIndex => b"staged-index\0".to_vec(),
        GateFindingSourceKind::Revision => b"revision\0".to_vec(),
    };
    selection_evidence.extend_from_slice(anchor.as_bytes());
    selection_evidence.push(0);
    selection_evidence.extend_from_slice(&selected);
    let mainline = capture_mainline(repository, &anchor)?;
    Ok(GateFindingAnchor {
        repository: repository_authority,
        selection: Digest::compute(Domain::GateSelection, &selection_evidence),
        anchor,
        kind,
        mainline,
    })
}

/// Verify that a reported source is the resident repository's live selection.
///
/// # Errors
///
/// Refuses a repository mismatch, moved index, unresolved revision, or source
/// envelope whose canonical anchor does not match the current Git authority.
pub fn validate_gate_finding_source(
    repository: &Repository,
    source: &GateFindingSource,
) -> Result<(), RunError> {
    if source.mainline.is_none() {
        return Err(RunError::refusal(
            "gate::finding-mainline-missing",
            "focused finding source requires gate-finding-mainline-v1 lineage",
        ));
    }
    let selector = match source.kind {
        GateFindingSourceKind::StagedIndex => Selector::staged(),
        GateFindingSourceKind::Revision => Selector::revision(source.anchor.clone()),
    };
    let current = capture_gate_finding_anchor(repository, &selector)?;
    validate_gate_finding_source_against(source, &current)
}

fn validate_gate_finding_source_against(
    source: &GateFindingSource,
    current: &GateFindingAnchor,
) -> Result<(), RunError> {
    if current.repository != source.repository {
        return Err(RunError::refusal(
            "gate::finding-repository-mismatch",
            "focused finding evidence belongs to a different Git-common repository authority",
        ));
    }
    if current.selection != source.selection
        || current.anchor != source.anchor
        || current.kind != source.kind
        || source.mainline.as_ref().is_some_and(|mainline| {
            mainline.reference != current.mainline.reference
                || mainline.merge_base != current.mainline.merge_base
        })
    {
        return Err(RunError::refusal(
            "gate::finding-source-moved",
            "focused finding selection no longer matches its canonical Git anchor",
        ));
    }
    Ok(())
}

/// Require a candidate finding source to move forward from retained evidence.
///
/// `recovery` additionally requires a different selected tree; index metadata
/// movement or an empty descendant commit cannot make unchanged source prove a
/// repair or close its task.
///
/// # Errors
///
/// Refuses repository crossing, unrelated or older history, staged-to-revision
/// downgrade at one anchor, and recovery without source movement.
pub fn require_gate_finding_source_forward(
    repository: &Repository,
    prior: &GateFindingSource,
    candidate: &GateFindingSource,
    recovery: bool,
) -> Result<(), RunError> {
    validate_gate_finding_source(repository, candidate)?;
    require_forward_envelope(prior, candidate)?;
    if !commit_is_ancestor(repository, &prior.anchor, &candidate.anchor)? {
        require_orphaned_mainline_recovery(repository, prior, candidate, recovery)?;
    }
    require_recovery_movement(prior, candidate, recovery)
}

fn require_orphaned_mainline_recovery(
    repository: &Repository,
    prior: &GateFindingSource,
    candidate: &GateFindingSource,
    recovery: bool,
) -> Result<(), RunError> {
    if !recovery {
        return Err(source_regressed(&prior.anchor, &candidate.anchor));
    }
    let prior_mainline = prior.mainline.as_ref().ok_or_else(|| {
        RunError::refusal(
            "gate::finding-mainline-migration-required",
            "retained finding source predates gate-finding-mainline-v1",
        )
    })?;
    let candidate_mainline = candidate.mainline.as_ref().ok_or_else(|| {
        RunError::refusal(
            "gate::finding-mainline-missing",
            "candidate finding source carries no mainline lineage",
        )
    })?;
    validate_mainline(repository, &prior.anchor, prior_mainline)?;
    if prior_mainline.reference != candidate_mainline.reference {
        return Err(RunError::refusal(
            "gate::finding-mainline-moved",
            format!(
                "mainline reference moved from {:?} to {:?}",
                prior_mainline.reference, candidate_mainline.reference
            ),
        ));
    }
    if candidate.kind != GateFindingSourceKind::Revision
        || candidate.anchor != canonical_commit(repository, &candidate_mainline.reference)?
    {
        return Err(RunError::refusal(
            "gate::finding-mainline-required",
            "orphaned finding recovery requires an exact report at the current mainline revision",
        ));
    }
    require_commit_ancestor(repository, &prior_mainline.merge_base, &candidate.anchor)?;
    Ok(())
}

fn require_forward_envelope(
    prior: &GateFindingSource,
    candidate: &GateFindingSource,
) -> Result<(), RunError> {
    if prior.repository != candidate.repository {
        return Err(RunError::refusal(
            "gate::finding-repository-mismatch",
            "gate finding repair evidence cannot cross Git-common repository authority",
        ));
    }
    if prior.anchor == candidate.anchor
        && prior.kind == GateFindingSourceKind::StagedIndex
        && candidate.kind == GateFindingSourceKind::Revision
    {
        return Err(RunError::refusal(
            "gate::finding-source-regressed",
            "a committed revision at the same anchor cannot supersede staged finding evidence",
        ));
    }
    Ok(())
}

fn require_recovery_movement(
    prior: &GateFindingSource,
    candidate: &GateFindingSource,
    recovery: bool,
) -> Result<(), RunError> {
    if recovery && prior.tree == candidate.tree {
        return Err(RunError::refusal(
            "gate::finding-recovery-unproven",
            "the clean report selects the same tree that produced the standing finding",
        ));
    }
    Ok(())
}

fn canonical_commit(repository: &Repository, revision: &str) -> Result<String, RunError> {
    if revision.is_empty() || revision.starts_with('-') || revision.contains(['\0', '\r', '\n']) {
        return Err(RunError::refusal(
            "gate::finding-anchor-refused",
            format!("invalid Git revision anchor {revision:?}"),
        ));
    }
    let peeled = format!("{revision}^{{commit}}");
    let arguments = ["rev-parse", "--verify", "--end-of-options", peeled.as_str()]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let output = git_os(repository.worktree_root(), &arguments)?;
    let commit = std::str::from_utf8(&output)
        .map_err(|error| RunError::refusal("gate::finding-anchor-refused", error))?
        .trim();
    if !matches!(commit.len(), 40 | 64)
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RunError::refusal(
            "gate::finding-anchor-refused",
            format!("Git returned non-canonical commit identity {commit:?}"),
        ));
    }
    Ok(commit.to_owned())
}

fn capture_mainline(
    repository: &Repository,
    measured_anchor: &str,
) -> Result<GateFindingMainline, RunError> {
    let reference = git_text(
        repository,
        &["symbolic-ref", "--quiet", MAINLINE_SYMBOLIC_REF],
        "mainline reference",
    )?;
    let branch = reference
        .strip_prefix("refs/remotes/origin/")
        .filter(|branch| !branch.is_empty() && *branch != "HEAD")
        .ok_or_else(|| {
            RunError::refusal(
                "gate::finding-mainline-refused",
                format!("origin HEAD resolved to non-mainline reference {reference:?}"),
            )
        })?;
    if branch.contains(['\0', '\r', '\n']) {
        return Err(RunError::refusal(
            "gate::finding-mainline-refused",
            format!("origin HEAD resolved to invalid reference {reference:?}"),
        ));
    }
    let head = canonical_commit(repository, &reference)?;
    let merge_base = unique_merge_base(repository, measured_anchor, &head)?;
    Ok(GateFindingMainline {
        reference,
        merge_base,
    })
}

fn validate_mainline(
    repository: &Repository,
    measured_anchor: &str,
    mainline: &GateFindingMainline,
) -> Result<(), RunError> {
    let expected = mainline
        .reference
        .strip_prefix("refs/remotes/origin/")
        .filter(|branch| !branch.is_empty() && *branch != "HEAD")
        .ok_or_else(|| {
            RunError::refusal(
                "gate::finding-mainline-refused",
                format!(
                    "retained mainline reference is invalid: {:?}",
                    mainline.reference
                ),
            )
        })?;
    if expected.contains(['\0', '\r', '\n']) {
        return Err(RunError::refusal(
            "gate::finding-mainline-refused",
            format!(
                "retained mainline reference is invalid: {:?}",
                mainline.reference
            ),
        ));
    }
    let head = canonical_commit(repository, &mainline.reference)?;
    let merge_base = unique_merge_base(repository, measured_anchor, &head)?;
    if merge_base != mainline.merge_base {
        return Err(RunError::refusal(
            "gate::finding-mainline-moved",
            format!(
                "retained merge base {} does not match measured mainline base {merge_base}",
                mainline.merge_base
            ),
        ));
    }
    Ok(())
}

fn unique_merge_base(repository: &Repository, left: &str, right: &str) -> Result<String, RunError> {
    let bases = git_text(
        repository,
        &["merge-base", "--all", left, right],
        "mainline merge base",
    )?;
    let bases = bases.lines().collect::<Vec<_>>();
    let [base] = bases.as_slice() else {
        return Err(RunError::refusal(
            "gate::finding-mainline-ambiguous",
            format!(
                "measured anchor {left} and mainline {right} have {} merge bases",
                bases.len()
            ),
        ));
    };
    canonical_commit(repository, base)
}

fn git_text(repository: &Repository, arguments: &[&str], label: &str) -> Result<String, RunError> {
    let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    let output = git_os(repository.worktree_root(), &arguments)?;
    let output = std::str::from_utf8(&output)
        .map_err(|error| RunError::refusal("gate::finding-git-non-utf8", error))?;
    let output = output.trim();
    if output.contains('\0') {
        return Err(RunError::refusal(
            "gate::finding-git-invalid",
            format!("{label} contains NUL"),
        ));
    }
    Ok(output.to_owned())
}

fn require_commit_ancestor(
    repository: &Repository,
    prior: &str,
    candidate: &str,
) -> Result<(), RunError> {
    if commit_is_ancestor(repository, prior, candidate)? {
        Ok(())
    } else {
        Err(source_regressed(prior, candidate))
    }
}

fn commit_is_ancestor(
    repository: &Repository,
    prior: &str,
    candidate: &str,
) -> Result<bool, RunError> {
    let prior = canonical_commit(repository, prior)?;
    let candidate = canonical_commit(repository, candidate)?;
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repository.worktree_root())
        .args(["merge-base", "--is-ancestor", &prior, &candidate])
        .output()
        .map_err(|error| RunError::fault("gate::git-unavailable", error))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        code => Err(RunError::fault(
            "gate::finding-lineage-unavailable",
            format!(
                "{GIT} merge-base --is-ancestor exited {code:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )),
    }
}

fn source_regressed(prior: &str, candidate: &str) -> RunError {
    RunError::refusal(
        "gate::finding-source-regressed",
        format!("candidate anchor {candidate} is not a descendant of retained anchor {prior}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mainline(root: &std::path::Path, head: &str) {
        let remote = root.join(".git/refs/remotes/origin");
        fs::create_dir_all(&remote).expect("create fixture mainline refs");
        fs::write(remote.join("main"), format!("{head}\n")).expect("write fixture mainline");
        fs::write(remote.join("HEAD"), "ref: refs/remotes/origin/main\n")
            .expect("write fixture origin HEAD");
    }

    fn digest(domain: Domain, label: &str) -> Digest {
        Digest::compute(domain, label.as_bytes())
    }

    fn source(
        repository: &str,
        selection: &str,
        tree: &str,
        anchor: &str,
        kind: GateFindingSourceKind,
    ) -> GateFindingSource {
        GateFindingSource {
            repository: digest(Domain::GateRepositoryAuthority, repository),
            selection: digest(Domain::GateSelection, selection),
            tree: digest(Domain::GateTree, tree),
            anchor: anchor.to_owned(),
            kind,
            mainline: None,
        }
    }

    fn captured(source: &GateFindingSource) -> GateFindingAnchor {
        GateFindingAnchor {
            repository: source.repository.clone(),
            selection: source.selection.clone(),
            anchor: source.anchor.clone(),
            kind: source.kind,
            mainline: GateFindingMainline {
                reference: "refs/remotes/origin/main".to_owned(),
                merge_base: "merge-base".to_owned(),
            },
        }
    }

    #[test]
    fn source_validation_refuses_repository_and_selection_movement() {
        let reported = source(
            "repository",
            "selection",
            "tree",
            "anchor",
            GateFindingSourceKind::StagedIndex,
        );
        let mut other_repository = captured(&reported);
        other_repository.repository = digest(Domain::GateRepositoryAuthority, "other");
        assert_eq!(
            validate_gate_finding_source_against(&reported, &other_repository)
                .unwrap_err()
                .reason(),
            "gate::finding-repository-mismatch"
        );

        let mut moved = captured(&reported);
        moved.selection = digest(Domain::GateSelection, "moved");
        assert_eq!(
            validate_gate_finding_source_against(&reported, &moved)
                .unwrap_err()
                .reason(),
            "gate::finding-source-moved"
        );
    }

    #[test]
    fn forward_envelope_refuses_repository_crossing_and_kind_regression() {
        let prior = source(
            "repository",
            "staged",
            "tree-a",
            "anchor",
            GateFindingSourceKind::StagedIndex,
        );
        let crossed = source(
            "other",
            "revision",
            "tree-b",
            "descendant",
            GateFindingSourceKind::Revision,
        );
        assert_eq!(
            require_forward_envelope(&prior, &crossed)
                .unwrap_err()
                .reason(),
            "gate::finding-repository-mismatch"
        );

        let regressed = source(
            "repository",
            "revision",
            "tree-b",
            "anchor",
            GateFindingSourceKind::Revision,
        );
        assert_eq!(
            require_forward_envelope(&prior, &regressed)
                .unwrap_err()
                .reason(),
            "gate::finding-source-regressed"
        );
    }

    #[test]
    fn recovery_requires_selected_tree_movement() {
        let prior = source(
            "repository",
            "before",
            "same-tree",
            "before",
            GateFindingSourceKind::Revision,
        );
        let candidate = source(
            "repository",
            "after",
            "same-tree",
            "after",
            GateFindingSourceKind::Revision,
        );
        assert_eq!(
            require_recovery_movement(&prior, &candidate, true)
                .unwrap_err()
                .reason(),
            "gate::finding-recovery-unproven"
        );
        require_recovery_movement(&prior, &candidate, false)
            .expect("refresh does not require tree movement");
    }

    #[test]
    fn orphaned_revision_recovers_only_from_current_mainline() {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-finding-mainline-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("create fixture repository");
        crate::test_repository::initialize(&root);
        fs::write(root.join("source.txt"), "base\n").expect("write base source");
        crate::test_repository::stage(&root, &["source.txt"]);
        let base = crate::test_repository::commit(&root, "base");
        write_mainline(&root, &base);

        fs::write(root.join("source.txt"), "finding\n").expect("write finding source");
        crate::test_repository::stage(&root, &["source.txt"]);
        let finding = crate::test_repository::commit(&root, "temporary finding");
        let repository = Repository::discover(&root).expect("discover fixture repository");
        let prior = capture_gate_finding_anchor(&repository, &Selector::revision(finding.clone()))
            .expect("capture finding source")
            .bind(digest(Domain::GateTree, "finding-tree"));

        fs::write(root.join(".git/refs/heads/main"), format!("{base}\n"))
            .expect("abandon finding revision");
        fs::write(root.join("source.txt"), "mainline\n").expect("write mainline source");
        crate::test_repository::stage(&root, &["source.txt"]);
        let mainline = crate::test_repository::commit(&root, "mainline repair");
        write_mainline(&root, &mainline);
        let clean = capture_gate_finding_anchor(&repository, &Selector::revision(mainline.clone()))
            .expect("capture current mainline")
            .bind(digest(Domain::GateTree, "clean-tree"));

        require_gate_finding_source_forward(&repository, &prior, &clean, true)
            .expect("current mainline reconciles abandoned finding");

        let mut legacy = prior;
        legacy.mainline = None;
        assert_eq!(
            require_gate_finding_source_forward(&repository, &legacy, &clean, true)
                .unwrap_err()
                .reason(),
            "gate::finding-mainline-migration-required"
        );
        fs::remove_dir_all(root).expect("remove finding source fixture");
    }
}
