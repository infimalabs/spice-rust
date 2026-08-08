use super::*;

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt as _;
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-task-git-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create task Git scratch");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove task Git scratch");
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
        let remote = scratch.path().join("remote.git");
        git(
            scratch.path(),
            &["init", "--bare", "-b", "main", path(&remote)],
        );
        let agent = scratch.path().join("agent");
        git(scratch.path(), &["init", "-b", "main", path(&agent)]);
        identity(&agent);
        fs::write(agent.join("README.md"), "base\n").expect("write base");
        git(&agent, &["add", "README.md"]);
        git(&agent, &["commit", "-m", "base"]);
        git(&agent, &["remote", "add", "origin", path(&remote)]);
        git(&agent, &["push", "-u", "origin", "main"]);
        git(&agent, &["remote", "set-head", "origin", "--auto"]);
        let peer = scratch.path().join("peer");
        git(scratch.path(), &["clone", path(&remote), path(&peer)]);
        identity(&peer);
        Self {
            _scratch: scratch,
            agent,
            peer,
        }
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.agent).expect("discover agent repository")
    }

    fn agent_commit(&self, name: &str, content: &str) -> String {
        fs::write(self.agent.join(name), content).expect("write agent path");
        git(&self.agent, &["add", name]);
        git(&self.agent, &["commit", "-m", "agent work"]);
        stdout(&self.agent, &["rev-parse", "HEAD"])
    }

    fn peer_commit(&self, name: &str, content: &str) -> String {
        fs::write(self.peer.join(name), content).expect("write peer path");
        git(&self.peer, &["add", name]);
        git(&self.peer, &["commit", "-m", "peer work"]);
        git(&self.peer, &["push", "origin", "main"]);
        stdout(&self.peer, &["rev-parse", "HEAD"])
    }
}

fn metadata() -> Metadata<'static> {
    Metadata {
        label: "GIT-1kBoundary",
        title: "Publish lane work",
        project: "rim.task.git",
        phase: "todo",
        actor: "lane-fixture",
    }
}

#[test]
fn publication_builds_baseline_first_merge_and_pushes_it() {
    let fixture = Fixture::new("publish");
    let agent = fixture.agent_commit("agent.txt", "agent\n");
    let upstream = fixture.peer_commit("peer.txt", "peer\n");

    let result = publish(&fixture.repository(), metadata()).expect("publish task work");

    assert_eq!(result.head, agent);
    assert_eq!(result.upstream_head.as_deref(), Some(upstream.as_str()));
    assert_eq!(
        parents(&fixture.agent, &result.integrated),
        vec![upstream, agent]
    );
    assert_eq!(
        stdout(&fixture.agent, &["ls-remote", "origin", "refs/heads/main"])
            .split_whitespace()
            .next(),
        Some(result.integrated.as_str())
    );
    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain"]), "");
    assert_eq!(stdout(&fixture.agent, &["show", "HEAD:peer.txt"]), "peer");
}

#[test]
fn retry_after_published_merge_recovers_original_lane_head() {
    let fixture = Fixture::new("published-retry");
    let agent = fixture.agent_commit("agent.txt", "agent\n");
    let upstream = fixture.peer_commit("peer.txt", "peer\n");
    let first = publish(&fixture.repository(), metadata()).expect("first publication");

    let retried =
        publish(&fixture.repository(), metadata()).expect("retry published task transition");

    assert_eq!(retried.head, agent);
    assert_eq!(retried.integrated, first.integrated);
    assert_eq!(
        parents(&fixture.agent, &retried.integrated),
        vec![upstream, agent],
    );
}

