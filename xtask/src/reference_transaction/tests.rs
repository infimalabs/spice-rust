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

    /// Publish an integration commit standing one ahead of the branch tip.
    ///
    /// Where the published commit is the tip itself, that same short oid also
    /// stands in the abandoned-commits list the refusal prints, so a message
    /// naming it there satisfies an assertion meant for the baseline and the
    /// baseline field could be a constant. Publishing ahead separates the two:
    /// the baseline commit is then one no abandoned commit can impersonate.
    ///
    /// It is also the shape the refusal describes rather than an arrangement
    /// invented for the assertion -- a task boundary that advanced the
    /// integration reference past this lane. The branch is left where it was,
    /// because a fixture that moved it would be publishing this lane's own tip
    /// again under another name.
    fn publish_commit_ahead_of_tip(&self) {
        let tip = self.head();
        let tree = format!("{tip}^{{tree}}");
        let advanced = self.expect(&[
            "commit-tree",
            &tree,
            "-p",
            &tip,
            "-m",
            "integration advanced past this lane",
        ]);
        self.add_origin_remote();
        self.track("origin", "refs/heads/main");
        self.expect(&["update-ref", "refs/remotes/origin/main", &advanced]);
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

    /// The tip comes from HEAD, which `refused_queue` has just asserted the
    /// refusal left where it was -- the same value `apply` reads out of the
    /// reference itself while the transaction is still `prepared`.
    fn refused(&self, arguments: &[&str]) -> WorktreeDamage {
        let recorded = self.refused_queue(arguments);
        let tip = self.head();
        let updates = parse_updates(&recorded).expect("the recorded queue parses");
        let update = updates
            .iter()
            .rfind(|update| update.reference == "refs/heads/main")
            .expect("the refused queue named the branch");
        worktree_damage(self.path(), update, &tip).expect("damage is read")
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
                new: "new",
                reference: "refs/heads/main",
            },
            RefUpdate {
                new: "tip",
                reference: "refs/tags/v1",
            },
        ]
    );
}

