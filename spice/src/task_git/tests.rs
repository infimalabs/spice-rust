use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use wire::{
    Actor, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Instant, LeaseHead, Phase, Seq,
    TaskHead, TaskStamp, WorkflowHead,
};

struct RepositoryFixture {
    root: PathBuf,
    repository: Repository,
    opening: GitBoundary,
}

impl RepositoryFixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-task-git-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create task Git fixture");
        git(&root, &["init", "-b", "main"]);
        fs::write(root.join("tracked.txt"), "opening\n").expect("write tracked fixture");
        git(&root, &["add", "tracked.txt"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Spice Test",
                "-c",
                "user.email=spice@example.invalid",
                "commit",
                "-m",
                "fixture opening",
            ],
        );
        let repository = Repository::discover(&root).expect("discover task Git fixture");
        let head = required_read(&root, &["rev-parse", "HEAD"], "read fixture HEAD")
            .expect("fixture HEAD reads");
        let opening = GitBoundary {
            head: head.clone(),
            integrated: head,
            upstream: None,
            upstream_head: None,
            local_commits: 0,
        };
        Self {
            root,
            repository,
            opening,
        }
    }

    fn active_entry(&self, phase: &str) -> (board::Entry, Actor, Instant) {
        let lane = uuid(1);
        let occupant = uuid(2);
        let epoch = uuid(3);
        let actor = Actor::Lane(lane);
        let phase: Phase = phase.parse().expect("fixture phase");
        let born = TaskStamp::for_micros(1).expect("fixture task stamp");
        let statement = TaskHead {
            born,
            queue_origin: seq(1),
            title: "Exercise a task Git boundary".to_owned(),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "task.phase".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: "task:phase-boundary-fixture"
                .parse()
                .expect("fixture provenance"),
            creation: wire::body::TaskCreationContext::unavailable(
                wire::body::TaskCreationUnavailable::SpiceActor,
            ),
            revision: seq(1),
        };
        let workflow = WorkflowHead {
            flow: ["plan", "todo", "review"]
                .map(|value| value.parse().expect("fixture workflow phase"))
                .to_vec(),
            phase: phase.clone(),
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
        };
        let claim = ClaimHead {
            actor: actor.clone(),
            fact: seq(2),
            claimed: seq(2),
            cell: ClaimCell {
                project: "task.phase".to_owned(),
                phase,
            },
            lane,
            occupant,
            epoch,
            recovery: None,
            disposition: ClaimDisposition::Active,
        };
        let start = instant("2030-04-12T18:44:00.000Z");
        let now = instant("2030-04-12T18:44:01.000Z");
        let lease = LeaseHead {
            start,
            renewal: start,
            expiry: instant("2030-04-12T18:45:00.000Z"),
            maximum: instant("2030-04-12T19:44:00.000Z"),
            epoch,
        };
        let projects = board::ProjectPolicy::new(Vec::new(), Vec::new(), 2, 3)
            .expect("fixture project policy");
        let entry = board::Entry::new(statement, workflow, Some(claim), Some(lease), &projects)
            .expect("fixture task entry admits");
        (entry, actor, now)
    }
}

impl Drop for RepositoryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = run(root, arguments).expect("launch fixture Git");
    assert!(
        output.success(),
        "git {arguments:?} failed: stdout={} stderr={}",
        output.stdout,
        output.stderr,
    );
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture sequence")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("fixture instant")
}

fn metadata(phase: &str) -> Metadata<'_> {
    Metadata {
        label: "PHASE-1kRLWYyD",
        title: "Exercise plan boundaries",
        project: "task.phase",
        phase,
        actor: "lane:018f82d4-6f59-7d06-92e3-000000000001",
    }
}

#[test]
fn plan_publication_refuses_real_file_and_history_mutations() {
    let fixture = RepositoryFixture::new("plan-refusals");
    let (entry, actor, now) = fixture.active_entry("plan");
    let active = board::Active::admit(&entry, &actor, now).expect("plan claim is active");

    fs::write(fixture.root.join("tracked.txt"), "implementation\n")
        .expect("modify tracked fixture");
    let tracked = publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&fixture.opening),
    )
    .expect_err("tracked implementation dirt must refuse plan completion");
    assert_eq!(tracked.reason(), "git::worktree-dirty");
    assert!(tracked.detail().contains("tracked.txt"), "{tracked}");

    fs::write(fixture.root.join("tracked.txt"), "opening\n").expect("restore tracked fixture");
    fs::write(fixture.root.join("untracked.txt"), "implementation\n")
        .expect("write untracked fixture");
    let untracked = publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&fixture.opening),
    )
    .expect_err("untracked implementation dirt must refuse plan completion");
    assert_eq!(untracked.reason(), "git::worktree-dirty");
    assert!(untracked.detail().contains("untracked.txt"), "{untracked}");
    fs::remove_file(fixture.root.join("untracked.txt")).expect("remove untracked fixture");

    publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&fixture.opening),
    )
    .expect("the exact clean plan opening may complete");

    fs::write(
        fixture.root.join("tracked.txt"),
        "committed implementation\n",
    )
    .expect("modify committed fixture");
    git(&fixture.root, &["add", "tracked.txt"]);
    git(
        &fixture.root,
        &[
            "-c",
            "user.name=Spice Test",
            "-c",
            "user.email=spice@example.invalid",
            "commit",
            "--no-verify",
            "-m",
            "implementation during plan",
        ],
    );
    let committed = publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&fixture.opening),
    )
    .expect_err("a bypassed implementation commit must refuse plan completion");
    assert_eq!(committed.reason(), "git::plan-implementation");
    assert!(
        committed.detail().contains("lane HEAD moved"),
        "{committed}"
    );
}