#[test]
fn capture_preserves_loose_head_and_claim_preparation_refuses_it() {
    let fixture = Fixture::new("capture");
    let loose = fixture.agent_commit("loose.txt", "loose\n");

    let refusal = prepare_claim(&fixture.repository()).expect_err("loose claim refused");
    let captured = capture(&fixture.repository()).expect("capture loose commit");

    assert_eq!(refusal.reason(), "git::claim-loose-commits");
    assert_eq!(captured.boundary.head, loose);
    assert_eq!(captured.boundary.integrated, loose);
    assert_eq!(captured.boundary.local_commits, 1);
    assert_eq!(captured.subject, "agent work");
    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), loose);
}

#[test]
fn free_preparation_names_every_commit_a_release_would_strand() {
    let fixture = Fixture::new("free-loose");
    let first = fixture.agent_commit("first.txt", "first\n");
    let second = fixture.agent_commit("second.txt", "second\n");

    let refusal = prepare_free(&fixture.repository(), "CLAIM-1kFixture")
        .expect_err("release refused while commits are loose");

    assert_eq!(refusal.reason(), "git::free-loose-commits");
    for commit in [&first, &second] {
        let short = stdout(&fixture.agent, &["rev-parse", "--short", commit]);
        assert!(
            refusal.detail().contains(&short),
            "{} omits {short}",
            refusal.detail()
        );
    }
    assert!(
        refusal.remedy().contains("spice task done CLAIM-1kFixture"),
        "{}",
        refusal.remedy()
    );
    assert_eq!(
        stdout(&fixture.agent, &["rev-parse", "HEAD"]),
        second,
        "refusing a release must not move the lane"
    );
}

#[test]
fn claim_ignores_release_payloads_but_refuses_other_untracked_files() {
    let fixture = Fixture::new("release-output");
    fs::write(
        fixture.agent.join(".gitignore"),
        include_str!("../../../.gitignore"),
    )
    .expect("write repository ignore policy");
    git(&fixture.agent, &["add", ".gitignore"]);
    git(&fixture.agent, &["commit", "-m", "add ignore policy"]);
    git(&fixture.agent, &["push", "origin", "main"]);
    let object = fixture.agent.join("dist/stage/objects/1f/2e");
    let target = fixture
        .agent
        .join("dist/stage/targets/aarch64-apple-darwin/payload.json");
    fs::create_dir_all(object.parent().expect("object parent"))
        .expect("create staged object directory");
    fs::create_dir_all(target.parent().expect("target parent"))
        .expect("create staged target directory");
    fs::write(&object, "object\n").expect("write staged object");
    fs::write(&target, "payload\n").expect("write staged target");

    prepare_claim(&fixture.repository()).expect("release payloads do not block task next");

    fs::write(fixture.agent.join("LATER.md"), "unrelated\n")
        .expect("write unrelated untracked file");
    let error = prepare_claim(&fixture.repository())
        .expect_err("unrelated untracked work blocks task next");

    assert_eq!(error.reason(), "git::worktree-dirty");
    assert!(error.detail().contains("?? LATER.md"), "{error}");
    assert!(
        !error.detail().contains("dist/stage"),
        "ignored release payload leaked into refusal: {error}"
    );
}

#[test]
fn free_preparation_permits_a_lane_carrying_no_local_commits() {
    let fixture = Fixture::new("free-clean");

    prepare_free(&fixture.repository(), "CLAIM-1kFixture")
        .expect("release permitted with nothing to strand");
}

#[test]
fn conflict_leaves_complete_recoverable_merge_state() {
    let fixture = Fixture::new("conflict");
    fs::write(fixture.agent.join("README.md"), "agent\n").expect("write agent conflict");
    git(&fixture.agent, &["add", "README.md"]);
    git(&fixture.agent, &["commit", "-m", "agent conflict"]);
    let agent = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    fs::write(fixture.peer.join("README.md"), "peer\n").expect("write peer conflict");
    git(&fixture.peer, &["add", "README.md"]);
    git(&fixture.peer, &["commit", "-m", "peer conflict"]);
    git(&fixture.peer, &["push", "origin", "main"]);
    let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);

    let error = publish(&fixture.repository(), metadata()).expect_err("conflict refuses");

    assert_eq!(error.reason(), "git::recoverable-conflict");
    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), agent);
    assert_eq!(
        stdout(&fixture.agent, &["rev-parse", "--verify", "MERGE_HEAD"]),
        upstream
    );
    assert!(stdout(&fixture.agent, &["status", "--short"]).starts_with("UU README.md"));
    assert!(
        fixture
            .agent
            .join("README.md")
            .read_to_string()
            .contains("<<<<<<<")
    );
    assert!(
        !transaction_path(&fixture.agent)
            .expect("transaction path")
            .exists()
    );
}

