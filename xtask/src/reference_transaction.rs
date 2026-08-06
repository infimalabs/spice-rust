//! Native guard for Git reference transactions on the current branch.
//!
//! # The refusal can arrive after the worktree is written
//!
//! This guard cannot refuse a destructive rewind before that rewind touches
//! the tree, and no care taken in this module changes it. `reference-transaction`
//! is a *reference* hook: githooks(5) describes it as invoked by commands that
//! perform reference updates, defines its `prepared` state as the point where
//! "all reference updates have been queued to the transaction and references
//! were locked on disk", and scopes an abort to references alone. Git publishes
//! no `pre-reset` hook, so there is no earlier surface for this guard to
//! occupy. A `reset --hard` writes the index and worktree first and only then
//! opens the reference transaction this hook is able to veto.
//!
//! Verified against git 2.50.1 rather than recalled: with this guard installed,
//! a `reset --hard` onto an upstream-merged parent is refused, and leaves HEAD
//! unmoved while the index and worktree hold the abandoned rewind's target
//! content. The branch survives; the tree does not. That asymmetry is why
//! every error this hook can return from the `prepared` state accounts for the
//! tree rather than merely reporting that it refused. Whenever a destructive
//! rewind may have written it, the named recovery is `git reset --hard HEAD`,
//! which this guard admits because such an update leaves the reference where
//! it already is, so
//! `abandoned_upstream_commits` finds nothing to protect.
//!
//! That damage is not what every refused command leaves behind, so the refusal
//! reads the state rather than assuming the worst of it. A refused
//! `commit --amend` never wrote the working tree at all, and telling its
//! operator that their files were overwritten would send them to a destructive
//! recovery they do not need. `WorktreeDamage` names which of the two happened
//! and the refusal carries the matching account; the discriminator and its
//! bounds are recorded on `worktree_damage`.
//!
//! Refusing any earlier would mean intercepting the rewind above Git, in a
//! spice-side wrapper for destructive commands. That is a larger design
//! question and is deliberately outside this module. It is recorded here so
//! the next reader of this guard can tell why the refusal arrives when it does
//! without re-deriving Git's hook ordering to reach the same answer.
//!
//! # This guard runs inside a lock every worktree shares
//!
//! `prepared` means the references "were locked on disk", so whatever this hook
//! does, it does while Git holds those locks. One of them is not per-worktree:
//! deleting a reference takes `packed-refs.lock` in the *common* directory, which
//! every linked worktree shares, and an ordinary commit ends with a deletion
//! transaction that clears `AUTO_MERGE`, `CHERRY_PICK_HEAD` and `REVERT_HEAD`.
//! Those pseudo-references are normally absent, and deleting an absent reference
//! takes the shared lock anyway.
//!
//! Recorded from a real commit rather than reasoned about, by a hook that logged
//! its state alongside whether the common lock existed: the queue naming
//! `refs/heads/main` was handed over with the lock **free**, and the only queue
//! ever handed over with the lock **held** named `AUTO_MERGE` alone. So the
//! transaction this guard exists to inspect never holds the shared lock, and the
//! transaction that holds it can never contain anything this guard may refuse.
//!
//! That is why [`apply`] reads its own input before it reads Git. Resolving
//! `HEAD` and the publication baseline costs up to five subprocesses, and for a
//! queue naming no branch every one of them was spent inside the shared lock to
//! reach a conclusion the queue already determined. A queue that names no
//! `refs/heads/` reference is answered from the queue alone, launching nothing.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PREPARED: &str = "prepared";
const PROTECTED_COMMIT_LIMIT: usize = 5;
const ASCII_ZERO: u8 = 0x30;
const REMOTE_PREFIX: &str = "refs/remotes/";
const LOCAL_PREFIX: &str = "refs/heads/";
const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";
const OPERATIONAL_RECOVERY: &str = concat!(
    ". This error aborts the reference transaction, so HEAD is not moved, but ",
    "this hook runs after the command's tree operations: the index and worktree ",
    "may already have changed. After correcting this operational failure, ",
    "inspect them and, if this was a destructive rewind, restore the kept ",
    "commit with `git reset --hard HEAD`."
);