#[test]
fn plan_completion_admits_an_exact_peer_integrated_upstream_advance() {
    let fixture = RepositoryFixture::new("plan-upstream-advance");
    configure_peer_integrations(&fixture.root);
    let opening = prepare_claim(&fixture.repository).expect("capture plan opening boundary");
    publish_peer_integrations(&fixture.root);
    git(&fixture.root, &["fetch", "origin"]);
    git(&fixture.root, &["reset", "--hard", "origin/main"]);
    let integrated = required_read(
        &fixture.root,
        &["rev-parse", "HEAD"],
        "read integrated head",
    )
    .expect("integrated head reads");

    let (entry, actor, now) = fixture.active_entry("plan");
    let active = board::Active::admit(&entry, &actor, now).expect("plan claim is active");
    let completed = publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&opening),
    )
    .expect("exact peer-integrated upstream advance completes");

    assert_eq!(completed.head, integrated);
    assert_eq!(completed.integrated, integrated);
    assert_eq!(
        required_read(
            &fixture.root,
            &["rev-parse", "HEAD"],
            "recheck integrated head"
        )
        .expect("integrated head remains readable"),
        integrated,
    );

    commit_local_plan_implementation(&fixture.root);
    let refused = publish_claimed(
        &fixture.repository,
        metadata("plan"),
        &active,
        Some(&opening),
    )
    .expect_err("a local descendant beyond current upstream must refuse");
    assert_eq!(refused.reason(), "git::plan-implementation");
    assert!(refused.detail().contains("current upstream"), "{refused}");
}

fn configure_peer_integrations(root: &Path) {
    let remote = root.join(".git/fixture-origin.git");
    let remote = remote.to_str().expect("fixture remote path is UTF-8");
    git(root, &["init", "--bare", remote]);
    git(root, &["remote", "add", "origin", remote]);
    git(root, &["push", "-u", "origin", "main"]);
}

fn publish_peer_integrations(root: &Path) {
    let remote = root.join(".git/fixture-origin.git");
    let peer = root.join(".git/fixture-peer");
    let remote = remote.to_str().expect("fixture remote path is UTF-8");
    let peer_text = peer.to_str().expect("fixture peer path is UTF-8");
    git(root, &["clone", remote, peer_text]);
    git(&peer, &["config", "user.name", "Peer Test"]);
    git(&peer, &["config", "user.email", "peer@example.invalid"]);
    fs::write(peer.join("tracked.txt"), "peer integrated\n").expect("write peer change");
    git(&peer, &["add", "tracked.txt"]);
    git(&peer, &["commit", "-m", "peer integration"]);
    fs::write(peer.join("second.txt"), "second peer integration\n")
        .expect("write second peer change");
    git(&peer, &["add", "second.txt"]);
    git(&peer, &["commit", "-m", "second peer integration"]);
    git(&peer, &["push", "origin", "main"]);
}

fn commit_local_plan_implementation(root: &Path) {
    fs::write(root.join("local.txt"), "plan implementation\n")
        .expect("write local plan implementation");
    git(root, &["add", "local.txt"]);
    git(
        root,
        &[
            "-c",
            "user.name=Spice Test",
            "-c",
            "user.email=spice@example.invalid",
            "commit",
            "--no-verify",
            "-m",
            "local plan implementation",
        ],
    );
}

fn commit_task_change(root: &Path) -> String {
    fs::write(root.join("task.txt"), "task change\n").expect("write task change");
    git(root, &["add", "task.txt"]);
    git(
        root,
        &[
            "-c",
            "user.name=Spice Test",
            "-c",
            "user.email=spice@example.invalid",
            "commit",
            "--no-verify",
            "-m",
            "task change",
        ],
    );
    required_read(root, &["rev-parse", "HEAD"], "read task commit").expect("task commit reads")
}