#[test]
fn resolved_conflict_retry_preserves_the_resolution_commit() {
    let fixture = Fixture::new("conflict-retry");
    fs::write(fixture.agent.join("README.md"), "agent\n").expect("write agent conflict");
    git(&fixture.agent, &["add", "README.md"]);
    git(&fixture.agent, &["commit", "-m", "agent conflict"]);
    let agent = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    fs::write(fixture.peer.join("README.md"), "peer\n").expect("write peer conflict");
    git(&fixture.peer, &["add", "README.md"]);
    git(&fixture.peer, &["commit", "-m", "peer conflict"]);
    git(&fixture.peer, &["push", "origin", "main"]);
    let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);
    publish(&fixture.repository(), metadata()).expect_err("conflict refuses");
    fs::write(fixture.agent.join("README.md"), "resolved\n").expect("write resolution");
    git(&fixture.agent, &["add", "README.md"]);
    git(&fixture.agent, &["commit", "--no-edit"]);
    let resolved = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    let marker = transaction_path(&fixture.agent).expect("transaction path");
    write_pending(
        &marker,
        &Pending::Conflict {
            agent,
            upstream: upstream.clone(),
            previous: BTreeMap::new(),
        },
    )
    .expect("retain post-conflict crash marker");
    recover_pending(&fixture.agent).expect("recognize committed resolution");

    let retried = publish(&fixture.repository(), metadata()).expect("publish resolved conflict");

    assert_eq!(retried.head, resolved);
    assert_eq!(
        parents(&fixture.agent, &retried.integrated),
        vec![upstream, resolved],
    );
    assert_eq!(
        stdout(&fixture.agent, &["show", "HEAD:README.md"]),
        "resolved",
    );
}

#[test]
fn reference_hook_refusal_restores_the_pre_merge_checkout() {
    let fixture = Fixture::new("hook-refusal");
    let agent = fixture.agent_commit("agent.txt", "agent\n");
    let upstream = fixture.peer_commit("peer.txt", "peer\n");
    git(&fixture.agent, &["fetch", "origin"]);
    let hook =
        git_state_path(&fixture.agent, "hooks/reference-transaction").expect("reference hook path");
    fs::write(
        &hook,
        concat!(
            "#!/bin/sh\n",
            "case \"$1\" in\n",
            "prepared) exit 1 ;;\n",
            "*) exit 0 ;;\n",
            "esac\n",
        ),
    )
    .expect("write rejecting hook");
    let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make hook executable");

    let message = compose_message(metadata(), &agent);
    let error = integrate(
        &fixture.agent,
        &agent,
        &upstream,
        &message,
        metadata().label,
    )
    .expect_err("hook refuses");

    assert_eq!(error.reason(), "git::ref-update-refused");
    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), agent);
    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain"]), "");
    assert!(!fixture.agent.join("peer.txt").exists());
    assert!(
        !transaction_path(&fixture.agent)
            .expect("transaction path")
            .exists()
    );
}