/// Refuse a queued update that would abandon published history on this branch.
///
/// The order of the checks is a contract, not a detail. Cheapest first, and no
/// subprocess until the queue proves one is needed, because Git calls this while
/// holding locks that reach outside this worktree. See the module header.
///
/// # Errors
///
/// Returns [`ReferenceTransactionError::ProtectedHistory`] for a refused update,
/// a malformed queue, or a failure to read Git.
pub fn apply(
    repository: &Path,
    state: &OsStr,
    input: &str,
) -> Result<(), ReferenceTransactionError> {
    if state != OsStr::new(PREPARED) {
        return Ok(());
    }

    let updates = parse_updates(input)?;
    if !updates
        .iter()
        .any(|update| update.reference.starts_with(LOCAL_PREFIX))
    {
        return Ok(());
    }

    let Some(current_ref) = git_read(repository, &["symbolic-ref", "--quiet", "HEAD"])? else {
        return Ok(());
    };
    let Some(upstream) = publication_baseline(repository)? else {
        return Ok(());
    };

    for update in updates {
        if update.reference != current_ref {
            continue;
        }
        let protected = abandoned_upstream_commits(repository, &update, &upstream)?;
        if !protected.is_empty() {
            let listed = protected
                .iter()
                .map(|commit| short_oid(repository, commit))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            return Err(ReferenceTransactionError::ProtectedHistory {
                reference: current_ref,
                commits: listed,
                damage: worktree_damage(repository, &update)?,
            });
        }
    }
    Ok(())
}

/// Resolve the commit standing for history this lane has published.
///
/// `@{upstream}` cannot answer this question inside a spice agent. The agent runs
/// under the git shadow environment, which points `branch.<lane>.merge` at
/// `refs/heads/<lane>` in *system* scope precisely so the lane's upstream resolves
/// to itself and `git status` stops moving whenever origin advances. Asking for
/// `@{upstream}` there compares the branch against itself: every merge base is the
/// branch tip, so every commit an update removes looks upstream-merged and the
/// guard refuses the ordinary local rewinds it exists to allow.
///
/// The shadow documents the readable path, and this mirrors it: the lane's
/// integration branch is `git config --get branch.<lane>.merge`, whose
/// command-scope pair the shadow appends last and which therefore reports the true
/// merge, with `origin/HEAD` as the backstop named for lanes that configure no
/// tracking. `branch.<lane>.remote` is never consulted; the shadow poisons it to
/// `.` and it cannot be trusted. A lane with no origin remote, no resolvable
/// integration branch, or an integration branch never published has no upstream
/// history for this guard to protect, and leaves it inert.
fn publication_baseline(repository: &Path) -> Result<Option<String>, ReferenceTransactionError> {
    if !git(repository, &["remote", "get-url", "origin"])?
        .status
        .success()
    {
        return Ok(None);
    }
    let Some(reference) = integration_reference(repository)? else {
        return Ok(None);
    };
    commit_oid(repository, &reference)
}

/// The remote-tracking reference the lane's integration branch publishes to.
fn integration_reference(repository: &Path) -> Result<Option<String>, ReferenceTransactionError> {
    if let Some(branch) = git_read(repository, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
        && let Some(merge) = git_read(
            repository,
            &["config", "--get", &format!("branch.{branch}.merge")],
        )?
        && let Some(name) = merge.strip_prefix(LOCAL_PREFIX)
    {
        return Ok(Some(format!("{REMOTE_PREFIX}origin/{name}")));
    }
    let Some(head) = git_read(repository, &["symbolic-ref", ORIGIN_HEAD])? else {
        return Ok(None);
    };
    Ok(head.starts_with(REMOTE_PREFIX).then_some(head))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RefUpdate<'a> {
    old: &'a str,
    new: &'a str,
    reference: &'a str,
}

fn parse_updates(input: &str) -> Result<Vec<RefUpdate<'_>>, ReferenceTransactionError> {
    let mut updates = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(ReferenceTransactionError::MalformedInput { line: index + 1 });
        }
        updates.push(RefUpdate {
            old: fields[0],
            new: fields[1],
            reference: fields[2],
        });
    }
    Ok(updates)
}

