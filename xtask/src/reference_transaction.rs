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
//! content. The branch survives; the tree does not. That asymmetry is why the
//! `ProtectedHistory` refusal reports that the queued target and the current
//! index and worktree agree, but it deliberately prescribes no destructive
//! recovery. That observable state is consistent with `reset --hard` having
//! written the tree, but a soft or mixed rewind can present the same state when
//! the index and worktree already matched its target. The hook is told neither
//! command nor intent, so the operator has to inspect the retained state before
//! deciding what, if anything, to restore.
//!
//! The operational errors cannot finish that state inspection. They therefore
//! report only that the index and worktree may have changed, tell the operator
//! to inspect them after correcting the operational cause, and name
//! `git reset --hard HEAD` only for a command the operator establishes was a
//! destructive rewind. That recovery leaves the reference where it already is,
//! so `abandoned_upstream_commits` finds nothing to protect.
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
//! `HEAD`, the publication baseline and the tip the branch holds costs several
//! subprocesses, and for a queue naming no branch every one of them was spent
//! inside the shared lock to reach a conclusion the queue already determined. A
//! queue that names no `refs/heads/` reference is answered from the queue alone,
//! launching nothing.
//!
//! # A successful commit can still report skipped cleanup
//!
//! Git's commit status and its cleanup diagnostic account for different work.
//! Measured on Git 2.50.1, the direct `git commit` can record the requested
//! branch update and exit zero, then exhaust `core.packedRefsTimeout` while
//! deleting an absent pseudo-reference and write an `Unable to create
//! .git/packed-refs.lock` diagnostic to stderr. If the other writer releases
//! the lock within the bound, the same successful commit says nothing about
//! it. A finite bound decides how long Git retries; it cannot guarantee silence.
//!
//! Callers therefore have to read status and both output streams together:
//! zero means the primary commit succeeded, while the diagnostic separately
//! says best-effort cleanup was skipped. A genuine nonzero commit failure keeps
//! its own output. This repository-local hook cannot intercept, suppress, or
//! reinterpret its parent Git process's streams, and it never removes a
//! `packed-refs.lock` owned by another writer.

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
const SELF_REMOTE: &str = ".";
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
    let Some(baseline) = publication_baseline(repository, &current_ref)? else {
        return Ok(());
    };
    // The tip is read here rather than taken from the queue: see `RefUpdate`.
    // Asking the repository is sound because `prepared` is before the write --
    // the reference is locked but still holds its pre-transaction value. No tip
    // at all means the branch is unborn and this queue creates it, which
    // abandons nothing.
    let Some(tip) = commit_oid(repository, &current_ref)? else {
        return Ok(());
    };

    for update in updates {
        if update.reference != current_ref {
            continue;
        }
        let protected = abandoned_upstream_commits(repository, &update, &tip, &baseline.commit)?;
        if !protected.is_empty() {
            let listed = protected
                .iter()
                .map(|commit| short_oid(repository, commit))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            return Err(ReferenceTransactionError::ProtectedHistory {
                reference: current_ref,
                commits: listed,
                baseline: baseline.reference.clone(),
                baseline_commit: short_oid(repository, &baseline.commit)?,
                damage: worktree_damage(repository, &update, &tip)?,
            });
        }
    }
    Ok(())
}

/// The reference this guard treats as published, and the commit it stands at.
///
/// Both travel together because the refusal has to name them together. A
/// baseline reduced to its commit is a number the reader cannot check, and a
/// reference without its commit cannot be compared against what the branch
/// actually holds.
struct Baseline {
    reference: String,
    commit: String,
}

/// Resolve the commit standing for history this lane has published.
///
/// This mirrors the predecessor's `_declared_upstream_ref` in
/// `spice/hooks/refguard.py`: the branch's own pairing names the reference, and
/// a pairing that names none leaves the guard inert. Nothing else is consulted,
/// and no second resolution stands behind the first. Two guards enforcing the
/// same policy have to answer the same reference, and a backstop reached only
/// here would be a policy this side invented.
///
/// A reference the pairing names but that resolves to no commit also leaves the
/// guard inert. The predecessor is stricter and refuses a rewind it cannot
/// judge; porting that refusal is `EFFORT-1kKpJKBf`, and it is a separate
/// behaviour from which reference gets resolved.
///
/// That naming and that resolution are answered together here, where the
/// predecessor answers them either side of reading the branch's own tip. The
/// orders agree on every outcome today -- both end inert -- and this one pays
/// for the tip only once a pairing has resolved. `EFFORT-1kKpJKBf` is where they
/// stop agreeing: the refusal it ports fires on an unresolvable reference and
/// reports the tip, which by then has to be read before this call rather than
/// after it.
fn publication_baseline(
    repository: &Path,
    current_ref: &str,
) -> Result<Option<Baseline>, ReferenceTransactionError> {
    let Some(reference) = declared_upstream_reference(repository, current_ref)? else {
        return Ok(None);
    };
    let Some(commit) = commit_oid(repository, &reference)? else {
        return Ok(None);
    };
    Ok(Some(Baseline { reference, commit }))
}