/// A hook child still exiting when Git does is not a surviving descendant.
///
/// The group is probed the instant Git exits, which is precisely when a
/// short-lived hook child is most likely still being torn down, so with no
/// grace an ordinary command faults. That is what was observed once under
/// full-workspace parallel load, on a path with no descendant of its own.
///
/// The descendant here outlives Git by a tenth of the grace, so the two
/// outcomes are not close in either direction: it is reliably still there
/// when the group is first probed, which is what makes this fail without
/// the wait, and reliably gone well inside the bound.
#[test]
fn reference_hook_descendant_still_exiting_does_not_fault() {
    let fixture = Fixture::new("hook-exiting");
    let head = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    let descendant_pid = fixture.agent.join(".git/hook-exiting-pid");
    let hook =
        git_state_path(&fixture.agent, "hooks/reference-transaction").expect("reference hook path");
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "case \"$1\" in\n",
            "prepared)\n",
            // Records itself, lingers past Git's exit, then leaves on its
            // own. Nothing signals it, so only the grace can collect it.
            "  /bin/sh -c 'printf \"%s\\n\" \"$$\" > {pid}; ",
            "sleep 0.05' &\n",
            "  while [ ! -s {pid} ]; do sleep 0.01; done\n",
            ";;\n",
            "esac\n",
            "exit 0\n",
        ),
        pid = path(&descendant_pid),
    );
    fs::write(&hook, script).expect("write exiting hook");
    let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make hook executable");

    // Deliberately no wall-clock assertion. The elapsed time here is a whole
    // Git subprocess, not the grace, so bounding it would fail on a loaded
    // machine for reasons this test is not about -- which is the very shape
    // of flake it exists to remove.
    let output = run(
        &fixture.agent,
        &["update-ref", "refs/heads/hook-exiting", &head],
    )
    .expect("a hook descendant on its way out is not a survivor");

    assert!(output.success(), "update-ref failed: {}", output.stderr);
    assert_eq!(
        stdout(&fixture.agent, &["rev-parse", "refs/heads/hook-exiting"]),
        head,
        "the reference the command was faulted out of never landed",
    );
}

#[test]
fn reference_hook_pipe_descendant_is_bounded_and_reaped() {
    let fixture = Fixture::new("hook-descendant");
    let head = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    let descendant_pid = fixture.agent.join(".git/hook-descendant-pid");
    let hook =
        git_state_path(&fixture.agent, "hooks/reference-transaction").expect("reference hook path");
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "case \"$1\" in\n",
            "prepared)\n",
            "  /bin/sh -c 'trap \"\" HUP TERM; ",
            "printf \"%s\\n\" \"$$\" > {pid}; ",
            "exec /usr/bin/tail -f /dev/null' &\n",
            "  while [ ! -s {pid} ]; do sleep 0.01; done\n",
            ";;\n",
            "esac\n",
            "exit 0\n",
        ),
        pid = path(&descendant_pid),
    );
    fs::write(&hook, script).expect("write descendant hook");
    let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make hook executable");

    let grace_started = Cell::new(None::<Instant>);
    let mut observe_grace = || grace_started.set(Some(Instant::now()));
    let error = run_observing_descendant_grace(
        &fixture.agent,
        &["update-ref", "refs/heads/hook-descendant", &head],
        &mut observe_grace,
    )
    .expect_err("a hook descendant cannot outlive Git");

    assert_eq!(error.reason(), "git::descendant-survived");
    let cleanup_elapsed = grace_started
        .get()
        .expect("the survivor passed through its production grace")
        .elapsed();
    assert!(
        cleanup_elapsed < Duration::from_secs(3),
        "task Git exceeded its descendant cleanup bound: {cleanup_elapsed:?}",
    );
    let pid = fs::read_to_string(&descendant_pid)
        .expect("read descendant pid")
        .trim()
        .parse::<i32>()
        .expect("descendant pid is numeric");
    let pid = rustix::process::Pid::from_raw(pid).expect("descendant pid is positive");
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH),
        "hook descendant remained alive after task Git returned",
    );
}

