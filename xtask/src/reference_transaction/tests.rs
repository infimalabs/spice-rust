use std::fs;
use std::os::unix::fs::PermissionsExt;
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

    /// Publish the current tip through the same configuration `apply` reads.
    fn publish_current_tip(&self) {
        let tip = self.head();
        let origin = self.path().to_str().expect("scratch path is UTF-8");
        self.expect(&["remote", "add", "origin", origin]);
        self.expect(&["config", "branch.main.merge", "refs/heads/main"]);
        self.expect(&["update-ref", "refs/remotes/origin/main", &tip]);
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
    fixture.expect(&["config", "core.packedRefsTimeout", "100"]);
    let lock = fixture.shared_lock();
    let held = "another writer holds this\n";
    fs::write(&lock, held).expect("the other writer takes the shared lock");
    let stamped = fs::metadata(&lock).expect("lock metadata").modified().ok();
    fixture.stage("five");

    let output = git(fixture.path(), &["commit", "--quiet", "-m", "five"]).expect("git launches");

    assert!(
        output.status.success(),
        "recording the commit never needs the shared lock: {}",
        combined(&output)
    );
    assert!(
        combined(&output).contains("packed-refs.lock"),
        "a cleanup that genuinely gave up must say so, or silence stops \
         meaning recovered: {}",
        combined(&output)
    );
    assert_eq!(
        fs::read_to_string(&lock).expect("the lock is still there"),
        held,
        "the lock another writer holds was removed, truncated or stolen"
    );
    assert_eq!(
        fs::metadata(&lock).expect("lock metadata").modified().ok(),
        stamped,
        "the lock another writer holds was rewritten"
    );
    fs::remove_file(&lock).expect("this fixture releases the lock it took");
}
