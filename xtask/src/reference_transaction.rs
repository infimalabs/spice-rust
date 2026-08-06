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
//! `ProtectedHistory` refusal carries a recovery rather than merely reporting
//! that it refused. The recovery is `git reset --hard HEAD`, which this guard
//! admits because such an update leaves the reference where it already is, so
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

pub fn apply(
    repository: &Path,
    state: &OsStr,
    input: &str,
) -> Result<(), ReferenceTransactionError> {
    if state != OsStr::new(PREPARED) {
        return Ok(());
    }

    let Some(current_ref) = git_read(repository, &["symbolic-ref", "--quiet", "HEAD"])? else {
        return Ok(());
    };
    let Some(upstream) = publication_baseline(repository)? else {
        return Ok(());
    };

    for update in parse_updates(input)? {
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
                write!(formatter, "malformed reference-transaction line {line}")
            }
            Self::GitLaunch {
                repository,
                arguments,
                source,
            } => write!(
                formatter,
                "reference-transaction could not run git -C {} {}: {source}",
                repository.display(),
                arguments.join(" ")
            ),
            Self::NonUtf8Output { arguments } => write!(
                formatter,
                "reference-transaction git output was not UTF-8: git {}",
                arguments.join(" ")
            ),
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
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A scratch repository whose reference updates can be made to fail the way
    /// this guard fails them.
    struct Fixture(PathBuf);

    impl Fixture {
        /// Three commits on `main`, with no hook installed yet so the history
        /// itself is allowed to be built.
        fn history(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-xtask-refguard-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("exclusive scratch root is created");
            let fixture = Self(fs::canonicalize(&root).expect("scratch root canonicalizes"));

            fixture.expect(&["init", "--quiet", "--initial-branch=main", "."]);
            fixture.expect(&["config", "user.email", "lane@example.invalid"]);
            fixture.expect(&["config", "user.name", "lane"]);
            for subject in ["one", "two", "three"] {
                fs::write(fixture.0.join("file.txt"), format!("{subject}\n"))
                    .expect("tracked file is written");
                fixture.expect(&["add", "file.txt"]);
                fixture.expect(&["commit", "--quiet", "-m", subject]);
            }
            fixture
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn expect(&self, arguments: &[&str]) -> String {
            let output = git(&self.0, arguments).expect("git launches");
            assert!(
                output.status.success(),
                "git {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("git output is UTF-8")
                .trim()
                .to_owned()
        }

        fn head(&self) -> String {
            self.expect(&["rev-parse", "HEAD"])
        }

        /// Refuse the branch update the way the real guard refuses it, recording
        /// what git handed the hook so the fixture reads the real transaction
        /// rather than one it composed for itself.
        ///
        /// Only the queue naming the branch is refused. `git reset` queues
        /// `ORIG_HEAD` first, and a hook that refuses that one too aborts the
        /// reset before it ever writes the worktree, which is the state this
        /// fixture exists to observe.
        fn refuse_reference_updates(&self) {
            let hooks = self.0.join("refusing-hooks");
            fs::create_dir_all(&hooks).expect("hook directory is created");
            let hook = hooks.join("reference-transaction");
            fs::write(
                &hook,
                format!(
                    concat!(
                        "#!/usr/bin/env sh\n",
                        "if [ \"$1\" = prepared ]; then\n",
                        "\tqueue=$(cat)\n",
                        "\tprintf '%s\\n' \"$queue\" >>{}\n",
                        "\tcase \"$queue\" in *refs/heads/main*) exit 1 ;; esac\n",
                        "fi\n",
                        "exit 0\n"
                    ),
                    self.0.join("transaction").display()
                ),
            )
            .expect("hook is written");
            let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&hook, permissions).expect("hook is executable");
            self.expect(&[
                "config",
                "core.hooksPath",
                hooks.to_str().expect("hook path is UTF-8"),
            ]);
        }

        /// Run a command the hook above will refuse, and read the damage from
        /// the transaction git actually queued for the branch.
        fn refused(&self, arguments: &[&str]) -> WorktreeDamage {
            let before = self.head();
            let output = git(&self.0, arguments).expect("git launches");
            assert!(
                !output.status.success(),
                "git {} was not refused, so this fixture proves nothing",
                arguments.join(" ")
            );
            assert_eq!(
                self.head(),
                before,
                "a refused update must leave HEAD where it was"
            );

            let recorded =
                fs::read_to_string(self.0.join("transaction")).expect("the hook recorded a queue");
            let updates = parse_updates(&recorded).expect("the recorded queue parses");
            let update = updates
                .iter()
                .rfind(|update| update.reference == "refs/heads/main")
                .expect("the refused queue named the branch");
            worktree_damage(self.path(), update).expect("damage is read")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parser_ignores_blank_lines_and_preserves_each_update() {
        let updates = parse_updates("old new refs/heads/main\n\nzero tip refs/tags/v1\n").unwrap();
        assert_eq!(
            updates,
            [
                RefUpdate {
                    old: "old",
                    new: "new",
                    reference: "refs/heads/main",
                },
                RefUpdate {
                    old: "zero",
                    new: "tip",
                    reference: "refs/tags/v1",
                },
            ]
        );
    }

    #[test]
    fn parser_names_the_first_malformed_line() {
        let error = parse_updates("old new refs/heads/main\nmissing field\n").unwrap_err();
        assert!(matches!(
            error,
            ReferenceTransactionError::MalformedInput { line: 2 }
        ));
    }

    fn refusal(damage: WorktreeDamage) -> String {
        ReferenceTransactionError::ProtectedHistory {
            reference: "refs/heads/main".to_owned(),
            commits: "deadbee".to_owned(),
            damage,
        }
        .to_string()
    }

    /// The guard cannot protect the tree, so a refusal that arrived after the
    /// tree was written owes the reader the state it was left in and the way
    /// out. All three halves are load-bearing text.
    #[test]
    fn a_rewritten_worktree_is_named_along_with_the_recovery() {
        let message = refusal(WorktreeDamage::Rewritten);
        assert!(
            message.contains("HEAD was not moved"),
            "refusal must say the reference stayed put: {message}"
        );
        assert!(
            message.contains("index and worktree"),
            "refusal must report that the rewind already wrote the tree: {message}"
        );
        assert!(
            message.contains("git reset --hard HEAD"),
            "refusal must name the recovery this guard admits: {message}"
        );
    }

    /// The opposite refusal must not borrow the rewind's account. Telling an
    /// operator whose files were never written that they were overwritten sends
    /// them to a recovery that destroys the work this refusal left intact.
    #[test]
    fn an_untouched_worktree_is_never_reported_as_rewritten() {
        let message = refusal(WorktreeDamage::Untouched);
        assert!(
            !message.contains("index and worktree"),
            "refusal claimed a rewrite that did not happen: {message}"
        );
        assert!(
            !message.contains("git reset --hard HEAD"),
            "refusal offered a destructive recovery for undamaged files: {message}"
        );
        assert!(
            message.contains("did not write the working tree"),
            "refusal must say the files on disk were left alone: {message}"
        );
    }

    #[test]
    fn non_prepared_states_are_inert_before_input_or_repository_access() {
        apply(
            Path::new("repository-does-not-exist"),
            OsStr::new("committed"),
            "malformed",
        )
        .unwrap();
    }

    /// Read from a real refused rewind rather than from documentation: git
    /// writes the worktree before it opens the transaction this hook can veto.
    #[test]
    fn a_refused_hard_rewind_reports_a_rewritten_worktree() {
        let fixture = Fixture::history("hard");
        fixture.refuse_reference_updates();

        assert_eq!(
            fixture.refused(&["reset", "--hard", "HEAD~2"]),
            WorktreeDamage::Rewritten,
            "a refused reset --hard had already written the worktree"
        );
    }

    /// The amend is why the account has to be read rather than assumed. It
    /// leaves index and worktree holding its own tree exactly as a hard rewind
    /// does, so only the ancestor half of the discriminator separates them.
    #[test]
    fn a_refused_amend_reports_an_untouched_worktree() {
        let fixture = Fixture::history("amend");
        fs::write(fixture.path().join("file.txt"), "staged\n").expect("staged content is written");
        fixture.expect(&["add", "file.txt"]);
        fixture.refuse_reference_updates();

        assert_eq!(
            fixture.refused(&["commit", "--amend", "-m", "amended"]),
            WorktreeDamage::Untouched,
            "a refused amend never wrote the worktree"
        );
    }

    /// Both of these move the branch backwards exactly as `--hard` does, so
    /// they are what keeps the content half of the discriminator load-bearing.
    #[test]
    fn a_refused_soft_or_mixed_rewind_reports_an_untouched_worktree() {
        for mode in ["--soft", "--mixed"] {
            let fixture = Fixture::history("rewind");
            fixture.refuse_reference_updates();

            assert_eq!(
                fixture.refused(&["reset", mode, "HEAD~2"]),
                WorktreeDamage::Untouched,
                "a refused reset {mode} left the files on disk alone"
            );
        }
    }
}
