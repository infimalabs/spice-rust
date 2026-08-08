//! GIT-08 fixtures: what a landing is allowed to change, and what it is not.
//!
//! Every case here drives the real completion entry point rather than the
//! boundary function, because the claim being proved is that a landing is
//! refused -- not that a predicate returns true.
//!
//! Four enter through the conflict path and one through a merge the lane built
//! itself, which are the two ways a landing arrives already shaped and so the
//! two call sites the boundary has. The sixth takes neither: it is an ordinary
//! machine merge, and it is here to prove the boundary stays quiet when the
//! landing carries peer work it did not write.

use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-task-landing-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create landing scratch");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove landing scratch");
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
            &["init", "--bare", "-b", "main", text(&remote)],
        );
        let agent = scratch.path().join("agent");
        git(scratch.path(), &["init", "-b", "main", text(&agent)]);
        identity(&agent);
        fs::write(agent.join("README.md"), "base\n").expect("write base");
        git(&agent, &["add", "README.md"]);
        git(&agent, &["commit", "-m", "base"]);
        git(&agent, &["remote", "add", "origin", text(&remote)]);
        git(&agent, &["push", "-u", "origin", "main"]);
        git(&agent, &["remote", "set-head", "origin", "--auto"]);
        let peer = scratch.path().join("peer");
        git(scratch.path(), &["clone", text(&remote), text(&peer)]);
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

    fn agent_commit(&self, files: &[(&str, &str)]) -> String {
        write_all(&self.agent, files);
        git(&self.agent, &["add", "-A"]);
        git(&self.agent, &["commit", "-m", "agent work"]);
        head(&self.agent)
    }

    /// Peer work that is already on the shared branch before the lane lands.
    fn peer_commit(&self, files: &[(&str, &str)]) -> String {
        write_all(&self.peer, files);
        git(&self.peer, &["add", "-A"]);
        git(&self.peer, &["commit", "-m", "peer work"]);
        git(&self.peer, &["push", "origin", "main"]);
        head(&self.peer)
    }

    /// Commit a resolution into the merge the conflict boundary left open.
    fn resolve(&self, files: &[(&str, &str)]) -> String {
        write_all(&self.agent, files);
        git(&self.agent, &["add", "-A"]);
        git(&self.agent, &["commit", "--no-edit"]);
        head(&self.agent)
    }
}

fn write_all(root: &Path, files: &[(&str, &str)]) {
    for (name, content) in files {
        fs::write(root.join(name), content).expect("write fixture path");
    }
}

fn metadata() -> Metadata<'static> {
    Metadata {
        label: "GIT-1kLanding",
        title: "Publish lane work",
        project: "rim.task.git",
        phase: "todo",
        actor: "lane-fixture",
    }
}

/// The overlap that forces the agent to resolve by hand, which is the only way
/// a landing can carry paths the task never committed.
fn paused_at_overlap(fixture: &Fixture) {
    let refusal = publish(&fixture.repository(), metadata()).expect_err("overlap pauses");
    assert_eq!(
        refusal.reason, "git::recoverable-conflict",
        "expected the conflict boundary, got {refusal:?}"
    );
}

#[test]
fn a_hand_resolved_landing_that_rewrites_a_peer_path_is_refused() {
    let fixture = Fixture::new("rewrite");
    fixture.agent_commit(&[("shared.txt", "agent\n")]);
    fixture.peer_commit(&[("shared.txt", "peer\n"), ("peer.txt", "peer\n")]);
    paused_at_overlap(&fixture);
    fixture.resolve(&[("shared.txt", "resolved\n"), ("peer.txt", "clobbered\n")]);

    let refusal = publish(&fixture.repository(), metadata()).expect_err("drifted landing refused");

    assert_eq!(refusal.reason, "git::landing-out-of-scope");
    assert_eq!(refusal.class, ErrorClass::Refusal);
    assert!(
        refusal.detail.contains("rewrites-baseline peer.txt"),
        "the drifted path is named and classified: {}",
        refusal.detail
    );
    assert!(
        !refusal.detail.contains("shared.txt"),
        "the conflicted path is inside the footprint and must not be reported: {}",
        refusal.detail
    );
    assert!(
        refusal.remedy.contains("git checkout") && refusal.remedy.contains("peer.txt"),
        "the recipe restores the path it named: {}",
        refusal.remedy
    );
}

#[test]
fn a_hand_resolved_landing_that_invents_a_path_is_classified_as_an_addition() {
    let fixture = Fixture::new("addition");
    fixture.agent_commit(&[("shared.txt", "agent\n")]);
    fixture.peer_commit(&[("shared.txt", "peer\n")]);
    paused_at_overlap(&fixture);
    fixture.resolve(&[("shared.txt", "resolved\n"), ("invented.txt", "stray\n")]);

    let refusal = publish(&fixture.repository(), metadata()).expect_err("drifted landing refused");

    assert_eq!(refusal.reason, "git::landing-out-of-scope");
    assert!(
        refusal.detail.contains("adds-to-baseline invented.txt"),
        "a path absent from the baseline is classified as an addition: {}",
        refusal.detail
    );
    assert!(
        refusal.remedy.contains("git rm -- invented.txt"),
        "an addition is recovered by removal, not by restoring content: {}",
        refusal.remedy
    );
    assert!(
        !refusal.remedy.contains("git checkout"),
        "nothing to restore, so the restoring recipe is absent: {}",
        refusal.remedy
    );
}