#[test]
fn restart_repairs_both_sides_of_materialize_then_cas() {
    let fixture = Fixture::new("restart");
    let expected = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    let target = fixture.agent_commit("target.txt", "target\n");
    git(
        &fixture.agent,
        &["update-ref", "refs/heads/main", &expected, &target],
    );
    let marker = transaction_path(&fixture.agent).expect("transaction path");
    write_pending(
        &marker,
        &Pending::Move {
            expected: expected.clone(),
            target: target.clone(),
        },
    )
    .expect("write interrupted transaction");
    git(&fixture.agent, &["read-tree", "--reset", "-u", &target]);

    recover_pending(&fixture.agent).expect("repair pre-CAS crash");

    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), expected);
    assert!(!fixture.agent.join("target.txt").exists());
    write_pending(
        &marker,
        &Pending::Move {
            expected: expected.clone(),
            target: target.clone(),
        },
    )
    .expect("write post-CAS transaction");
    git(
        &fixture.agent,
        &["update-ref", "refs/heads/main", &target, &expected],
    );

    recover_pending(&fixture.agent).expect("repair post-CAS crash");

    assert_eq!(stdout(&fixture.agent, &["rev-parse", "HEAD"]), target);
    assert_eq!(
        fs::read_to_string(fixture.agent.join("target.txt")).expect("read target"),
        "target\n"
    );
    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain"]), "");
}

#[test]
fn publication_race_retries_are_bounded_without_losing_commits() {
    let fixture = Fixture::new("race-bound");
    let agent = fixture.agent_commit("agent.txt", "agent\n");
    let counter = fixture.agent.join(".git/race-count");
    let hook = fixture.agent.join(".git/hooks/pre-push");
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "count=0\n",
            "[ ! -f {counter} ] || ",
            "count=$(cat {counter})\n",
            "count=$((count + 1))\n",
            "printf '%s\\n' \"$count\" > {counter}\n",
            "git -C {peer} pull --ff-only origin main\n",
            "printf 'peer %s\\n' \"$count\" > ",
            "{peer}/peer-$count.txt\n",
            "git -C {peer} add peer-$count.txt\n",
            "git -C {peer} commit -m ",
            "\"peer race $count\"\n",
            "git -C {peer} push origin main\n",
        ),
        counter = path(&counter),
        peer = path(&fixture.peer),
    );
    fs::write(&hook, script).expect("write racing pre-push hook");
    let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make hook executable");

    let error = publish(&fixture.repository(), metadata()).expect_err("race bound refuses");

    assert_eq!(
        error.reason(),
        "git::publish-race-exhausted",
        "{}",
        error.detail()
    );
    assert_eq!(
        fs::read_to_string(&counter)
            .expect("read push count")
            .trim(),
        "6"
    );
    let current = stdout(&fixture.agent, &["rev-parse", "HEAD"]);
    assert_eq!(
        stdout(&fixture.agent, &["show", &format!("{current}:agent.txt")]),
        "agent"
    );
    assert!(is_ancestor(&fixture.agent, &agent, &current).expect("agent ancestry"));
    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain"]), "");
}

/// A manifest neither side of the landing merge is wrong about.
///
/// The two tables the sides move sit far apart on purpose. Adjacent
/// insertions would conflict, and a conflict is the case Git already
/// reports; what has to be reproduced here is the merge that reports
/// nothing.
const MANIFEST_BASE: &str = concat!(
    "[workspace]\n",
    "\n",
    "[package]\n",
    "name = \"probe\"\n",
    "version = \"0.0.0\"\n",
    "edition = \"2024\"\n",
    "\n",
    "[features]\n",
    "default = []\n",
    "\n",
    "[lib]\n",
    "path = \"src/lib.rs\"\n",
    "\n",
    "[profile.dev]\n",
    "opt-level = 0\n",
);

/// The lane's history: the table is declared at the end of the manifest.
const MANIFEST_APPENDED: &str = concat!(
    "[workspace]\n",
    "\n",
    "[package]\n",
    "name = \"probe\"\n",
    "version = \"0.0.0\"\n",
    "edition = \"2024\"\n",
    "\n",
    "[features]\n",
    "default = []\n",
    "\n",
    "[lib]\n",
    "path = \"src/lib.rs\"\n",
    "\n",
    "[profile.dev]\n",
    "opt-level = 0\n",
    "\n",
    "[dev-dependencies]\n",
);