fn abandoned_upstream_commits(
    repository: &Path,
    update: &RefUpdate<'_>,
    upstream: &str,
) -> Result<Vec<String>, ReferenceTransactionError> {
    if is_zero_oid(update.old) {
        return Ok(Vec::new());
    }
    let Some(old) = commit_oid(repository, update.old)? else {
        return Ok(Vec::new());
    };
    let new = if is_zero_oid(update.new) {
        None
    } else {
        let Some(new) = commit_oid(repository, update.new)? else {
            return Ok(Vec::new());
        };
        Some(new)
    };
    if let Some(new) = new.as_deref()
        && is_ancestor(repository, &old, new)?
    {
        return Ok(Vec::new());
    }

    let Some(merge_bases) = git_read(repository, &["merge-base", "--all", &old, upstream])? else {
        return Ok(Vec::new());
    };
    let mut arguments = vec![
        "rev-list".to_owned(),
        format!("--max-count={PROTECTED_COMMIT_LIMIT}"),
    ];
    arguments.extend(merge_bases.lines().map(str::to_owned));
    if let Some(new) = new {
        arguments.push("--not".to_owned());
        arguments.push(new);
    }
    Ok(git_read_owned(repository, &arguments)?
        .map(|output| output.lines().map(str::to_owned).collect())
        .unwrap_or_default())
}

fn is_zero_oid(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte == ASCII_ZERO)
}

fn commit_oid(repository: &Path, value: &str) -> Result<Option<String>, ReferenceTransactionError> {
    if value.starts_with("ref:") {
        return Ok(None);
    }
    git_read_owned(
        repository,
        &[
            "rev-parse".to_owned(),
            "--verify".to_owned(),
            "--quiet".to_owned(),
            format!("{value}^{{commit}}"),
        ],
    )
}

fn short_oid(repository: &Path, value: &str) -> Result<String, ReferenceTransactionError> {
    Ok(git_read(repository, &["rev-parse", "--short", value])?.unwrap_or_else(|| value.to_owned()))
}

fn is_ancestor(
    repository: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, ReferenceTransactionError> {
    Ok(git(
        repository,
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )?
    .status
    .success())
}

/// What the refused command had already done to the working tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorktreeDamage {
    /// The command wrote the working tree before any hook could veto it.
    Rewritten,
    /// The command left the files on disk as they were.
    Untouched,
}

/// Read which damage the refused command left behind.
///
/// The hook is handed a state name, the queued updates and nothing else: git
/// exports no variable naming the command that opened the transaction, which was
/// confirmed by dumping the hook environment for `reset` and for `commit --amend`
/// rather than taken from documentation. So this reads the state the command left
/// instead of trying to recognise the command.
///
/// Two facts together are decisive. The update moves the branch backwards onto an
/// ancestor of its own tip, and the working tree already holds that ancestor's
/// content even though HEAD has not moved yet. Only a command that wrote the
/// working tree itself can leave that pair behind.
///
/// Both halves are load-bearing, measured live against git 2.50.1 at the
/// `prepared` state. Dropping the ancestor test would report a refused
/// `commit --amend` as rewritten, because an amend leaves index and worktree
/// holding its own new tree; the amend is excluded instead because its
/// replacement tip is no ancestor of the tip it replaces. Dropping the content
/// test would report `reset --soft` and `reset --mixed` as rewritten, because
/// both move backwards; they are excluded instead because neither writes the
/// files on disk, so the worktree still disagrees with the rewind target.
///
/// The bound worth naming: a `--soft` or `--mixed` rewind onto a commit whose
/// content the working tree already happens to match is reported as rewritten.
/// The report stays true where it matters, since the files really do hold the
/// target's content and the named recovery really does restore agreement with the
/// commits that were kept.
fn worktree_damage(
    repository: &Path,
    update: &RefUpdate<'_>,
) -> Result<WorktreeDamage, ReferenceTransactionError> {
    if is_zero_oid(update.new) {
        return Ok(WorktreeDamage::Untouched);
    }
    if is_ancestor(repository, update.new, update.old)? && worktree_holds(repository, update.new)? {
        return Ok(WorktreeDamage::Rewritten);
    }
    Ok(WorktreeDamage::Untouched)
}