/// The declared old value is required on the line and dropped from the update.
///
/// `git update-ref <reference> <new>` declares no old value, so Git writes the
/// zero OID into a field a rewind and a branch creation then share. Two queues
/// differing only there have to parse to the same updates; while the field
/// survives parsing, something downstream can still reach it.
#[test]
fn the_declared_old_value_does_not_survive_parsing() {
    let real = format!("{:040} new refs/heads/main\n", 1);
    let zero = format!("{:040} new refs/heads/main\n", 0);
    let declared = parse_updates(&real).unwrap();
    let undeclared = parse_updates(&zero).unwrap();
    assert_eq!(
        declared, undeclared,
        "a declared old value reached the parsed update, so a rewind and a \
         creation can still be told apart by a field git does not fill in"
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

/// The one phrase binding a resolved baseline reference to the commit it stood
/// at, asserted whole rather than as two independent searches.
///
/// The refusal also prints the abandoned commits, so the message is full of
/// short oids. A bare search for the baseline commit is satisfied by any of
/// them, and an assertion that searches that way holds just as well for a
/// guard rendering a baseline commit it never resolved. Binding the commit to
/// the reference it follows is what makes finding it mean anything.
fn baseline_phrase(reference: &str, commit: &str) -> String {
    format!("resolved to {reference} at {commit}")
}

/// The one phrase binding the refused branch to the whole list of commits the
/// guard protected on it, terminator included.
///
/// The terminating `.` is load-bearing rather than punctuation carried along,
/// and it guards the opposite direction from the one that looks obvious.
/// Naming both commits in order is what catches a list rendered short; the
/// terminator is what catches a list rendered long, naming commits the guard
/// never protected, because without it the phrase is only a prefix and any
/// appended oid satisfies it. Both were measured against the assertion below:
/// rendering only the first abandoned commit reds it, and appending one the
/// guard never selected reds it. This mirrors `baseline_phrase` for the same
/// reason: the message is full of short oids, and a bare search for one of them
/// says nothing about the field it was printed in.
fn abandoned_phrase(reference: &str, commits: &str) -> String {
    format!("commits on current branch {reference}: {commits}.")
}

/// The hand-built refusal keeps its baseline commit beside its reference.
///
/// The fixture-driven assertions below depend on exactly this adjacency, and a
/// rewording that split the pair would leave them searching for a phrase no
/// message contains -- failing loudly, but for a reason that reads as a lost
/// resolution rather than a moved word. Pinned here from hand-built fields so
/// the shape is stated once, without a git fixture, next to the values that
/// compose it.
#[test]
fn a_refusal_names_its_baseline_commit_beside_the_reference_it_resolved() {
    let message = refusal(WorktreeDamage::Untouched);
    assert!(
        message.contains(&baseline_phrase("refs/remotes/origin/main", "0ddba11")),
        "the refusal parted the baseline reference from the commit it stood at: {message}"
    );
}

/// The hand-built refusal keeps its abandoned commits beside the branch they
/// were on, and its own `commits` field is what proves it.
///
/// This is the counterpart of the assertion above, and it exists for the same
/// reason: the fixture-driven assertion below reads this adjacency, so a
/// rewording that split the list from its branch would leave that one hunting a
/// phrase no message contains. It also stops `deadbee` standing in for coverage
/// it does not carry -- the value is hand-built, so it proves the rendering
/// shape and nothing about which commits the guard selects, which is exactly the
/// half the fixture-driven test owns.
#[test]
fn a_refusal_lists_its_abandoned_commits_beside_the_branch_they_were_on() {
    let message = refusal(WorktreeDamage::Untouched);
    assert!(
        message.contains(&abandoned_phrase("refs/heads/main", "deadbee")),
        "the refusal parted the abandoned commits from the branch carrying them: {message}"
    );
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
    fixture.publish_commit_ahead_of_tip();
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
        message.contains(&baseline_phrase("refs/remotes/origin/main", &published)),
        "the refusal named a reference without the commit it stood at: {message}"
    );
}

/// The refusal names the commits it actually protected, all of them, read from
/// the fixture rather than written down here.
///
/// Every other assertion in this file reads the branch, the baseline, or the
/// worktree damage; none reads a rewound commit, so the abandoned list could be
/// rendered as any constant and the whole package stays green. Measured that
/// way: replacing the list with a literal leaves `cargo test -p xtask` at 119
/// passed. So the guard could name a single commit it never protected in place
/// of every commit it did.
///
/// `publish_commit_ahead_of_tip` is what lets the list be named without
/// ambiguity, and the two inequalities below are controls on the fixture rather
/// than on the guard: they fail if the arrangement ever stops separating the
/// baseline from the commits being abandoned, which is the condition that makes
/// the phrase assertion meaningful.
#[test]
fn a_refusal_names_every_commit_it_protected_rather_than_the_baseline() {
    let fixture = Fixture::history("names-abandoned");
    fixture.publish_commit_ahead_of_tip();
    let tip = fixture.expect(&["rev-parse", "--short", "HEAD"]);
    let parent = fixture.expect(&["rev-parse", "--short", "HEAD~1"]);
    let baseline = fixture.expect(&["rev-parse", "--short", "refs/remotes/origin/main"]);
    assert_ne!(
        tip, baseline,
        "the fixture stopped standing the baseline apart from the abandoned tip"
    );
    assert_ne!(
        parent, baseline,
        "the fixture stopped standing the baseline apart from the abandoned parent"
    );

    let message = refusal_message(&fixture);

    assert!(
        message.contains(&abandoned_phrase(
            "refs/heads/main",
            &format!("{tip}, {parent}")
        )),
        "the refusal did not list the commits it protected, newest first: {message}"
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
    fixture.publish_commit_ahead_of_tip();
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
        message.contains(&baseline_phrase("refs/remotes/origin/main", &published)),
        "the refusal named a reference without the commit it stood at: {message}"
    );
}

/// `_declared_upstream_ref` from the predecessor's `spice/hooks/refguard.py`,
/// transcribed.
///
/// Transcribed rather than executed, because the predecessor is a separate
/// checkout that may be absent: a differential that skipped itself when the
/// other side went missing would stop checking at exactly the moment the two
/// guards were free to drift. What it costs is that this transcription can go
/// stale against a predecessor that moves, which is the trade the same decision
/// already makes in `spice/tests/config_coexistence.rs`.
///
/// Written as its own resolution rather than by calling the guard's, so the two
/// answers compared below are genuinely two answers. It spells its own literals
/// for the same reason: sharing `SELF_REMOTE` or `LOCAL_PREFIX` would let a
/// redefinition of either move both sides at once, and two sides that move
/// together agree about everything. These are the strings the predecessor's own
/// constants hold.
fn predecessor_upstream_reference(fixture: &Fixture, current_ref: &str) -> Option<String> {
    let branch = current_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(current_ref);
    let read = |key: String| {
        let output =
            git(fixture.path(), &["config", "--local", "--get", &key]).expect("git launches");
        let value = String::from_utf8(output.stdout)
            .expect("git output is UTF-8")
            .trim()
            .to_owned();
        (output.status.success() && !value.is_empty()).then_some(value)
    };

    let remote = read(format!("branch.{branch}.remote"))?;
    let merge = read(format!("branch.{branch}.merge"))?;
    if remote == "." {
        return None;
    }
    let name = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
    Some(format!("refs/remotes/{remote}/{name}"))
}

/// The two guards resolve one baseline, row for row.
///
/// The test above pins that this guard stays live under the shadow; this one
/// pins that the predecessor stays live there too, and on the same reference.
/// Those are separate claims: a port could hold its own behaviour still while
/// drifting away from the system it has to agree with, and nothing that asserts
/// only this side's answer would notice. Each row compares the declared
/// reference and then the resolved commit, so a side that changed either --
/// or changed whether it answered at all -- parts from the other here.
///
/// The reference is compared and not just the commit because one row is only
/// visible there. A `.` remote composes `refs/remotes/./<branch>`, which git
/// never resolves -- measured, not assumed -- so dropping the `.` refusal
/// outright would still reach no commit and agree on the commit alone.
///
/// The rows resolve as well as decline, and the closing assertion refuses a run
/// in which none resolved, because two resolutions that both answer nothing
/// agree about everything.
#[test]
fn the_baseline_resolved_here_is_the_baseline_the_predecessor_resolves() {
    let rows: [Row; 4] = [
        // The configuration a lane actually runs under, where both sides must
        // read past the shadow to the same live reference.
        ("agree-shadow", |fixture| {
            fixture.publish_current_tip();
            fixture.poison_effective_remote();
        }),
        // A pairing that names `.` in local scope, which is the one the shadow
        // cannot produce and both sides refuse outright.
        ("agree-self-paired", |fixture| {
            fixture.publish_current_tip();
            fixture.track(SELF_REMOTE, "refs/heads/main");
        }),
        // Neither `origin` nor the branch's own name, so agreement here cannot
        // come from either side assuming one.
        ("agree-named-remote", |fixture| {
            let tip = fixture.head();
            fixture.expect(&["update-ref", "refs/remotes/upstream/trunk", &tip]);
            fixture.track("upstream", "refs/heads/trunk");
        }),
        ("agree-merge-alone", |fixture| {
            fixture.expect(&["config", "--local", "branch.main.merge", "refs/heads/main"]);
        }),
    ];

    let mut resolved = 0;
    for (label, configure) in rows {
        let fixture = Fixture::history(label);
        configure(&fixture);
        let current_ref = fixture.expect(&["symbolic-ref", "--quiet", "HEAD"]);

        let ours = declared_upstream_reference(fixture.path(), &current_ref)
            .expect("the pairing reaches an answer");
        let theirs = predecessor_upstream_reference(&fixture, &current_ref);
        assert_eq!(
            ours, theirs,
            "on {label} this guard and the predecessor declared different \
             upstreams, so the two systems no longer read the same pairing"
        );

        let our_baseline = match publication(fixture.path(), &current_ref)
            .expect("the baseline resolution reaches an answer")
        {
            Publication::Published(baseline) => Some((baseline.reference, baseline.commit)),
            Publication::Untracked | Publication::Unjudgeable { .. } => None,
        };
        let their_baseline = theirs.and_then(|reference| {
            commit_oid(fixture.path(), &reference)
                .expect("the transcribed reference resolves or does not")
                .map(|commit| (reference, commit))
        });
        assert_eq!(
            our_baseline, their_baseline,
            "on {label} this guard and the predecessor resolved different \
             baselines, so the two systems no longer refuse the same rewinds"
        );
        resolved += usize::from(our_baseline.is_some());
    }

    assert!(
        resolved > 0,
        "no row resolved a baseline, so the agreement above is agreement \
         between two resolutions that answered nothing"
    );
}

/// One row: the scratch repository's label, and the pairing it carries.
type Row = (&'static str, fn(&Fixture));

/// A rewind and an append on the same three-commit history.
///
/// The two differ in their new value alone, which since the tip is read from
/// the reference is the only field either arm consults.
///
/// The append advances onto a commit built ahead of the tip rather than
/// restating the tip itself. An update whose new value *is* the tip reaches it
/// trivially, and would satisfy a guard that compared the two for equality
/// instead of asking whether the tip is still reached; the ordinary commit this
/// guard must not strand advances onto a descendant, so that is what is queued.
fn rewind_and_append(fixture: &Fixture) -> (String, String) {
    let tip = fixture.head();
    let base = fixture.expect(&["rev-parse", "HEAD~2"]);
    let advanced = fixture.expect(&[
        "commit-tree",
        &format!("{tip}^{{tree}}"),
        "-p",
        &tip,
        "-m",
        "the lane's next commit",
    ]);
    (
        format!("{tip} {base} refs/heads/main\n"),
        format!("{tip} {advanced} refs/heads/main\n"),
    )
}

/// Carrying the reference out of the resolution must not give the guard a
/// baseline it never resolved. A pairing that names no reference at all leaves
/// the guard inert in both directions, and the rewind that the two tests above
/// refuse is permitted here instead.
///
/// Both directions, because the refusal for an unresolvable upstream is only
/// worth having if it stays off this state. A guard that refused here would have
/// collapsed "tracks nothing" into "tracks something unreadable" from the other
/// side, which is the same conflation read backwards.
#[test]
fn a_pairing_naming_no_reference_stays_inert_for_a_rewind_and_an_append() {
    let rows: [Row; 2] = [
        ("inert-no-pairing", |fixture| fixture.add_origin_remote()),
        ("inert-no-merge", |fixture| {
            fixture.add_origin_remote();
            fixture.expect(&["config", "--local", "branch.main.remote", "origin"]);
        }),
    ];

    for (label, configure) in rows {
        let fixture = Fixture::history(label);
        configure(&fixture);
        let (rewind, append) = rewind_and_append(&fixture);
        apply(fixture.path(), OsStr::new(PREPARED), &rewind)
            .unwrap_or_else(|error| panic!("{label} tracks no published history: {error}"));
        apply(fixture.path(), OsStr::new(PREPARED), &append)
            .unwrap_or_else(|error| panic!("{label} gives up nothing to append: {error}"));
    }

    // Without this the passes above are indistinguishable from a guard that
    // stopped refusing anything: the same queue, in the same fixture shape,
    // must still be refused once a baseline exists.
    let fixture = Fixture::history("inert-control");
    fixture.publish_current_tip();
    let (rewind, _) = rewind_and_append(&fixture);
    apply(fixture.path(), OsStr::new(PREPARED), &rewind)
        .expect_err("the control must still be refused once the tip is published");
}

/// A pairing whose reference resolves to no commit establishes nothing about
/// published history, so a rewind under it is unknown rather than cleared.
///
/// Permitting it would report exactly what this guard reports after checking an
/// upstream and finding the rewind clear, which is the one answer it must not
/// give for a question it never asked.
#[test]
fn a_rewind_under_an_unresolvable_upstream_is_refused_and_names_its_correction() {
    let fixture = Fixture::history("unjudgeable-rewind");
    fixture.add_origin_remote();
    fixture.track("origin", "refs/heads/main");
    let (rewind, _) = rewind_and_append(&fixture);

    let error = apply(fixture.path(), OsStr::new(PREPARED), &rewind)
        .expect_err("an upstream that resolves to no commit cannot clear a rewind");

    let reported = error.to_string();
    assert!(
        reported.contains("refs/remotes/origin/main"),
        "the refusal must name the reference it could not resolve, or the \
         operator cannot tell which pairing to correct: {reported}"
    );
    assert!(
        reported.contains("Fetch that remote")
            && reported.contains("correct the branch's remote and merge pairing"),
        "the refusal must name both corrections, because an unfetched remote \
         and a misnamed pairing produce the same unresolvable reference: \
         {reported}"
    );
}

/// A lane whose remote is merely unfetched keeps working.
///
/// An append gives up no history whatever the upstream turns out to be, so the
/// upstream it could not read never mattered to it. The rewind in the same
/// fixture is the control: without it, this passing append is equally consistent
/// with a guard that resolved the upstream after all.
#[test]
fn an_append_under_an_unresolvable_upstream_is_still_permitted() {
    let fixture = Fixture::history("unjudgeable-append");
    fixture.add_origin_remote();
    fixture.track("origin", "refs/heads/main");
    let (rewind, append) = rewind_and_append(&fixture);

    apply(fixture.path(), OsStr::new(PREPARED), &append)
        .expect("an append gives up no history, so its upstream never had to be read");

    // Named, not merely an error: an operational failure to read this fixture
    // would also be an error here, and would leave the append's pass proving
    // nothing about which upstream state it ran under.
    let error = apply(fixture.path(), OsStr::new(PREPARED), &rewind)
        .expect_err("the same unresolvable upstream must still refuse the rewind");
    assert!(
        matches!(error, ReferenceTransactionError::UnjudgeableUpstream { .. }),
        "the control must be refused for the unresolvable upstream itself: {error}"
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
