use super::*;

use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-task-recovery-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create recovery scratch");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove recovery scratch");
    }
}

struct Fixture {
    _scratch: Scratch,
    agent: PathBuf,
    peer: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let remote = scratch.0.join("remote.git");
        git(&scratch.0, &["init", "--bare", "-b", "main", path(&remote)]);
        let agent = scratch.0.join("agent");
        git(&scratch.0, &["init", "-b", "main", path(&agent)]);
        identity(&agent);
        fs::write(agent.join("README.md"), "base\n").expect("write recovery base");
        git(&agent, &["add", "README.md"]);
        git(&agent, &["commit", "-m", "base"]);
        git(&agent, &["remote", "add", "origin", path(&remote)]);
        git(&agent, &["push", "-u", "origin", "main"]);
        git(&agent, &["remote", "set-head", "origin", "--auto"]);
        let peer = scratch.0.join("peer");
        git(&scratch.0, &["clone", path(&remote), path(&peer)]);
        identity(&peer);
        Self {
            _scratch: scratch,
            agent,
            peer,
        }
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.agent).expect("discover recovery repository")
    }

    fn peer_commit(&self, name: &str, content: &str) -> String {
        fs::write(self.peer.join(name), content).expect("write peer path");
        git(&self.peer, &["add", name]);
        git(&self.peer, &["commit", "-m", "peer work"]);
        git(&self.peer, &["push", "origin", "main"]);
        stdout(&self.peer, &["rev-parse", "HEAD"])
    }

    fn agent_commit(&self, name: &str, content: &str) -> String {
        fs::write(self.agent.join(name), content).expect("write agent path");
        git(&self.agent, &["add", name]);
        git(&self.agent, &["commit", "-m", "agent work"]);
        stdout(&self.agent, &["rev-parse", "HEAD"])
    }
}

#[test]
fn dirty_recovery_replays_every_layer_after_synchronization() {
    let fixture = Fixture::new("dirty-recovery");
    seed_recovery_baseline(&fixture);
    let original = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    fs::write(fixture.agent.join("README.md"), "staged\n").expect("write staged content");
    git(&fixture.agent, &["add", "README.md"]);
    fs::write(fixture.agent.join("README.md"), "unstaged\n").expect("write unstaged content");
    fs::remove_file(fixture.agent.join("delete.txt")).expect("delete tracked file");
    let executable = fixture.agent.join("script.sh");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make script executable");
    git(&fixture.agent, &["add", "script.sh"]);
    fs::write(fixture.agent.join("staged.txt"), "new staged\n").expect("write staged file");
    git(&fixture.agent, &["add", "staged.txt"]);
    fs::write(fixture.agent.join("untracked.bin"), [0, 1, 2, 255])
        .expect("write binary untracked file");
    fs::set_permissions(
        fixture.agent.join("untracked.bin"),
        fs::Permissions::from_mode(0o751),
    )
    .expect("set untracked mode");
    std::os::unix::fs::symlink("README.md", fixture.agent.join("untracked-link"))
        .expect("write untracked symlink");
    let before = stdout(&fixture.agent, &["status", "--porcelain=v1"]);
    let upstream = fixture.peer_commit("peer.txt", "peer\n");

    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine dirty lane");

    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain=v1"]), "");
    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), original);
    let boundary = synchronize_recovery(&fixture.repository(), &recovery)
        .expect("synchronize quarantined lane");
    assert_eq!(boundary.head, upstream);
    replay_recovery(&fixture.repository(), &recovery).expect("replay every dirty layer");

    assert_eq!(
        stdout(&fixture.agent, &["status", "--porcelain=v1"]),
        before
    );
    assert_eq!(stdout(&fixture.agent, &["show", ":README.md"]), "staged");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("README.md")).expect("read replayed worktree"),
        "unstaged\n"
    );
    assert!(!fixture.agent.join("delete.txt").exists());
    assert_eq!(
        fs::metadata(&executable)
            .expect("read executable mode")
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    assert_eq!(
        fs::read(fixture.agent.join("untracked.bin")).expect("read untracked bytes"),
        [0, 1, 2, 255]
    );
    assert_eq!(
        fs::metadata(fixture.agent.join("untracked.bin"))
            .expect("read untracked mode")
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert_eq!(
        fs::read_link(fixture.agent.join("untracked-link")).expect("read replayed symlink"),
        Path::new("README.md")
    );
    assert!(
        pending_recovery(&fixture.repository())
            .expect("inspect retired recovery")
            .is_none()
    );
}

#[test]
fn preclaim_restore_recovers_the_exact_original_dirty_generation() {
    let fixture = Fixture::new("dirty-rollback");
    fs::write(fixture.agent.join("README.md"), "staged\n").expect("write staged content");
    git(&fixture.agent, &["add", "README.md"]);
    fs::write(fixture.agent.join("README.md"), "unstaged\n").expect("write unstaged content");
    fs::write(fixture.agent.join("loose.txt"), "loose\n").expect("write untracked content");
    let original = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    let before = stdout(&fixture.agent, &["status", "--porcelain=v1"]);
    fixture.peer_commit("peer.txt", "peer\n");

    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine dirty lane");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize lane");
    restore_recovery(&fixture.repository(), &recovery).expect("restore preclaim generation");

    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), original);
    assert_eq!(
        stdout(&fixture.agent, &["status", "--porcelain=v1"]),
        before
    );
    assert_eq!(stdout(&fixture.agent, &["show", ":README.md"]), "staged");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("README.md")).expect("read restored worktree"),
        "unstaged\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.agent.join("loose.txt")).expect("read restored untracked"),
        "loose\n"
    );
}

