use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
        let empty_hooks = fixture.0.join("empty-hooks");
        fs::create_dir(&empty_hooks).expect("empty hook directory is created");
        fixture.expect(&[
            "config",
            "core.hooksPath",
            empty_hooks.to_str().expect("hook path is UTF-8"),
        ]);
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

    fn head_subject(&self) -> String {
        self.expect(&["log", "-1", "--format=%s"])
    }

    /// Publish the current tip through the same configuration `apply` reads.
    ///
    /// Both halves of the pairing are set because the guard reads both: a
    /// `merge` with no `remote` names no remote-tracking reference and is not
    /// a lane that tracks anything.
    fn publish_current_tip(&self) {
        let tip = self.head();
        self.add_origin_remote();
        self.track("origin", "refs/heads/main");
        self.expect(&["update-ref", "refs/remotes/origin/main", &tip]);
    }

    /// Pair the current branch with a remote and an integration branch, in the
    /// local scope the guard reads.
    fn track(&self, remote: &str, merge: &str) {
        self.expect(&["config", "--local", "branch.main.remote", remote]);
        self.expect(&["config", "--local", "branch.main.merge", merge]);
    }

    /// Poison the *effective* pairing the way a lane's git shadow poisons it,
    /// leaving the local scope intact.
    ///
    /// Measured in a lane worktree rather than imagined: the shadow overrides
    /// `branch.<lane>.remote` to `.` at command scope while the local scope
    /// still names the real remote, so `git config --get` answers `.` and
    /// `git config --local --get` answers `origin`. Worktree scope stands in
    /// for command scope here because both outrank local and a fixture cannot
    /// set the agent's command line; what is reproduced is the disagreement
    /// between the effective value and the local one, which is the whole trap.
    fn poison_effective_remote(&self) {
        self.expect(&["config", "extensions.worktreeConfig", "true"]);
        self.expect(&["config", "--worktree", "branch.main.remote", SELF_REMOTE]);
    }

    /// The origin remote alone, with nothing published through it yet.
    fn add_origin_remote(&self) {
        let origin = self.path().to_str().expect("scratch path is UTF-8");
        self.expect(&["remote", "add", "origin", origin]);
    }

    /// Publish the tip under a second remote-tracking name reachable only
    /// through `origin/HEAD`.
    ///
    /// The name differs from the integration branch's on purpose: a refusal
    /// that names this one cannot have arrived through `branch.<lane>.merge`,
    /// and a refusal that names the other cannot have arrived through here.
    fn point_origin_head_at(&self, branch: &str) {
        let tip = self.head();
        let reference = format!("refs/remotes/origin/{branch}");
        self.expect(&["update-ref", &reference, &tip]);
        self.expect(&["symbolic-ref", "refs/remotes/origin/HEAD", &reference]);
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
    fn refused_queue(&self, arguments: &[&str]) -> String {
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

        fs::read_to_string(self.0.join("transaction")).expect("the hook recorded a queue")
    }

    fn refused(&self, arguments: &[&str]) -> WorktreeDamage {
        let recorded = self.refused_queue(arguments);
        let updates = parse_updates(&recorded).expect("the recorded queue parses");
        let update = updates
            .iter()
            .rfind(|update| update.reference == "refs/heads/main")
            .expect("the refused queue named the branch");
        worktree_damage(self.path(), update).expect("damage is read")
    }

    /// Re-run the native guard against the queue and state a real refused Git
    /// operation left behind.
    fn applied_refusal(&self, arguments: &[&str]) -> ReferenceTransactionError {
        let recorded = self.refused_queue(arguments);
        apply(self.path(), OsStr::new(PREPARED), &recorded)
            .expect_err("the native guard refuses the recorded branch update")
    }

    /// The lock reference deletion takes in the common directory, which every
    /// linked worktree of a repository shares.
    fn shared_lock(&self) -> PathBuf {
        self.0
            .join(self.expect(&["rev-parse", "--git-common-dir"]))
            .join("packed-refs.lock")
    }

    /// Take the shared lock the way another writer takes it, and release it
    /// after `hold`.
    ///
    /// The lock exists before this returns, so a command started afterwards is
    /// guaranteed to contend rather than to race the holder into place. The
    /// released file is the one this fixture created; nothing here removes a
    /// lock another writer holds, which is the property
    /// `a_lock_another_writer_holds_survives_a_commit_that_gave_up` pins.
    fn hold_shared_lock(&self, hold: Duration) -> JoinHandle<()> {
        let lock = self.shared_lock();
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock)
            .expect("the scratch repository has no other writer");
        thread::spawn(move || {
            thread::sleep(hold);
            fs::remove_file(&lock).expect("a holder releases its own lock");
        })
    }

    /// Log every queue git hands the hook, tagged with whether the shared lock
    /// was held at the time, and answer with the log's path.
    fn record_lock_state(&self) -> PathBuf {
        let hooks = self.0.join("recording-hooks");
        fs::create_dir_all(&hooks).expect("hook directory is created");
        let hook = hooks.join("reference-transaction");
        let log = self.0.join("lock-state");
        fs::write(
            &hook,
            format!(
                concat!(
                    "#!/usr/bin/env sh\n",
                    "queue=$(cat)\n",
                    "if [ -e '{}' ]; then held=held; else held=free; fi\n",
                    "printf '%s %s %s\\n' \"$1\" \"$held\" \\\n",
                    "\t\"$(printf '%s' \"$queue\" | tr '\\n' ' ')\" >>'{}'\n",
                    "exit 0\n"
                ),
                self.shared_lock().display(),
                log.display()
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
        log
    }

    /// Stage one more revision of the tracked file, ready to be committed.
    fn stage(&self, content: &str) {
        fs::write(self.0.join("file.txt"), format!("{content}\n"))
            .expect("tracked file is written");
        self.expect(&["add", "file.txt"]);
    }
}

/// Everything a caller sees on the terminal. Mixed success-and-error output is
/// the defect under test, so the streams are read exactly as they arrive.
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // A fixture is not the writer that owns this shared lock. Refuse to
        // recursively erase the repository until that writer releases it.
        if self.shared_lock().exists() {
            return;
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ForeignLock(PathBuf);

impl ForeignLock {
    fn take(path: PathBuf, contents: &str) -> Self {
        fs::write(&path, contents).expect("the other writer takes the shared lock");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn release(self) {
        fs::remove_file(&self.0).expect("the other writer releases its shared lock");
    }
}

#[derive(Debug, PartialEq)]
struct LockIdentity {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    owner: u32,
    group: u32,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl LockIdentity {
    fn read(path: &Path) -> Self {
        let bytes = fs::read(path).expect("foreign lock bytes read");
        let metadata = fs::metadata(path).expect("foreign lock metadata reads");
        Self {
            bytes,
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            size: metadata.size(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
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

/// Pin the shared statement through one concrete operational variant.
///
/// Each caller remains a separately named test so removing the production
/// binding from any one Display arm fails that arm's fixture, not only a sibling.
fn assert_operational_error_names_tree_and_recovery(error: ReferenceTransactionError) {
    let message = error.to_string();
    assert!(
        message.contains("HEAD is not moved"),
        "operational refusal must say the reference stays put: {message}"
    );
    assert!(
        message.contains("index and worktree may already have changed"),
        "operational refusal must name the uncertain tree state: {message}"
    );
    assert!(
        message.contains("git reset --hard HEAD"),
        "operational refusal must name the admitted rewind recovery: {message}"
    );
}

#[test]
fn malformed_input_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::MalformedInput {
        line: 2,
    });
}

#[test]
fn git_launch_failure_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::GitLaunch {
        repository: PathBuf::from("/fixture/repository"),
        arguments: vec!["status".to_owned()],
        source: io::Error::new(io::ErrorKind::NotFound, "fixture git is absent"),
    });
}

#[test]
fn non_utf8_output_names_the_tree_risk_and_recovery() {
    assert_operational_error_names_tree_and_recovery(ReferenceTransactionError::NonUtf8Output {
        arguments: vec!["symbolic-ref".to_owned(), "HEAD".to_owned()],
    });
}

fn refusal(damage: WorktreeDamage) -> String {
    ReferenceTransactionError::ProtectedHistory {
        reference: "refs/heads/main".to_owned(),
        commits: "deadbee".to_owned(),
        baseline: "refs/remotes/origin/main".to_owned(),
        baseline_commit: "0ddba11".to_owned(),
        damage,
    }
    .to_string()
}

/// The guard cannot prove which command produced a matching tree, so this arm
/// names the observable state without prescribing destructive recovery.
#[test]
fn a_rewritten_worktree_is_named_without_prescribing_destructive_recovery() {
    let message = refusal(WorktreeDamage::Rewritten);
    assert!(
        message.contains("HEAD was not moved"),
        "refusal must say the reference stayed put: {message}"
    );
    assert!(
        message.contains("index and worktree match"),
        "refusal must report the matching tree state: {message}"
    );
    assert!(
        !message.contains("git reset --hard HEAD"),
        "an ambiguous tree state prescribed destructive recovery: {message}"
    );
    assert!(
        message.contains("Inspect `git status`"),
        "an ambiguous tree state did not name a safe next observation: {message}"
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

/// The damage arm is selected by `apply`, not merely by direct unit calls.
#[test]
fn apply_reports_the_rewritten_arm_for_a_refused_hard_rewind() {
    let fixture = Fixture::history("apply-hard");
    fixture.publish_current_tip();
    fixture.refuse_reference_updates();

    let error = fixture.applied_refusal(&["reset", "--hard", "HEAD~2"]);
    assert!(
        matches!(
            &error,
            ReferenceTransactionError::ProtectedHistory {
                damage: WorktreeDamage::Rewritten,
                ..
            }
        ),
        "apply selected the wrong damage arm: {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("index and worktree match the refused rewind target"),
        "the apply refusal did not render the rewritten arm: {message}"
    );
}

/// A genuine refusal that did not write the tree reaches the opposite arm
/// through the same `apply` entry point.
#[test]
fn apply_reports_the_untouched_arm_for_a_refused_amend() {
    let fixture = Fixture::history("apply-amend");
    fixture.publish_current_tip();
    fs::write(fixture.path().join("file.txt"), "staged\n").expect("amended content is written");
    fixture.expect(&["add", "file.txt"]);
    fixture.refuse_reference_updates();

    let error = fixture.applied_refusal(&["commit", "--amend", "-m", "amended"]);
    assert!(
        matches!(
            &error,
            ReferenceTransactionError::ProtectedHistory {
                damage: WorktreeDamage::Untouched,
                ..
            }
        ),
        "apply selected the wrong damage arm: {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("did not write the working tree"),
        "the apply refusal did not render the untouched arm: {message}"
    );
}

/// A soft rewind can satisfy the rewritten discriminator without writing the
/// index or worktree. That ambiguous state must never prescribe reset --hard.
#[test]
fn a_matching_soft_rewind_never_prescribes_destructive_recovery() {
    let fixture = Fixture::history("matching-soft");
    fixture.publish_current_tip();
    fs::write(fixture.path().join("file.txt"), "one\n")
        .expect("the existing tree is made to match the rewind target");
    fixture.expect(&["add", "file.txt"]);
    fixture.refuse_reference_updates();

    let error = fixture.applied_refusal(&["reset", "--soft", "HEAD~2"]);
    assert!(
        matches!(
            &error,
            ReferenceTransactionError::ProtectedHistory {
                damage: WorktreeDamage::Rewritten,
                ..
            }
        ),
        "the matching-content bound was not reached: {error:?}"
    );
    let message = error.to_string();
    assert!(
        !message.contains("git reset --hard HEAD"),
        "a soft rewind that wrote neither index nor tree prescribed destructive recovery: {message}"
    );
    assert!(
        message.contains("soft or mixed rewind can look identical"),
        "the refusal hid why its damage classification is ambiguous: {message}"
    );
}

/// The guard runs inside a lock every worktree shares, so a queue it cannot
/// possibly refuse has to be answered without launching anything.
///
/// Proven by severing rather than by timing: the repository is a directory
/// that does not exist, so any launch attempt fails and cannot be mistaken for
/// a launch that happened to succeed.
#[test]
fn a_queue_naming_no_branch_is_answered_without_launching_git() {
    let absent = std::env::temp_dir().join(format!(
        "spice-xtask-refguard-absent-{}",
        std::process::id()
    ));
    assert!(!absent.exists(), "the severing directory must not exist");
    let cleanup = "0000000000000000000000000000000000000000 \
                   0000000000000000000000000000000000000000 AUTO_MERGE\n";

    apply(&absent, OsStr::new(PREPARED), cleanup)
        .expect("a queue naming no branch is answered from the queue alone");

    // Without this control the pass above proves nothing: it would read the
    // same whether the guard launched nothing or launched something that
    // quietly succeeded. The same unreachable directory must still stop a
    // queue that does name the branch.
    let error = apply(
        &absent,
        OsStr::new(PREPARED),
        "0000000000000000000000000000000000000000 aaaabbbb refs/heads/main\n",
    )
    .expect_err("a queue naming the branch has to reach git");
    assert!(
        matches!(error, ReferenceTransactionError::GitLaunch { .. }),
        "the control must fail at the launch itself: {error:?}"
    );
}

/// Answering early must not become a way out of the refusal itself. Every
/// other refusal test reads a scratch hook that mimics this guard, so this is
/// the one that drives a genuine refusal through `apply`.
#[test]
fn a_queue_abandoning_published_history_is_still_refused() {
    let fixture = Fixture::history("published");
    let tip = fixture.head();
    fixture.publish_current_tip();
    let base = fixture.expect(&["rev-parse", "HEAD~2"]);

    let error = apply(
        fixture.path(),
        OsStr::new(PREPARED),
        &format!("{tip} {base} refs/heads/main\n"),
    )
    .expect_err("an update abandoning published commits is refused");

    assert!(
        matches!(error, ReferenceTransactionError::ProtectedHistory { .. }),
        "the guard answered something other than a refusal: {error:?}"
    );
}

/// Drive a genuine refusal and answer with what its message said.
fn refusal_message(fixture: &Fixture) -> String {
    let tip = fixture.head();
    let base = fixture.expect(&["rev-parse", "HEAD~2"]);
    apply(
        fixture.path(),
        OsStr::new(PREPARED),
        &format!("{tip} {base} refs/heads/main\n"),
    )
    .expect_err("an update abandoning published commits is refused")
    .to_string()
}

/// A refusal that names no baseline cannot be told apart from a misresolved
/// one, so each resolution has to say which reference it reached.
///
/// Both resolutions are configured here and they name different references, so
/// this assertion fails if the guard reaches the backstop instead.
#[test]
fn a_refusal_names_the_integration_reference_it_resolved() {
    let fixture = Fixture::history("names-integration");
    fixture.publish_current_tip();
    fixture.point_origin_head_at("trunk");
    let published = fixture.expect(&["rev-parse", "--short", "refs/remotes/origin/main"]);

    let message = refusal_message(&fixture);

    assert!(
        message.contains("refs/remotes/origin/main"),
        "the refusal never named the integration reference it resolved: {message}"
    );
    assert!(
        !message.contains("refs/remotes/origin/trunk"),
        "the refusal named the backstop instead of the integration branch: {message}"
    );
    assert!(
        message.contains(&published),
        "the refusal named a reference without the commit it stood at: {message}"
    );
}

/// The pairing is the only resolution, so a published `origin/HEAD` behind a
/// branch that pairs with nothing resolves no baseline at all.
///
/// This is the shape that would previously have been refused through the
/// backstop, kept as a test because deleting the backstop without pinning its
/// absence would leave the guard free to grow a second resolution back. The
/// predecessor has no such fallback, and a baseline reached only on this side
/// is a policy this side invented.
#[test]
fn a_branch_that_pairs_with_nothing_is_inert_even_where_origin_head_is_published() {
    let fixture = Fixture::history("no-backstop");
    fixture.add_origin_remote();
    fixture.point_origin_head_at("trunk");
    let tip = fixture.head();
    let base = fixture.expect(&["rev-parse", "HEAD~2"]);

    apply(
        fixture.path(),
        OsStr::new(PREPARED),
        &format!("{tip} {base} refs/heads/main\n"),
    )
    .expect("origin/HEAD is not a baseline this guard resolves");
}

/// A pairing whose remote is the self remote names no reference at all.
///
/// `.` is what the shadow writes to aim a lane's upstream at itself, and the
/// predecessor rejects it by name rather than letting it through to be
/// resolved. This asserts the resolution rather than the guard's end state on
/// purpose: a `.` remote spells `refs/remotes/./<branch>`, which git does not
/// normalise and never resolves, so the guard falls inert either way and an
/// end-to-end assertion would pass just as well with the rule deleted. Only
/// the resolved value separates the stated rule from that accident. The arm
/// below then confirms the accident is not doing the work.
#[test]
fn a_branch_paired_with_the_self_remote_names_no_reference() {
    let fixture = Fixture::history("self-remote");
    let tip = fixture.head();
    let base = fixture.expect(&["rev-parse", "HEAD~2"]);
    fixture.add_origin_remote();
    fixture.expect(&["update-ref", "refs/remotes/origin/main", &tip]);
    fixture.track(SELF_REMOTE, "refs/heads/main");

    assert_eq!(
        declared_upstream_reference(fixture.path(), "refs/heads/main")
            .expect("the pairing is readable"),
        None,
        "the self remote was carried into a reference instead of being refused by name"
    );

    apply(
        fixture.path(),
        OsStr::new(PREPARED),
        &format!("{tip} {base} refs/heads/main\n"),
    )
    .expect("a branch paired with the self remote tracks no published history");
}

/// The pairing names the remote, so a lane that publishes through a remote
/// other than `origin` is protected under that remote's name.
///
/// Nothing in the pairing rule privileges `origin`, and a resolution that
/// hardcoded it would answer a reference this fixture never publishes while
/// silently protecting nothing. The integration branch is deliberately not
/// `main` either, so a hardcoded branch name fails here too.
#[test]
fn the_pairing_names_the_remote_rather_than_assuming_origin() {
    let fixture = Fixture::history("named-remote");
    let tip = fixture.head();
    let upstream = fixture.path().to_str().expect("scratch path is UTF-8");
    fixture.expect(&["remote", "add", "upstream", upstream]);
    fixture.expect(&["update-ref", "refs/remotes/upstream/trunk", &tip]);
    fixture.track("upstream", "refs/heads/trunk");

    let message = refusal_message(&fixture);

    assert!(
        message.contains("refs/remotes/upstream/trunk"),
        "the guard did not resolve the remote and branch the pairing named: {message}"
    );
}

/// The baseline this guard resolves under a lane's real shadow configuration
/// must be the one the predecessor resolves there.
///
/// The predecessor's `_declared_upstream_ref` in `spice/hooks/refguard.py`
/// reads `branch.<lane>.remote` and `branch.<lane>.merge` from local scope and
/// from nowhere else, so under the shadow it answers `refs/remotes/origin/main`
/// and the guard stays live. Reading the effective value instead answers `.`
/// there, which this guard treats as no upstream, and the whole guard would go
/// quietly inert in every lane while its battery stayed green -- a rewind that
/// abandons published work would simply be permitted.
///
/// So this fixture carries the disagreement itself: local scope names `origin`
/// while the effective scope answers `.`. The refusal is asserted rather than
/// merely the resolution, because being live is the property that is lost.
#[test]
fn the_baseline_under_a_poisoned_effective_remote_is_the_one_local_scope_names() {
    let fixture = Fixture::history("shadow-parity");
    fixture.publish_current_tip();
    fixture.poison_effective_remote();
    let published = fixture.expect(&["rev-parse", "--short", "refs/remotes/origin/main"]);

    assert_eq!(
        fixture.expect(&["config", "--get", "branch.main.remote"]),
        SELF_REMOTE,
        "the fixture never reproduced the shadow: the effective remote is not poisoned"
    );
    assert_eq!(
        fixture.expect(&["config", "--local", "--get", "branch.main.remote"]),
        "origin",
        "the fixture never reproduced the shadow: local scope lost the real remote"
    );

    let message = refusal_message(&fixture);

    assert!(
        message.contains("refs/remotes/origin/main"),
        "the guard did not resolve the reference local scope names: {message}"
    );
    assert!(
        message.contains(&published),
        "the refusal named a reference without the commit it stood at: {message}"
    );
}

/// Carrying the reference out of the resolution must not give the guard a
/// baseline it never resolved. Each arm leaves it inert, and the rewind that
/// the two tests above refuse is permitted here instead.
#[test]
fn a_lane_with_no_resolvable_baseline_permits_the_rewind_it_would_otherwise_refuse() {
    let unpublished = |label: &str| {
        let fixture = Fixture::history(label);
        let tip = fixture.head();
        let base = fixture.expect(&["rev-parse", "HEAD~2"]);
        (fixture, format!("{tip} {base} refs/heads/main\n"))
    };

    let (fixture, queue) = unpublished("inert-no-pairing");
    fixture.add_origin_remote();
    apply(fixture.path(), OsStr::new(PREPARED), &queue)
        .expect("a branch whose pairing names no remote tracks no published history");

    let (fixture, queue) = unpublished("inert-no-merge");
    fixture.add_origin_remote();
    fixture.expect(&["config", "--local", "branch.main.remote", "origin"]);
    apply(fixture.path(), OsStr::new(PREPARED), &queue)
        .expect("a pairing naming a remote but no integration branch is inert");

    let (fixture, queue) = unpublished("inert-never-published");
    fixture.add_origin_remote();
    fixture.track("origin", "refs/heads/main");
    apply(fixture.path(), OsStr::new(PREPARED), &queue)
        .expect("an integration branch never published is inert");

    // Without this the three passes above are indistinguishable from a guard
    // that stopped refusing anything: the same queue, in the same fixture
    // shape, must still be refused once a baseline exists.
    let (fixture, queue) = unpublished("inert-control");
    fixture.publish_current_tip();
    apply(fixture.path(), OsStr::new(PREPARED), &queue)
        .expect_err("the control must still be refused once the tip is published");
}

/// Read from a real commit rather than from githooks(5): the transaction this
/// guard inspects and the transaction that holds the shared lock are disjoint.
#[test]
fn only_the_queue_naming_no_branch_is_handed_over_under_the_shared_lock() {
    let fixture = Fixture::history("locked");
    let log = fixture.record_lock_state();
    fixture.stage("four");
    fixture.expect(&["commit", "--quiet", "-m", "four"]);

    let recorded = fs::read_to_string(&log).expect("the hook recorded its states");
    let mut held = 0;
    let mut named_branch_free = 0;
    for line in recorded.lines() {
        let (state, rest) = line.split_once(' ').expect("each record is tagged");
        let (lock, queue) = rest.split_once(' ').expect("each record names the lock");
        if lock == "held" {
            held += 1;
            assert!(
                !queue.contains(LOCAL_PREFIX),
                "a queue naming the branch was handed over under the shared \
                 lock, so the guard's early answer is unsound: {line}"
            );
        } else if queue.contains(LOCAL_PREFIX) && state == PREPARED {
            named_branch_free += 1;
        }
    }
    assert!(
        held > 0,
        "no queue was handed over under the shared lock, so this proves \
         nothing: {recorded}"
    );
    assert!(
        named_branch_free > 0,
        "the branch update was never observed, so this proves nothing: \
         {recorded}"
    );
}

/// Contention that Git's own bounded retry absorbs must leave no trace. This
/// is the first half of the outcome an agent reads: silence means recovered.
#[test]
fn a_commit_that_waited_for_the_shared_lock_reports_nothing() {
    let fixture = Fixture::history("waited");
    fixture.expect(&["config", "core.packedRefsTimeout", "10000"]);
    fixture.stage("four");
    let before = fixture.head();

    let hold = Duration::from_millis(400);
    let holder = fixture.hold_shared_lock(hold);
    let started = Instant::now();
    let output = git(fixture.path(), &["commit", "--quiet", "-m", "four"]).expect("git launches");
    let waited = started.elapsed();
    holder.join().expect("the holder thread finishes");

    assert!(
        waited >= hold,
        "the commit never contended, so this proves nothing"
    );
    assert!(
        output.status.success(),
        "the commit failed: {}",
        combined(&output)
    );
    assert_ne!(
        fixture.head(),
        before,
        "the successful commit advances HEAD"
    );
    assert_eq!(
        fixture.head_subject(),
        "four",
        "the requested commit was recorded"
    );
    let seen = combined(&output);
    assert!(
        !seen.contains("packed-refs.lock"),
        "a commit that recovered on its own must say nothing about the lock: \
         {seen}"
    );
}

/// The companion the clean-output assertion needs. If a commit could be made
/// quiet by muting its streams, the test above would pass on a broken tree.
#[test]
fn a_commit_that_genuinely_fails_still_reports_its_failure() {
    let fixture = Fixture::history("failed");

    let output = git(fixture.path(), &["commit", "-m", "nothing staged"]).expect("git launches");

    assert!(
        !output.status.success(),
        "a commit with nothing staged has to fail for this to prove anything"
    );
    assert!(
        !combined(&output).trim().is_empty(),
        "a failed commit that says nothing reads exactly like a clean one"
    );
}

/// The second half of the outcome an agent reads: a message means a real
/// cleanup was genuinely skipped, and the lock behind it belongs to somebody
/// else. Nothing may remove, truncate or steal it.
#[test]
fn a_lock_another_writer_holds_survives_a_commit_that_gave_up() {
    let fixture = Fixture::history("survives");
    let fixture_root = fixture.path().to_owned();
    fixture.expect(&["config", "core.packedRefsTimeout", "100"]);
    let held = "another writer holds this\n";
    let foreign_lock = ForeignLock::take(fixture.shared_lock(), held);
    let identity = LockIdentity::read(foreign_lock.path());
    fixture.stage("five");
    let before = fixture.head();

    let output = git(fixture.path(), &["commit", "--quiet", "-m", "five"]).expect("git launches");

    assert!(
        output.status.success(),
        "recording the commit never needs the shared lock: {}",
        combined(&output)
    );
    assert_ne!(
        fixture.head(),
        before,
        "the successful commit advances HEAD"
    );
    assert_eq!(
        fixture.head_subject(),
        "five",
        "the requested commit was recorded"
    );
    let seen = combined(&output);
    assert!(
        seen.contains("Unable to create")
            && seen.contains("packed-refs.lock")
            && seen.contains("File exists"),
        "a cleanup that genuinely gave up must say so, or silence stops \
         meaning recovered: {}",
        seen
    );
    assert_eq!(
        LockIdentity::read(foreign_lock.path()),
        identity,
        "the lock another writer holds changed bytes or metadata"
    );
    drop(fixture);
    assert_eq!(
        LockIdentity::read(foreign_lock.path()),
        identity,
        "fixture cleanup touched the lock that the other writer still holds"
    );
    foreign_lock.release();
    fs::remove_dir_all(fixture_root).expect("the released fixture is removed");
}