/// The baseline's history: the same table, relocated above `[features]`.
const MANIFEST_RELOCATED: &str = concat!(
    "[workspace]\n",
    "\n",
    "[package]\n",
    "name = \"probe\"\n",
    "version = \"0.0.0\"\n",
    "edition = \"2024\"\n",
    "\n",
    "[dev-dependencies]\n",
    "\n",
    "[features]\n",
    "default = []\n",
    "\n",
    "[lib]\n",
    "path = \"src/lib.rs\"\n",
    "\n",
    "[profile.dev]\n",
    "opt-level = 0\n",
);

/// A manifest that does not load at all, committed by one side alone.
const MANIFEST_DUPLICATED: &str = concat!(
    "[workspace]\n",
    "\n",
    "[package]\n",
    "name = \"probe\"\n",
    "version = \"0.0.0\"\n",
    "edition = \"2024\"\n",
    "\n",
    "[features]\n",
    "default = []\n",
    "\n",
    "[lib]\n",
    "path = \"src/lib.rs\"\n",
    "\n",
    "[profile.dev]\n",
    "opt-level = 0\n",
    "\n",
    "[features]\n",
    "extra = []\n",
);

/// The cheap load: whether Cargo can read this repository at all.
fn admission_document() -> String {
    concat!(
        "[policy]\n",
        "admission = [\"cargo\", \"metadata\", \"--no-deps\", ",
        "\"--offline\", \"--format-version\", \"1\"]\n",
    )
    .to_owned()
}

/// A repository whose baseline already carries a readable manifest.
fn manifest_fixture_with_admission(name: &str, admission: bool) -> Fixture {
    let fixture = Fixture::new(name);
    fs::create_dir(fixture.agent.join("src")).expect("create fixture package source");
    fs::write(fixture.agent.join("src/lib.rs"), "").expect("write fixture package source");
    fs::write(fixture.agent.join("Cargo.toml"), MANIFEST_BASE).expect("write fixture manifest");
    let mut tracked = vec!["add", "src/lib.rs", "Cargo.toml"];
    if admission {
        fs::write(fixture.agent.join("spice.toml"), admission_document())
            .expect("write fixture admission policy");
        tracked.push("spice.toml");
    }
    git(&fixture.agent, &tracked);
    git(&fixture.agent, &["commit", "-m", "manifest base"]);
    git(&fixture.agent, &["push", "origin", "main"]);
    git(&fixture.peer, &["pull", "--ff-only", "origin", "main"]);
    fixture
}

/// A manifest and admission command inherited by both sides of a merge.
fn manifest_fixture(name: &str) -> Fixture {
    manifest_fixture_with_admission(name, true)
}

fn remote_head(fixture: &Fixture) -> String {
    stdout(&fixture.agent, &["ls-remote", "origin", "refs/heads/main"])
        .split_whitespace()
        .next()
        .expect("remote names a branch head")
        .to_owned()
}

/// Two valid manifests whose clean merge carries one table twice.
///
/// Nothing in Git reports this: the sides edited different regions, so the
/// three-way merge succeeds and the published workspace is the first
/// artifact that cannot be read. The refusal has to come from loading the
/// merged tree, which is the only place the duplicate exists.
#[test]
fn a_clean_merge_that_duplicates_a_manifest_table_is_refused() {
    let fixture = manifest_fixture("duplicate-table");
    fixture.agent_commit("Cargo.toml", MANIFEST_APPENDED);
    let upstream = fixture.peer_commit("Cargo.toml", MANIFEST_RELOCATED);

    let error = publish(&fixture.repository(), metadata())
        .expect_err("a manifest that cannot be read refuses");

    assert_eq!(error.reason(), "git::admission-failed");
    assert!(
        error.detail().contains("duplicate key"),
        "the failing output is preserved: {}",
        error.detail()
    );
    assert!(
        error.detail().contains("cargo metadata"),
        "the failing command is preserved: {}",
        error.detail()
    );
    assert_eq!(remote_head(&fixture), upstream);
}