#[cfg(unix)]
fn install_rejecting_branch_hook(root: &Path) -> PathBuf {
    let marker = root.join("reference-hook-ran");
    let hook = root.join(".git/hooks/reference-transaction");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nwhile read old new reference; do\n  case \"$reference\" in\n    refs/heads/*) printf invoked > '{}'; exit 1;;\n  esac\ndone\n",
            marker.display(),
        ),
    )
    .expect("write rejecting reference hook");
    let mut permissions = fs::metadata(&hook)
        .expect("read reference hook metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hook, permissions).expect("make reference hook executable");
    marker
}

#[cfg(unix)]
#[test]
fn synthesized_todo_and_review_landings_are_stable_and_hook_free() {
    for phase in ["todo", "review"] {
        let fixture = RepositoryFixture::new(&format!("hook-free-{phase}"));
        configure_peer_integrations(&fixture.root);
        let upstream = required_read(
            &fixture.root,
            &["rev-parse", "origin/main"],
            "read publication baseline",
        )
        .expect("publication baseline reads");
        let agent = commit_task_change(&fixture.root);
        let marker = install_rejecting_branch_hook(&fixture.root);
        let (entry, actor, now) = fixture.active_entry(phase);
        let active = board::Active::admit(&entry, &actor, now).expect("claim is active");

        let first = publish_claimed(&fixture.repository, metadata(phase), &active, None)
            .unwrap_or_else(|error| panic!("first {phase} publication failed: {error}"));
        assert_eq!(
            merge_first_parent(&fixture.root, &first.integrated)
                .expect("read first landing parent"),
            Some(upstream.clone()),
        );
        assert_eq!(
            merge_second_parent(&fixture.root, &first.integrated)
                .expect("read second landing parent"),
            Some(agent.clone()),
        );
        assert!(!marker.exists(), "task-owned ref update invoked the hook");

        let remote_reset = format!("{upstream}:refs/heads/main");
        git(
            &fixture.root,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "push",
                "--force",
                "origin",
                &remote_reset,
            ],
        );
        git(
            &fixture.root,
            &["-c", "core.hooksPath=/dev/null", "reset", "--hard", &agent],
        );

        let second = publish_claimed(&fixture.repository, metadata(phase), &active, None)
            .unwrap_or_else(|error| panic!("retried {phase} publication failed: {error}"));
        assert_eq!(second.integrated, first.integrated);
        assert!(!marker.exists(), "retried ref update invoked the hook");
    }
}

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spice-task-git-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).expect("create scratch directory");
        Self(path)
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn completion_fetches_when_bare_head_names_the_remote_tracking_branch() {
    let scratch = ScratchDirectory::new("bare-head-fetch");
    let seed = scratch.0.join("seed");
    let remote = scratch.0.join("origin.git");
    let common = scratch.0.join("common.git");
    let lane = scratch.0.join("lane");
    let seed_text = seed.to_str().expect("seed path is UTF-8");
    let remote_text = remote.to_str().expect("remote path is UTF-8");
    let common_text = common.to_str().expect("common path is UTF-8");
    let lane_text = lane.to_str().expect("lane path is UTF-8");

    git(&scratch.0, &["init", "-b", "main", seed_text]);
    git(&seed, &["config", "user.name", "Spice Test"]);
    git(&seed, &["config", "user.email", "spice@example.invalid"]);
    fs::write(seed.join("tracked.txt"), "opening\n").expect("write opening file");
    git(&seed, &["add", "tracked.txt"]);
    git(&seed, &["commit", "--no-verify", "-m", "opening"]);
    git(&scratch.0, &["init", "--bare", remote_text]);
    git(&seed, &["remote", "add", "origin", remote_text]);
    git(&seed, &["push", "-u", "origin", "main"]);

    git(&scratch.0, &["init", "--bare", common_text]);
    git(&common, &["remote", "add", "origin", remote_text]);
    git(&common, &["fetch", "origin"]);
    git(&common, &["branch", "lane", "origin/main"]);
    git(&common, &["worktree", "add", lane_text, "lane"]);
    git(&lane, &["config", "branch.lane.remote", "origin"]);
    git(&lane, &["config", "branch.lane.merge", "refs/heads/main"]);

    fs::write(seed.join("tracked.txt"), "arrived\n").expect("write arriving change");
    git(&seed, &["add", "tracked.txt"]);
    git(&seed, &["commit", "--no-verify", "-m", "arriving change"]);
    git(&seed, &["push", "origin", "main"]);
    let arrived = required_read(&seed, &["rev-parse", "HEAD"], "read arriving commit")
        .expect("arriving commit reads");

    git(
        &common,
        &["symbolic-ref", "HEAD", "refs/remotes/origin/main"],
    );
    let repository = Repository::discover(&lane).expect("discover linked worktree");
    let boundary = prepare_claim(&repository).expect("bare-HEAD completion fetch succeeds");

    assert_eq!(boundary.integrated, arrived);
    assert_eq!(
        required_read(&lane, &["rev-parse", "HEAD"], "read advanced lane")
            .expect("advanced lane reads"),
        arrived,
    );
    assert_eq!(
        required_read(&common, &["symbolic-ref", "HEAD"], "read bare HEAD")
            .expect("bare HEAD reads"),
        "refs/remotes/origin/main",
    );
}