/// Whether the working tree and index already hold this commit's content.
fn worktree_holds(repository: &Path, commit: &str) -> Result<bool, ReferenceTransactionError> {
    Ok(git(repository, &["diff-index", "--quiet", commit, "--"])?
        .status
        .success())
}

fn git_read(
    repository: &Path,
    arguments: &[&str],
) -> Result<Option<String>, ReferenceTransactionError> {
    git_read_owned(
        repository,
        &arguments
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn git_read_owned(
    repository: &Path,
    arguments: &[String],
) -> Result<Option<String>, ReferenceTransactionError> {
    let output = git(repository, arguments)?;
    if !output.status.success() {
        return Ok(None);
    }
    let output =
        String::from_utf8(output.stdout).map_err(|_| ReferenceTransactionError::NonUtf8Output {
            arguments: arguments.to_vec(),
        })?;
    let output = output.trim().to_owned();
    Ok((!output.is_empty()).then_some(output))
}

fn git<S>(repository: &Path, arguments: &[S]) -> Result<Output, ReferenceTransactionError>
where
    S: AsRef<OsStr>,
{
    let arguments = arguments
        .iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    Command::new("git")
        .args(&arguments)
        .current_dir(repository)
        .output()
        .map_err(|source| ReferenceTransactionError::GitLaunch {
            repository: repository.to_path_buf(),
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            source,
        })
}

/// A native `reference-transaction` refusal.
#[derive(Debug)]
pub enum ReferenceTransactionError {
    MalformedInput {
        line: usize,
    },
    GitLaunch {
        repository: PathBuf,
        arguments: Vec<String>,
        source: io::Error,
    },
    NonUtf8Output {
        arguments: Vec<String>,
    },
    ProtectedHistory {
        reference: String,
        commits: String,
        damage: WorktreeDamage,
    },
}

impl fmt::Display for ReferenceTransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedInput { line } => {
                write!(formatter, "malformed reference-transaction line {line}")?;
                formatter.write_str(OPERATIONAL_RECOVERY)
            }
            Self::GitLaunch {
                repository,
                arguments,
                source,
            } => {
                write!(
                    formatter,
                    "reference-transaction could not run git -C {} {}: {source}",
                    repository.display(),
                    arguments.join(" ")
                )?;
                formatter.write_str(OPERATIONAL_RECOVERY)
            }
            Self::NonUtf8Output { arguments } => {
                write!(
                    formatter,
                    "reference-transaction git output was not UTF-8: git {}",
                    arguments.join(" ")
                )?;
                formatter.write_str(OPERATIONAL_RECOVERY)
            }
            Self::ProtectedHistory {
                reference,
                commits,
                damage,
            } => {
                write!(
                    formatter,
                    concat!(
                        "reference-transaction guard refused to abandon upstream-merged ",
                        "commits on current branch {}: {}. This is expected when a task ",
                        "boundary has advanced origin/upstream; keep those commits and ",
                        "continue with an append-only commit instead of amending or ",
                        "resetting backwards. "
                    ),
                    reference, commits
                )?;
                match damage {
                    WorktreeDamage::Rewritten => formatter.write_str(concat!(
                        "This refusal protected the branch, not the tree: HEAD was not ",
                        "moved, but the rewind had already written the index and worktree ",
                        "before any hook could run, so they now disagree with the commits ",
                        "that were kept. Restore them with `git reset --hard HEAD`, which ",
                        "this guard admits."
                    )),
                    WorktreeDamage::Untouched => formatter.write_str(concat!(
                        "HEAD was not moved and this command did not write the working ",
                        "tree, so the files on disk are the ones you already had and there ",
                        "is nothing here to restore. Do not reset to recover from this ",
                        "refusal; it would discard work this refusal did not touch."
                    )),
                }
            }
        }
    }
}

impl Error for ReferenceTransactionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::GitLaunch { source, .. } => Some(source),
            Self::MalformedInput { .. }
            | Self::NonUtf8Output { .. }
            | Self::ProtectedHistory { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