/// Admission reads the tree, not the paths the lane happens to have moved.
///
/// The baseline commit here is the kind no lane ever gated. The lane's own
/// change is one document, so any check keyed on what the lane touched
/// would admit this and publish a workspace that does not load.
#[test]
fn an_ungated_baseline_commit_refuses_a_lane_that_only_moved_a_document() {
    let fixture = manifest_fixture("ungated-baseline");
    let agent = fixture.agent_commit("NOTES.md", "lane work\n");
    let upstream = fixture.peer_commit("Cargo.toml", MANIFEST_DUPLICATED);

    let error =
        publish(&fixture.repository(), metadata()).expect_err("an unreadable baseline refuses");

    assert_eq!(error.reason(), "git::admission-failed");
    assert_eq!(remote_head(&fixture), upstream);
    // The integrated state is recoverable rather than discarded: the merge
    // is checked out, carries both sides, and names its second parent.
    assert_eq!(parents(&fixture.agent, "HEAD"), vec![upstream, agent]);
    assert_eq!(stdout(&fixture.agent, &["status", "--porcelain"]), "");
}

/// A command introduced by the integrated baseline governs that tree.
#[test]
fn an_ungated_baseline_can_introduce_the_command_that_refuses_it() {
    let fixture = manifest_fixture_with_admission("baseline-introduces-admission", false);
    let agent = fixture.agent_commit("NOTES.md", "lane work\n");
    fs::write(fixture.peer.join("spice.toml"), admission_document())
        .expect("write baseline admission policy");
    fs::write(fixture.peer.join("Cargo.toml"), MANIFEST_DUPLICATED)
        .expect("write unreadable baseline manifest");
    git(&fixture.peer, &["add", "spice.toml", "Cargo.toml"]);
    git(
        &fixture.peer,
        &["commit", "-m", "introduce admission with broken manifest"],
    );
    git(&fixture.peer, &["push", "origin", "main"]);
    let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);

    let error = publish(&fixture.repository(), metadata())
        .expect_err("the baseline-introduced command refuses its tree");

    assert_eq!(error.reason(), "git::admission-failed");
    assert!(error.detail().contains("duplicate key"));
    assert_eq!(remote_head(&fixture), upstream);
    assert_eq!(parents(&fixture.agent, "HEAD"), vec![upstream, agent]);
}

/// Removing policy from the arriving baseline cannot weaken the lane.
#[test]
fn an_ungated_baseline_cannot_remove_the_lanes_admission() {
    let fixture = manifest_fixture("baseline-removes-admission");
    fixture.agent_commit("NOTES.md", "lane work\n");
    fs::remove_file(fixture.peer.join("spice.toml")).expect("remove baseline admission policy");
    fs::write(fixture.peer.join("Cargo.toml"), MANIFEST_DUPLICATED)
        .expect("write unreadable baseline manifest");
    git(&fixture.peer, &["add", "--all"]);
    git(
        &fixture.peer,
        &["commit", "-m", "remove admission with broken manifest"],
    );
    git(&fixture.peer, &["push", "origin", "main"]);
    let upstream = stdout(&fixture.peer, &["rev-parse", "HEAD"]);

    let error = publish(&fixture.repository(), metadata())
        .expect_err("the lane command survives baseline removal");

    assert_eq!(error.reason(), "git::admission-failed");
    assert!(error.detail().contains("duplicate key"));
    assert_eq!(remote_head(&fixture), upstream);
}