#[test]
fn agent_resolution_reuses_merge_head_without_fetching_again() {
    let fixture = RepositoryFixture::new("resolution-no-fetch");
    configure_peer_integrations(&fixture.root);
    let upstream = required_read(
        &fixture.root,
        &["rev-parse", "origin/main"],
        "read resolved merge target",
    )
    .expect("resolved merge target reads");
    fs::rename(
        fixture.root.join(".git/fixture-origin.git"),
        fixture.root.join(".git/unavailable-origin.git"),
    )
    .expect("make origin unavailable");

    let (_, resolved) = prepare_completion_target(
        &fixture.root,
        "todo",
        None,
        &fixture.opening.head,
        Some(&upstream),
    )
    .expect("agent resolution does not fetch")
    .expect("resolution retains its configured publication target");

    assert_eq!(resolved, upstream);
}

#[test]
fn publication_race_rebuilds_once_against_the_fetched_origin_identity() {
    let fixture = RepositoryFixture::new("moving-origin");
    configure_peer_integrations(&fixture.root);
    let stale = required_read(
        &fixture.root,
        &["rev-parse", "origin/main"],
        "read stale publication baseline",
    )
    .expect("stale baseline reads");
    let agent = commit_task_change(&fixture.root);
    let message = compose_message(metadata("todo"));
    let admission = Admission::resolve(&fixture.repository, &agent).expect("resolve admission");
    let initial = integrate(&fixture.root, &agent, &stale, &message, "PHASE-1kRLWYyD")
        .expect("synthesize initial landing");

    publish_peer_integrations(&fixture.root);
    let remote = fixture.root.join(".git/fixture-origin.git");
    let fresh = required_read(&remote, &["rev-parse", "main"], "read moved origin")
        .expect("moved origin reads");
    let target = Target {
        remote: "origin".to_owned(),
        baseline: "origin/main".to_owned(),
        branch: "main".to_owned(),
    };

    let (integrated, published) = publish_with_retry(
        &fixture.root,
        &target,
        initial,
        stale,
        Publication {
            repository: &fixture.repository,
            agent_head: &agent,
            message: &message,
            label: "PHASE-1kRLWYyD",
            admission: &admission,
        },
    )
    .expect("moving-origin publication retries");

    assert_eq!(published, fresh);
    assert_eq!(
        merge_first_parent(&fixture.root, &integrated).expect("read raced first parent"),
        Some(fresh),
    );
    assert_eq!(
        required_read(&remote, &["rev-parse", "main"], "read published origin")
            .expect("published origin reads"),
        integrated,
    );
}

#[test]
fn clean_implementation_and_review_publication_remain_admitted() {
    for phase in ["todo", "review"] {
        let fixture = RepositoryFixture::new(phase);
        let (entry, actor, now) = fixture.active_entry(phase);
        let active = board::Active::admit(&entry, &actor, now).expect("control claim is active");
        let boundary = publish_claimed(&fixture.repository, metadata(phase), &active, None)
            .unwrap_or_else(|error| panic!("clean {phase} publication was refused: {error}"));
        assert_eq!(boundary.integrated, fixture.opening.integrated);
    }
}

#[test]
fn plan_boundary_predicate_retains_its_direct_failure_classification() {
    let boundary = GitBoundary {
        head: "opening-head".to_owned(),
        integrated: "opening-head".to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some("opening-head".to_owned()),
        local_commits: 0,
    };
    assert!(require_plan_opening("plan", false, Some(&boundary)).is_ok());
    let fixture = RepositoryFixture::new("plan-boundary-predicate");
    assert!(
        require_plan_synchronization(&fixture.root, "plan", Some(&boundary), "opening-head", None,)
            .is_ok()
    );

    let moved = require_plan_synchronization(
        &fixture.root,
        "plan",
        Some(&boundary),
        "implementation-head",
        None,
    )
    .expect_err("a commit made during plan must refuse completion");
    assert_eq!(moved.reason(), "git::plan-implementation");

    let absent = require_plan_opening("plan", false, None)
        .expect_err("an unmeasured plan opening must fail closed");
    assert_eq!(absent.reason(), "git::plan-implementation");
}