/// The remote-tracking reference this branch's own pairing names, if it names one.
///
/// Read from local scope and from nowhere else, which is the whole of the rule
/// rather than a precaution. A lane runs under a per-process git shadow that
/// redefines the current branch's upstream across two scopes at once: a
/// *system*-scope `branch.<lane>.merge` pointing at `refs/heads/<lane>`, and a
/// *command*-scope `branch.<lane>.remote` of `.`. Together those aim
/// `@{upstream}` back at this very branch, so every local commit reads as
/// already published and every ordinary amend is refused. Asking for the
/// effective value walks into the same trap by a different door: measured in a
/// lane worktree, `git config --get branch.<lane>.remote` answers `.` while
/// `git config --local --get branch.<lane>.remote` answers `origin`. So
/// `@{upstream}` is never consulted here, and neither is any effective-scope
/// read -- not even as a fallback, because a fallback is exactly where that
/// shadow comes back in.
///
/// Which remote and which branch are whatever the pairing names; `origin` and
/// `main` are not assumed. Only the naming is answered here. Whether the named
/// reference exists is a separate question, and conflating the two would let a
/// pairing nobody can resolve read exactly like a branch that tracks nothing.
fn declared_upstream_reference(
    repository: &Path,
    current_ref: &str,
) -> Result<Option<String>, ReferenceTransactionError> {
    let branch = current_ref
        .strip_prefix(LOCAL_PREFIX)
        .unwrap_or(current_ref);
    let Some(remote) = git_read(
        repository,
        &[
            "config",
            "--local",
            "--get",
            &format!("branch.{branch}.remote"),
        ],
    )?
    else {
        return Ok(None);
    };
    let Some(merge) = git_read(
        repository,
        &[
            "config",
            "--local",
            "--get",
            &format!("branch.{branch}.merge"),
        ],
    )?
    else {
        return Ok(None);
    };
    if remote == SELF_REMOTE {
        return Ok(None);
    }
    let name = merge.strip_prefix(LOCAL_PREFIX).unwrap_or(merge.as_str());
    Ok(Some(format!("{REMOTE_PREFIX}{remote}/{name}")))
}

/// One line of the hook's queue. The reported old value is deliberately absent.
///
/// Git names three fields and [`parse_updates`] still requires all three, but
/// the first is whatever the caller *declared*, not what the reference holds.
/// `git update-ref <ref> <new>` declares nothing, so it arrives as the zero OID
/// and a genuine rewind reads as a branch creation. Carrying the field would
/// only invite reading it again; the tip is read from the reference instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RefUpdate<'a> {
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
            new: fields[1],
            reference: fields[2],
        });
    }
    Ok(updates)
}

fn abandoned_upstream_commits(
    repository: &Path,
    update: &RefUpdate<'_>,
    tip: &str,
    upstream: &str,
) -> Result<Vec<String>, ReferenceTransactionError> {
    let new = if is_zero_oid(update.new) {
        None
    } else {
        let Some(new) = commit_oid(repository, update.new)? else {
            return Ok(Vec::new());
        };
        Some(new)
    };
    if let Some(new) = new.as_deref()
        && is_ancestor(repository, tip, new)?
    {
        return Ok(Vec::new());
    }

    let Some(merge_bases) = git_read(repository, &["merge-base", "--all", tip, upstream])? else {
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
    /// The queued target matches the index and worktree, consistently with a
    /// rewrite but not proving one occurred.
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
/// content even though HEAD has not moved yet. A command that wrote the working
/// tree itself leaves that pair behind. It is not the only way to arrive there:
/// a rewind that wrote nothing leaves the same pair whenever the tree already
/// held the target's content, which is the bound named below.
///
/// Both halves are load-bearing, measured live against git 2.50.1 at the
/// `prepared` state. Dropping the ancestor test would report a refused
/// `commit --amend` as rewritten, because an amend leaves index and worktree
/// holding its own new tree; the amend is excluded instead because its
/// replacement tip is no ancestor of the tip it replaces. Dropping the content
/// test would report `reset --soft` and `reset --mixed` as rewritten, because
/// both move backwards; they are excluded instead because neither writes the
/// files on disk, so the worktree ordinarily still disagrees with the rewind
/// target. Ordinarily rather than always: a worktree that already agreed is not
/// made to disagree by a rewind that wrote nothing, so the content test admits
/// those two after all, which is the bound below.
///
/// The bound worth naming: a `--soft` or `--mixed` rewind onto a commit whose
/// content the index and worktree already happen to match is reported as
/// rewritten. The refusal text therefore treats this arm as an ambiguous
/// observable state and never prescribes a destructive reset from it.
fn worktree_damage(
    repository: &Path,
    update: &RefUpdate<'_>,
    tip: &str,
) -> Result<WorktreeDamage, ReferenceTransactionError> {
    if is_zero_oid(update.new) {
        return Ok(WorktreeDamage::Untouched);
    }
    if is_ancestor(repository, update.new, tip)? && worktree_holds(repository, update.new)? {
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
        baseline: String,
        baseline_commit: String,
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
                baseline,
                baseline_commit,
                damage,
            } => {
                write!(
                    formatter,
                    concat!(
                        "reference-transaction guard refused to abandon upstream-merged ",
                        "commits on current branch {}: {}. Published history was resolved ",
                        "to {} at {}, and those commits are contained in it; if that is not ",
                        "the reference this lane publishes to, the refusal is a ",
                        "misresolution and not a protection. This is expected when a task ",
                        "boundary has advanced origin/upstream; keep those commits and ",
                        "continue with an append-only commit instead of amending or ",
                        "resetting backwards. "
                    ),
                    reference, commits, baseline, baseline_commit
                )?;
                match damage {
                    WorktreeDamage::Rewritten => formatter.write_str(concat!(
                        "This refusal protected the branch, not necessarily the tree: HEAD ",
                        "was not moved, and the index and worktree match the refused rewind ",
                        "target. A hard rewind may have written them before any hook could ",
                        "run, but a soft or mixed rewind can look identical when that content ",
                        "was already present. Inspect `git status`, `git diff`, and ",
                        "`git diff --cached` before choosing any recovery; do not use a ",
                        "destructive reset based only on this diagnostic."
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