#[test]
fn untracked_collision_refuses_without_overwriting_either_copy() {
    let fixture = Fixture::new("dirty-collision");
    fs::write(fixture.agent.join("collision.txt"), "handoff\n").expect("write untracked handoff");
    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine collision");
    fixture.peer_commit("collision.txt", "baseline\n");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize collision lane");

    let error = replay_recovery(&fixture.repository(), &recovery)
        .expect_err("baseline collision refuses replay");

    assert_eq!(error.reason(), "git::recovery-untracked-collision");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("collision.txt")).expect("read baseline copy"),
        "baseline\n"
    );
    assert!(
        pending_recovery(&fixture.repository())
            .expect("inspect retained collision")
            .is_some()
    );
    restore_recovery(&fixture.repository(), &recovery).expect("restore collision handoff");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("collision.txt")).expect("read handoff copy"),
        "handoff\n"
    );
}

#[test]
fn nested_repository_refuses_before_any_dirty_path_is_cleaned() {
    let fixture = Fixture::new("dirty-nested");
    let nested = fixture.agent.join("nested");
    git(&fixture.agent, &["init", path(&nested)]);
    fs::write(nested.join("payload.txt"), "nested\n").expect("write nested payload");
    let before = stdout(&fixture.agent, &["status", "--porcelain=v1"]);

    let error = quarantine_for_recovery(&fixture.repository())
        .expect_err("nested repository is unsupported");

    assert_eq!(error.reason(), "git::recovery-nested-repository");
    assert_eq!(
        stdout(&fixture.agent, &["status", "--porcelain=v1"]),
        before
    );
    assert!(nested.join("payload.txt").exists());
    assert!(
        pending_recovery(&fixture.repository())
            .expect("inspect absent recovery")
            .is_none()
    );
}

#[test]
fn capture_cas_refuses_moved_bytes_without_cleaning_and_rollback_is_exact() {
    let fixture = Fixture::new("dirty-cas");
    fs::write(fixture.agent.join("moving.txt"), "sealed\n").expect("write sealed bytes");
    let recovery = capture_recovery_for_test(&fixture.repository()).expect("seal dirty generation");
    fs::write(fixture.agent.join("moving.txt"), "moved\n").expect("move sealed bytes");

    let error = finish_recovery_capture_for_test(&fixture.repository(), &recovery)
        .expect_err("capture CAS refuses moved bytes");

    assert_eq!(error.reason(), "git::recovery-snapshot-moved");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("moving.txt")).expect("read uncleaned movement"),
        "moved\n"
    );
    restore_recovery(&fixture.repository(), &recovery).expect("restore sealed bytes after refusal");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("moving.txt")).expect("read restored sealed bytes"),
        "sealed\n"
    );
}

#[test]
fn configuration_movement_refuses_before_cleaning_until_the_seal_is_restored() {
    let fixture = Fixture::new("dirty-config-cas");
    fs::write(fixture.agent.join("dirty.txt"), "dirty\n").expect("write dirty bytes");
    let recovery = capture_recovery_for_test(&fixture.repository()).expect("seal dirty generation");
    git(&fixture.agent, &["config", "recovery.moved", "true"]);

    let error = finish_recovery_capture_for_test(&fixture.repository(), &recovery)
        .expect_err("configuration CAS refuses movement");

    assert_eq!(error.reason(), "git::recovery-snapshot-moved");
    assert!(fixture.agent.join("dirty.txt").exists());
    git(&fixture.agent, &["config", "--unset", "recovery.moved"]);
    restore_recovery(&fixture.repository(), &recovery)
        .expect("restored configuration admits rollback");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("dirty.txt")).expect("read restored dirty bytes"),
        "dirty\n"
    );
}

