//! Worktree snapshots are commit objects, never references or admitted lane commits.

use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

use wire::intent::TransplantConflict;

/// Original index bytes are kept separately only until release admission settles.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rollback {
    pub(super) commit: String,
    index: Option<PathBuf>,
}

#[derive(Debug)]
pub(super) struct Captured {
    rollback: Rollback,
    /// The worktree's transaction marker, resolved when the snapshot was taken.
    marker: Option<PathBuf>,
}

impl Captured {
    pub(super) fn commit(&self) -> &str {
        &self.rollback.commit
    }

    pub(super) fn rollback(&self) -> &Rollback {
        &self.rollback
    }

    pub(super) fn restore(&self, root: Worktree<'_>) -> Result<(), Error> {
        restore(root, &self.rollback)
    }
}

impl Drop for Captured {
    fn drop(&mut self) {
        // A failed recovery still owns its backup through the durable marker.
        if self.marker.as_ref().is_some_and(|path| path.exists()) {
            return;
        }
        if let Some(index) = &self.rollback.index {
            let _ = fs::remove_file(index);
        }
    }
}

pub(super) fn require_releasable(root: Worktree<'_>) -> Result<(), Error> {
    require_no_open_merge(root, "task release")?;
    require_no_replay(root)?;
    let unresolved = required_read(
        root,
        &[
            "--no-optional-locks",
            "diff",
            "--name-only",
            "--diff-filter=U",
        ],
        "inspect unresolved snapshot paths",
    )?;
    if unresolved.is_empty() {
        Ok(())
    } else {
        Err(Error::refusal(
            "git::worktree-dirty",
            format!("task release has unresolved paths:\n{unresolved}"),
            "resolve and stage every conflicted path before releasing the task",
        ))
    }
}

fn require_no_replay(root: Worktree<'_>) -> Result<(), Error> {
    let pick = try_read(
        root,
        &["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"],
    )?;
    if pick.is_some() || git_state_path(root, "sequencer")?.exists() {
        Err(Error::refusal(
            "git::parked-replay-paused",
            "parked commits are still in Git's cherry-pick sequence",
            "resolve the paused replay and run git cherry-pick --continue, --skip, or --abort first",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn capture(root: Worktree<'_>, head: &str) -> Result<Option<Captured>, Error> {
    if required_read(
        root,
        &["--no-optional-locks", "status", "--porcelain"],
        "inspect release worktree",
    )?
    .is_empty()
    {
        return Ok(None);
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "spice-task-snapshot-index-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    );
    let build = git_state_path(root, &name)?;
    let saved = build.with_extension("original");
    let index = git_state_path(root, "index")?;
    let original = match fs::copy(&index, &saved) {
        Ok(_) => Some(saved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(filesystem_fault("save release index", &saved, error)),
    };
    let created = (|| {
        if let Some(original) = &original {
            fs::copy(original, &build)
                .map_err(|error| filesystem_fault("copy snapshot index", &build, error))?;
        }
        let path = transaction::git_index_spelling(&build, "git::snapshot-index-path")?;
        let environment = [("GIT_INDEX_FILE", path.as_str())];
        if original.is_none() {
            checked(
                run_with_environment(root, &["read-tree", head], &environment)?,
                "initialize snapshot index",
            )?;
        }
        checked(
            run_with_environment(root, &["add", "-A", "--", "."], &environment)?,
            "capture dirty worktree",
        )?;
        let tree = checked(
            run_with_environment(root, &["write-tree"], &environment)?,
            "write snapshot tree",
        )?;
        checked(
            run(
                root,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    head,
                    "-m",
                    "Spice parked worktree snapshot (uncommitted work)",
                ],
            )?,
            "write dangling worktree snapshot",
        )
    })();
    let _ = fs::remove_file(&build);
    match created {
        Ok(commit) => Ok(Some(Captured {
            rollback: Rollback {
                commit,
                index: original,
            },
            marker: transaction_path(root).ok(),
        })),
        Err(error) => {
            if let Some(original) = original {
                let _ = fs::remove_file(original);
            }
            Err(error)
        }
    }
}

fn checked(output: GitOutput, action: &str) -> Result<String, Error> {
    if output.success() {
        Ok(output.stdout.trim().to_owned())
    } else {
        Err(Error::fault("git::snapshot-failed", output.detail(action)))
    }
}

pub(super) fn restore(root: Worktree<'_>, rollback: &Rollback) -> Result<(), Error> {
    checked(
        run(root, &["read-tree", "--reset", "-u", &rollback.commit])?,
        "restore dirty release worktree",
    )?;
    let index = git_state_path(root, "index")?;
    if let Some(saved) = &rollback.index {
        fs::copy(saved, &index)
            .map_err(|error| filesystem_fault("restore original release index", &index, error))?;
    } else {
        fs::remove_file(&index)
            .map_err(|error| filesystem_fault("restore absent release index", &index, error))?;
    }
    Ok(())
}

/// Apply only the snapshot's delta; neither a successful pick nor a conflict moves HEAD.
pub(super) fn apply(root: Worktree<'_>, commit: &str) -> Result<Option<TransplantConflict>, Error> {
    transplant::require_object(root, commit)?;
    let picked = run(root, &["cherry-pick", "--no-commit", commit])?;
    if picked.success() {
        // A successful no-commit pick leaves MERGE_MSG and AUTO_MERGE behind,
        // including when the restored delta is already present. Let Git clear
        // that synthetic operation state without moving HEAD or discarding the
        // restored work; the claimant stages it through the ordinary gate.
        checked(
            run(root, &["reset", "--mixed", "--quiet", "HEAD"])?,
            "finish restoring uncommitted task snapshot",
        )?;
        return Ok(None);
    }
    let paths = required_read(
        root,
        &["diff", "--name-only", "--diff-filter=U"],
        "read snapshot conflicts",
    )?;
    if paths.is_empty() {
        return Err(Error::fault(
            "git::snapshot-restore-failed",
            picked.detail("restore uncommitted task snapshot"),
        ));
    }
    Ok(Some(TransplantConflict {
        commit: commit.to_owned(),
        paths: paths.lines().map(str::to_owned).collect(),
        remaining: Vec::new(),
        remedy: "resolve and git add the snapshot conflicts, then commit through the ordinary pre-commit gate; the snapshot itself must remain uncommitted".into(),
    }))
}

/// A real-commit conflict precedes the dirty delta; retain it until that replay settles.
pub(super) fn defer(root: Worktree<'_>, commit: &str) -> Result<(), Error> {
    write_pending(
        &transaction_path(root)?,
        &Pending::Snapshot {
            commit: commit.to_owned(),
        },
    )
}

pub(super) fn resume(root: Worktree<'_>, commit: &str, marker: &Path) -> Result<(), Error> {
    require_no_replay(root)?;
    require_no_open_merge(root, "restore task snapshot")?;
    require_clean_or(
        root,
        "restore task snapshot after parked replay",
        &format!(
            concat!(
                "commit the reported paths through the pre-commit gate so pending ",
                "snapshot {commit} can restore on top of them, then retry",
            ),
            commit = commit
        ),
    )?;
    let conflict = apply(root, commit)?;
    remove_marker(marker)?;
    if let Some(conflict) = conflict {
        Err(Error::refusal(
            "git::snapshot-conflict",
            format!(
                "snapshot {} has unresolved paths: {}",
                commit,
                conflict.paths.join(", ")
            ),
            conflict.remedy,
        ))
    } else {
        Ok(())
    }
}