#[test]
fn a_merge_the_lane_built_itself_is_checked_before_it_is_accepted() {
    let fixture = Fixture::new("prebuilt");
    let agent = fixture.agent_commit(&[("agent.txt", "agent\n")]);
    let upstream = fixture.peer_commit(&[("peer.txt", "peer\n")]);
    git(&fixture.agent, &["fetch", "origin"]);
    // A landing already shaped as a baseline-first merge, carrying a peer path
    // the lane never committed. Nothing downstream rebuilds this commit.
    write_all(&fixture.agent, &[("peer.txt", "clobbered\n")]);
    git(&fixture.agent, &["add", "-A"]);
    let tree = stdout(&fixture.agent, &["write-tree"]);
    let merge = stdout(
        &fixture.agent,
        &[
            "commit-tree",
            &tree,
            "-p",
            &upstream,
            "-p",
            &agent,
            "-m",
            "hand-built landing",
        ],
    );
    git(&fixture.agent, &["reset", "--hard", &merge]);

    let refusal = publish(&fixture.repository(), metadata()).expect_err("drifted landing refused");

    assert_eq!(refusal.reason, "git::landing-out-of-scope");
    assert!(
        refusal.detail.contains("rewrites-baseline peer.txt"),
        "the prebuilt merge is judged against the same footprint: {}",
        refusal.detail
    );
    assert!(
        !refusal.detail.contains("agent.txt"),
        "the lane's own path is inside the footprint and must not be reported: {}",
        refusal.detail
    );
}

#[test]
fn a_drifted_path_holding_a_space_is_quoted_where_the_recipe_is_pasted() {
    let fixture = Fixture::new("quoting");
    fixture.agent_commit(&[("shared.txt", "agent\n")]);
    fixture.peer_commit(&[("shared.txt", "peer\n"), ("peer notes.txt", "peer\n")]);
    paused_at_overlap(&fixture);
    fixture.resolve(&[
        ("shared.txt", "resolved\n"),
        ("peer notes.txt", "clobbered\n"),
        ("stray file.txt", "stray\n"),
    ]);

    let refusal = publish(&fixture.repository(), metadata()).expect_err("drifted landing refused");

    assert!(
        refusal.remedy.contains("-- 'peer notes.txt'")
            && refusal.remedy.contains("git rm -- 'stray file.txt'"),
        "a pasted path is one pathspec, not two: {}",
        refusal.remedy
    );
    // The listing is prose rather than a command, so it names the path bare,
    // which is also where the predecessor stops quoting.
    assert!(
        refusal.detail.contains("rewrites-baseline peer notes.txt"),
        "the listing names the path as git printed it: {}",
        refusal.detail
    );
}

#[test]
fn a_hand_resolved_landing_that_stays_inside_the_footprint_publishes() {
    let fixture = Fixture::new("control-resolved");
    fixture.agent_commit(&[("shared.txt", "agent\n")]);
    let upstream = fixture.peer_commit(&[("shared.txt", "peer\n"), ("peer.txt", "peer\n")]);
    paused_at_overlap(&fixture);
    // The same flow as the refused cases, resolving only the conflicted path.
    let resolved = fixture.resolve(&[("shared.txt", "resolved\n")]);

    let published = publish(&fixture.repository(), metadata()).expect("clean landing publishes");

    assert_eq!(published.head, resolved);
    assert_eq!(
        stdout(
            &fixture.agent,
            &["show", &format!("{}:peer.txt", published.integrated)]
        ),
        "peer",
        "the peer path landed unchanged from the baseline {upstream}"
    );
}

#[test]
fn peer_work_already_on_the_baseline_is_not_drift() {
    let fixture = Fixture::new("control-machine");
    let agent = fixture.agent_commit(&[("agent.txt", "agent\n")]);
    let upstream = fixture.peer_commit(&[("peer.txt", "peer\n")]);

    let published = publish(&fixture.repository(), metadata()).expect("machine merge publishes");

    assert_eq!(published.head, agent);
    assert_eq!(published.upstream_head.as_deref(), Some(upstream.as_str()));
    // The landing carries peer.txt, but it carries it unchanged from the
    // baseline, so it never enters the diff this boundary reads.
    assert_eq!(stdout(&fixture.agent, &["show", "HEAD:peer.txt"]), "peer");
}

fn text(path: &Path) -> &str {
    path.to_str().expect("fixture path is UTF-8")
}

fn head(root: &Path) -> String {
    stdout(root, &["rev-parse", "HEAD"])
}

fn identity(root: &Path) {
    git(root, &["config", "user.email", "lane@example.invalid"]);
    git(root, &["config", "user.name", "Lane Fixture"]);
}

fn stdout(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
}

fn git(root: &Path, args: &[&str]) {
    let _ = stdout(root, args);
}
