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

mod behavior;
mod locking;