#[test]
fn unsupported_fifo_refuses_before_cleaning_any_other_path() {
    let fixture = Fixture::new("dirty-fifo");
    let fifo = fixture.agent.join("handoff.fifo");
    let status = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("launch mkfifo fixture");
    assert!(status.success(), "mkfifo fixture failed");
    fs::write(fixture.agent.join("ordinary.txt"), "ordinary\n").expect("write ordinary dirty path");

    let error = quarantine_for_recovery(&fixture.repository())
        .expect_err("special file cannot enter quarantine");

    assert_eq!(error.reason(), "git::recovery-file-kind");
    assert!(fifo.exists());
    assert_eq!(
        fs::read_to_string(fixture.agent.join("ordinary.txt")).expect("read untouched path"),
        "ordinary\n"
    );
}

#[test]
fn recovery_source_has_no_broad_git_cleaning_primitive() {
    let source = concat!(
        include_str!("recovery.rs"),
        include_str!("recovery/storage.rs")
    );

    for forbidden in ["\"stash\"", "\"clean\",", "\"reset\", \"--hard\""] {
        assert!(
            !source.contains(forbidden),
            "recovery source contains forbidden broad primitive {forbidden:?}",
        );
    }
    assert!(
        source.contains("\"restore\".to_owned()") && source.contains("\"--\".to_owned()"),
        "tracked cleanup is not visibly path-delimited",
    );
}

#[test]
fn rollback_restores_exact_index_flags_from_the_sealed_index_bytes() {
    let fixture = Fixture::new("dirty-index-flags");
    git(
        &fixture.agent,
        &["update-index", "--assume-unchanged", "README.md"],
    );
    let before = stdout(&fixture.agent, &["ls-files", "-v", "README.md"]);
    fs::write(fixture.agent.join("loose.txt"), "loose\n").expect("write recovery trigger");

    let recovery =
        quarantine_for_recovery(&fixture.repository()).expect("quarantine flagged index");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize flagged index");
    restore_recovery(&fixture.repository(), &recovery).expect("restore flagged index");

    assert_eq!(
        stdout(&fixture.agent, &["ls-files", "-v", "README.md"]),
        before
    );
    assert_eq!(
        fs::read_to_string(fixture.agent.join("loose.txt")).expect("read restored trigger"),
        "loose\n"
    );
}

#[test]
fn restart_promotes_one_fsynced_manifest_publication_idempotently() {
    let fixture = Fixture::new("dirty-manifest-restart");
    fs::write(fixture.agent.join("loose.txt"), "loose\n").expect("write recovery trigger");
    let recovery = capture_recovery_for_test(&fixture.repository()).expect("capture manifest");
    let root = fixture.repository().git_dir().join("spice-task-recovery");
    let manifest = root.join("manifest.json");
    let temporary = root.join("manifest.recovery-tmp");
    fs::write(
        &temporary,
        fs::read(&manifest).expect("read durable manifest"),
    )
    .expect("stage durable manifest publication");
    fs::remove_file(&manifest).expect("simulate crash before manifest rename");

    let resumed = pending_recovery(&fixture.repository())
        .expect("recover staged manifest")
        .expect("recovery remains pending");

    assert_eq!(resumed, recovery);
    assert!(manifest.is_file());
    assert!(!temporary.exists());
    restore_recovery(&fixture.repository(), &recovery).expect("restore after manifest restart");
}

#[test]
fn restart_resumes_from_durable_replay_intent() {
    let fixture = Fixture::new("dirty-replay-restart");
    fs::write(fixture.agent.join("staged.txt"), "staged\n").expect("write staged recovery");
    git(&fixture.agent, &["add", "staged.txt"]);
    fs::write(fixture.agent.join("loose.txt"), "loose\n").expect("write loose recovery");
    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine replay");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize replay");
    journal_replaying_for_test(&fixture.repository(), &recovery)
        .expect("journal interrupted replay intent");
    fs::write(fixture.agent.join("staged.txt"), "partial staged\n")
        .expect("simulate partial staged replay");
    git(&fixture.agent, &["add", "staged.txt"]);
    fs::write(fixture.agent.join("loose.txt"), "partial loose\n")
        .expect("simulate partial untracked replay");

    replay_recovery(&fixture.repository(), &recovery).expect("resume replay intent");

    assert_eq!(stdout(&fixture.agent, &["show", ":staged.txt"]), "staged");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("loose.txt")).expect("read resumed loose file"),
        "loose\n"
    );
    assert!(pending_recovery(&fixture.repository()).unwrap().is_none());
}