/// A conflict is still the conflict path, not an admission refusal.
#[test]
fn a_real_conflict_keeps_its_recoverable_merge_path_under_admission() {
    let fixture = manifest_fixture("conflict-under-admission");
    fixture.agent_commit("Cargo.toml", &MANIFEST_BASE.replace("0.0.0", "0.1.0"));
    fixture.peer_commit("Cargo.toml", &MANIFEST_BASE.replace("0.0.0", "0.2.0"));

    let error =
        publish(&fixture.repository(), metadata()).expect_err("a conflicted manifest refuses");

    assert_eq!(error.reason(), "git::recoverable-conflict");
    assert!(
        fixture
            .agent
            .join("Cargo.toml")
            .read_to_string()
            .contains("<<<<<<<")
    );
}

/// The lane is not charged for bytes it authored itself.
///
/// This manifest does not load, and it is published anyway, because no
/// merge changed it: landing admits the trees nobody wrote, and the tree a
/// lane wrote is the verification gate's subject rather than this one's.
/// Were the command run unconditionally, this would refuse.
#[test]
fn a_tree_the_lane_authored_publishes_without_running_the_command() {
    let fixture = manifest_fixture("authored-tree");
    let agent = fixture.agent_commit("Cargo.toml", MANIFEST_DUPLICATED);

    let result =
        publish(&fixture.repository(), metadata()).expect("an unmerged lane tree publishes");

    assert_eq!(result.head, agent);
    assert_eq!(remote_head(&fixture), result.integrated);
}

/// A tree built by losing the publication race is read like any other.
///
/// The lane authored the tree its first push carries, so nothing runs over
/// it. The baseline that beats it to the remote introduces both the command
/// and a manifest that does not load. The replacement tree is synthesized
/// inside the retry, so both policy selection and admission must repeat.
#[test]
fn a_tree_built_by_losing_the_publication_race_is_refused() {
    let fixture = manifest_fixture_with_admission("race-admission", false);
    fixture.agent_commit("NOTES.md", "lane work\n");
    let broken = fixture.agent.join(".git/broken-manifest");
    fs::write(&broken, MANIFEST_DUPLICATED).expect("write raced manifest");
    let policy = fixture.agent.join(".git/raced-policy");
    fs::write(&policy, admission_document()).expect("write raced admission policy");
    let counter = fixture.agent.join(".git/race-count");
    let hook = fixture.agent.join(".git/hooks/pre-push");
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "[ ! -f {counter} ] || exit 0\n",
            ": > {counter}\n",
            "git -C {peer} pull --ff-only origin main\n",
            "cp {broken} {peer}/Cargo.toml\n",
            "cp {policy} {peer}/spice.toml\n",
            "git -C {peer} add Cargo.toml spice.toml\n",
            "git -C {peer} commit -m \"peer manifest\"\n",
            "git -C {peer} push origin main\n",
        ),
        counter = path(&counter),
        peer = path(&fixture.peer),
        broken = path(&broken),
        policy = path(&policy),
    );
    fs::write(&hook, script).expect("write racing pre-push hook");
    let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make hook executable");

    let error = publish(&fixture.repository(), metadata())
        .expect_err("a raced manifest that cannot be read refuses");

    assert_eq!(
        error.reason(),
        "git::admission-failed",
        "{}",
        error.detail()
    );
    assert!(
        error.detail().contains("duplicate key"),
        "the raced tree is what was read: {}",
        error.detail()
    );
}

trait ReadText {
    fn read_to_string(&self) -> String;
}

impl ReadText for PathBuf {
    fn read_to_string(&self) -> String {
        fs::read_to_string(self).expect("read text path")
    }
}

fn path(path: &Path) -> &str {
    path.to_str().expect("fixture path is UTF-8")
}

fn identity(root: &Path) {
    git(root, &["config", "user.name", "Spice Task Git"]);
    git(root, &["config", "user.email", "spice@example.test"]);
}

fn parents(root: &Path, commit: &str) -> Vec<String> {
    stdout(root, &["show", "-s", "--format=%P", commit])
        .split_whitespace()
        .map(str::to_owned)
        .collect()
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
        .expect("launch fixture Git")
}