#[test]
fn replay_restart_refuses_unsealed_drift_without_touching_it() {
    let fixture = Fixture::new("dirty-replay-restart-drift");
    fs::write(fixture.agent.join("handoff.txt"), "handoff\n").expect("write handoff");
    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine replay");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize replay");
    journal_replaying_for_test(&fixture.repository(), &recovery)
        .expect("journal interrupted replay intent");
    fs::write(fixture.agent.join("intruder.txt"), "intruder\n").expect("write concurrent drift");

    let error = replay_recovery(&fixture.repository(), &recovery)
        .expect_err("unsealed replay drift refuses restart");

    assert_eq!(error.reason(), "git::recovery-replay-drift");
    assert_eq!(
        fs::read_to_string(fixture.agent.join("intruder.txt")).expect("read preserved drift"),
        "intruder\n"
    );
    assert!(pending_recovery(&fixture.repository()).unwrap().is_some());
}

#[test]
fn tracked_replay_conflict_keeps_complete_index_stages_and_quarantine() {
    let fixture = Fixture::new("dirty-replay-conflict");
    fs::write(fixture.agent.join("README.md"), "handoff\n").expect("write handoff conflict");
    git(&fixture.agent, &["add", "README.md"]);
    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine conflict");
    fixture.peer_commit("README.md", "baseline\n");
    synchronize_recovery(&fixture.repository(), &recovery).expect("synchronize conflict");

    let error = replay_recovery(&fixture.repository(), &recovery)
        .expect_err("overlapping tracked replay conflicts");

    assert_eq!(error.reason(), "git::recovery-replay-conflict");
    assert!(
        stdout(&fixture.agent, &["status", "--short"])
            .lines()
            .any(|line| line == "UU README.md")
    );
    assert_eq!(
        stdout(&fixture.agent, &["ls-files", "--unmerged", "README.md"])
            .lines()
            .count(),
        3
    );
    assert!(pending_recovery(&fixture.repository()).unwrap().is_some());
}

#[test]
fn loose_commit_refusal_leaves_the_quarantine_rollback_exact() {
    let fixture = Fixture::new("dirty-loose-refusal");
    let loose = fixture.agent_commit("committed.txt", "committed\n");
    fs::write(fixture.agent.join("handoff.txt"), "handoff\n").expect("write dirty handoff");
    let before = stdout(&fixture.agent, &["status", "--porcelain=v1"]);
    let recovery = quarantine_for_recovery(&fixture.repository()).expect("quarantine loose lane");

    let error = synchronize_recovery(&fixture.repository(), &recovery)
        .expect_err("ordinary loose-commit boundary refuses recovery");

    assert_eq!(error.reason(), "git::claim-loose-commits");
    restore_recovery(&fixture.repository(), &recovery).expect("restore refused recovery");
    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), loose);
    assert_eq!(
        stdout(&fixture.agent, &["status", "--porcelain=v1"]),
        before
    );
}

fn seed_recovery_baseline(fixture: &Fixture) {
    fs::write(fixture.agent.join("delete.txt"), "delete me\n").expect("write delete base");
    fs::write(fixture.agent.join("script.sh"), "#!/bin/sh\n").expect("write script base");
    git(&fixture.agent, &["add", "delete.txt", "script.sh"]);
    git(&fixture.agent, &["commit", "-m", "recovery base"]);
    git(&fixture.agent, &["push", "origin", "main"]);
    git(&fixture.peer, &["pull", "--ff-only", "origin", "main"]);
}

fn path(path: &Path) -> &str {
    path.to_str().expect("fixture path is UTF-8")
}

fn identity(root: &Path) {
    git(root, &["config", "user.name", "Spice Task Recovery"]);
    git(root, &["config", "user.email", "recovery@example.test"]);
}

fn stdout(root: &Path, args: &[&str]) -> String {
    let output = test_git(root, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git(root: &Path, args: &[&str]) {
    let output = test_git(root, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_KEY_0")
        .env_remove("GIT_CONFIG_VALUE_0")
        .env_remove("GIT_CONFIG_KEY_1")
        .env_remove("GIT_CONFIG_VALUE_1")
        .output()
        .expect("launch recovery fixture Git")
}
