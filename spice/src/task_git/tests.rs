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
        git(&root, &["config", "user.name", "Spice Test"]);
        git(&root, &["config", "user.email", "spice@example.invalid"]);
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
            generation: 0,
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
            acknowledged: 0,
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
    commit_task_change_at(root, None)
}

fn commit_task_change_at(root: &Path, date: Option<&str>) -> String {
    fs::write(root.join("task.txt"), "task change\n").expect("write task change");
    git(root, &["add", "task.txt"]);
    let arguments = [
        "-c",
        "user.name=Spice Test",
        "-c",
        "user.email=spice@example.invalid",
        "commit",
        "--no-verify",
        "-m",
        "task change",
    ];
    let output = match date {
        Some(date) => run_with_environment(
            root,
            &arguments,
            &[("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)],
        )
        .expect("launch dated task commit"),
        None => run(root, &arguments).expect("launch task commit"),
    };
    assert!(
        output.success(),
        "dated task commit failed: stdout={} stderr={}",
        output.stdout,
        output.stderr,
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
    let mut landing_dates = Vec::new();
    for (phase, task_date) in [
        ("todo", "2030-04-12T18:44:01Z"),
        ("review", "2030-04-12T18:44:02Z"),
    ] {
        let fixture = RepositoryFixture::new(&format!("hook-free-{phase}"));
        configure_peer_integrations(&fixture.root);
        let upstream = required_read(
            &fixture.root,
            &["rev-parse", "origin/main"],
            "read publication baseline",
        )
        .expect("publication baseline reads");
        let agent = commit_task_change_at(&fixture.root, Some(task_date));
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
        let dates = required_read(
            &fixture.root,
            &["show", "-s", "--format=%aI%n%cI", &first.integrated],
            "read synthesized landing dates",
        )
        .expect("synthesized landing dates read");
        assert_eq!(dates, format!("{task_date}\n{task_date}"));
        landing_dates.push(dates);

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
    assert_ne!(landing_dates[0], landing_dates[1]);
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

fn lane_head(root: &Path) -> String {
    required_read(root, &["rev-parse", "HEAD"], "read fixture HEAD").expect("fixture HEAD reads")
}

fn commit_file(root: &Path, path: &str, contents: &str, message: &str) -> String {
    fs::write(root.join(path), contents).expect("write fixture change");
    git(root, &["add", path]);
    git(root, &["commit", "--no-verify", "-m", message]);
    lane_head(root)
}

/// A remote-backed lane with a committer identity, since replay recreates commits.
fn published_fixture(label: &str) -> RepositoryFixture {
    let fixture = RepositoryFixture::new(label);
    git(&fixture.root, &["config", "user.name", "Spice Test"]);
    git(
        &fixture.root,
        &["config", "user.email", "spice@example.invalid"],
    );
    configure_peer_integrations(&fixture.root);
    fixture
}

#[test]
fn release_parks_only_unpublished_work_and_claim_replays_it_onto_a_moved_upstream() {
    let fixture = published_fixture("release-replay");
    let root = &fixture.root;
    let claimed = prepare_claim(&fixture.repository).expect("claim the opening");
    publish_peer_integrations(root);
    advance_at_launch(&fixture.repository);
    let relaunched = lane_head(root);
    assert_ne!(
        relaunched, claimed.head,
        "the relaunch fast-forwards the lane"
    );

    let untouched = prepare_free(&fixture.repository, Some(&claimed))
        .expect("published history alone releases");
    assert!(untouched.is_none(), "upstream commits are not task work");
    assert_eq!(lane_head(root), relaunched);

    let task = commit_file(root, "task.txt", "task change\n", "task change");
    let released = prepare_free(&fixture.repository, Some(&claimed))
        .expect("release after a relaunch")
        .expect("the task commit parks");
    assert_eq!(released.parked().commits, [task.clone()]);
    assert_eq!(released.parked().baseline, relaunched);
    assert_eq!(lane_head(root), relaunched);

    let peer = root.join(".git/fixture-peer");
    let upstream = commit_file(&peer, "third.txt", "third\n", "third peer integration");
    git(&peer, &["push", "origin", "main"]);
    let claim = prepare_task_claim(&fixture.repository, Some(released.parked()))
        .expect("claim replays the parked commit");
    let report = claim.transplant().expect("the claim reports its replay");
    assert_eq!(report.onto, upstream);
    assert!(report.conflict.is_none() && report.present.is_empty() && report.missing.is_empty());
    let [applied] = report.applied.as_slice() else {
        panic!("one parked commit replays: {report:?}");
    };
    assert_eq!(applied.parked, task);
    assert_eq!(lane_head(root), applied.commit);
    assert_eq!(claim.boundary.head, applied.commit);
    assert_eq!(
        required_read(root, &["rev-parse", "HEAD^"], "read replay parent").expect("parent"),
        upstream,
    );
    assert_eq!(
        fs::read_to_string(root.join("task.txt")).expect("replayed file"),
        "task change\n"
    );
}

/// Parked work that already reached upstream, as the same commit or as an
/// equivalent change, is reported present and never recommitted on top of it.
#[test]
fn a_claim_does_not_duplicate_parked_commits_that_upstream_already_carries() {
    let fixture = published_fixture("replay-integrated");
    let root = &fixture.root;
    let claimed = prepare_claim(&fixture.repository).expect("claim the opening");
    let landed = commit_file(root, "landed.txt", "landed\n", "landed task change");
    let equivalent = commit_file(root, "equivalent.txt", "equivalent\n", "equivalent change");
    let released = prepare_free(&fixture.repository, Some(&claimed))
        .expect("release the task")
        .expect("both task commits park");
    assert_eq!(
        released.parked().commits,
        [landed.clone(), equivalent.clone()]
    );

    git(root, &["push", "origin", &format!("{landed}:main")]);
    let remote = root.join(".git/fixture-origin.git");
    let peer = root.join(".git/fixture-peer");
    git(
        root,
        &[
            "clone",
            remote.to_str().expect("fixture remote path is UTF-8"),
            peer.to_str().expect("fixture peer path is UTF-8"),
        ],
    );
    git(&peer, &["config", "user.name", "Peer Test"]);
    git(&peer, &["config", "user.email", "peer@example.invalid"]);
    let upstream = commit_file(&peer, "equivalent.txt", "equivalent\n", "peer re-landed");
    assert_ne!(upstream, equivalent);
    git(&peer, &["push", "origin", "main"]);

    let claim = prepare_task_claim(&fixture.repository, Some(released.parked()))
        .expect("claim replays the parked commits");
    let report = claim.transplant().expect("the claim reports its replay");
    assert_eq!(report.onto, upstream);
    assert_eq!(report.present, [landed, equivalent]);
    assert!(report.applied.is_empty() && report.conflict.is_none());
    assert_eq!(lane_head(root), upstream);
    assert_eq!(claim.boundary.local_commits, 0);
}

#[test]
fn a_conflicting_replay_stays_paused_for_the_claimant_until_the_claim_rolls_back() {
    let fixture = published_fixture("replay-conflict");
    let root = &fixture.root;
    let claimed = prepare_claim(&fixture.repository).expect("claim the opening");
    let conflicting = commit_file(root, "tracked.txt", "task edit\n", "edit tracked");
    let later = commit_file(root, "later.txt", "later\n", "later task change");
    let released = prepare_free(&fixture.repository, Some(&claimed))
        .expect("release the task")
        .expect("both task commits park");
    assert_eq!(
        released.parked().commits,
        [conflicting.clone(), later.clone()]
    );
    assert_eq!(lane_head(root), claimed.head);

    publish_peer_integrations(root);
    let claim = prepare_task_claim(&fixture.repository, Some(released.parked()))
        .expect("a conflict does not refuse the claim");
    let report = claim.transplant().expect("the claim reports its replay");
    let conflict = report.conflict.as_ref().expect("the replay stopped");
    assert_eq!(conflict.commit, conflicting);
    assert_eq!(conflict.paths, ["tracked.txt"]);
    assert_eq!(conflict.remaining, [later]);
    assert!(report.applied.is_empty());
    assert_eq!(lane_head(root), report.onto);
    assert!(
        try_read(
            root,
            &["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"]
        )
        .expect("read pick state")
        .is_some(),
        "Git's cherry-pick is left paused in the worktree",
    );

    claim.rollback().expect("a refused claim returns the lane");
    assert_eq!(lane_head(root), claimed.head);
    assert!(
        try_read(
            root,
            &["rev-parse", "--verify", "--quiet", "CHERRY_PICK_HEAD"]
        )
        .expect("read pick state")
        .is_none()
    );
    require_clean(root, "fixture check").expect("rollback leaves a clean tree");
}

#[test]
fn dirty_release_restores_uncommitted_work_after_replaying_commits() {
    let fixture = published_fixture("dirty-release");
    let root = &fixture.root;
    let opening = prepare_claim(&fixture.repository).expect("claim opening");
    let committed = commit_file(root, "committed.txt", "committed\n", "task commit");
    fs::write(root.join("tracked.txt"), "unstaged edit\n").expect("edit tracked file");
    fs::write(root.join("staged.txt"), "staged edit\n").expect("write staged file");
    git(root, &["add", "staged.txt"]);
    let binary = [0, 255, 10, 128];
    fs::write(root.join("untracked.bin"), binary).expect("write untracked bytes");
    let release = prepare_free(&fixture.repository, Some(&opening))
        .expect("dirty release succeeds")
        .expect("dirty work parks");
    let snapshot = release
        .parked()
        .snapshot
        .as_ref()
        .expect("snapshot recorded");
    assert_eq!(release.parked().commits, [committed]);
    assert_eq!(lane_head(root), opening.head);
    require_clean(root, "released lane").expect("release removes all saved dirt");
    assert_eq!(
        required_read(
            root,
            &["for-each-ref", "--contains", snapshot],
            "inspect snapshot refs"
        )
        .expect("refs"),
        ""
    );

    let peer = root.join(".git/fixture-peer");
    let remote = root.join(".git/fixture-origin.git");
    git(
        root,
        &[
            "clone",
            remote.to_str().expect("remote path"),
            peer.to_str().expect("peer path"),
        ],
    );
    git(&peer, &["config", "user.name", "Peer Test"]);
    git(&peer, &["config", "user.email", "peer@example.invalid"]);
    let upstream = commit_file(&peer, "peer.txt", "peer\n", "peer advance");
    git(&peer, &["push", "origin", "main"]);
    let claim = prepare_task_claim(&fixture.repository, Some(release.parked()))
        .expect("restore task on moved upstream");
    assert_eq!(
        claim.transplant().expect("transplant").snapshot.as_ref(),
        Some(snapshot)
    );
    assert_eq!(claim.transplant().expect("transplant").onto, upstream);
    assert_eq!(claim.boundary.local_commits, 1);
    assert_ne!(&lane_head(root), snapshot);
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("tracked file"),
        "unstaged edit\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("staged.txt")).expect("staged file"),
        "staged edit\n"
    );
    assert_eq!(
        fs::read(root.join("untracked.bin")).expect("binary file"),
        binary
    );
    assert_eq!(
        fs::read_to_string(root.join("committed.txt")).expect("committed file"),
        "committed\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("peer.txt")).expect("peer file"),
        "peer\n"
    );
    assert_eq!(
        required_read(
            root,
            &["ls-tree", "--name-only", "HEAD"],
            "inspect committed paths"
        )
        .expect("paths"),
        "committed.txt\npeer.txt\ntracked.txt"
    );
    claim
        .rollback()
        .expect("refused claim discards restoration");
    assert_eq!(lane_head(root), opening.head);
    require_clean(root, "claim rollback").expect("snapshot additions removed");
}

#[test]
fn dirty_release_rollback_preserves_original_index_and_worktree() {
    let fixture = RepositoryFixture::new("dirty-rollback");
    let root = &fixture.root;
    fs::write(root.join("tracked.txt"), "staged generation\n").expect("staged edit");
    git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "working generation\n").expect("later edit");
    fs::write(root.join("new.txt"), "untracked\n").expect("untracked file");
    let index = required_read(
        root,
        &["ls-files", "--stage"],
        "read original staged entries",
    )
    .expect("index entries");
    let before =
        required_read(root, &["status", "--porcelain"], "read original status").expect("status");
    let release = prepare_free(&fixture.repository, Some(&fixture.opening))
        .expect("prepare dirty release")
        .expect("snapshot-only release");
    assert!(release.parked().commits.is_empty());
    require_clean(root, "prepared release").expect("clean before board write");
    release
        .rollback()
        .expect("rejected journal write restores dirt");
    assert_eq!(lane_head(root), fixture.opening.head);
    assert_eq!(
        required_read(
            root,
            &["ls-files", "--stage"],
            "read restored staged entries"
        )
        .expect("index entries"),
        index
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("working file"),
        "working generation\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("new.txt")).expect("new file"),
        "untracked\n"
    );
    assert_eq!(
        required_read(root, &["status", "--porcelain"], "read rollback status").expect("status"),
        before
    );
}

#[test]
fn a_pruned_snapshot_refuses_before_advancing_the_claimant() {
    let fixture = published_fixture("missing-snapshot");
    let root = &fixture.root;
    let opening = prepare_claim(&fixture.repository).expect("claim opening");
    fs::write(root.join("draft.txt"), "draft\n").expect("draft file");
    let release = prepare_free(&fixture.repository, Some(&opening))
        .expect("release")
        .expect("parked work");
    let snapshot = release.parked().snapshot.as_ref().expect("snapshot");
    let object = git_state_path(
        root,
        &format!("objects/{}/{}", &snapshot[..2], &snapshot[2..]),
    )
    .expect("loose snapshot object");
    fs::remove_file(object).expect("prune snapshot");
    publish_peer_integrations(root);
    let index = fs::read(git_state_path(root, "index").expect("index path")).expect("index bytes");
    let error = prepare_task_claim(&fixture.repository, Some(release.parked()))
        .expect_err("missing snapshot must not lose work");
    assert_eq!(error.reason(), "git::parked-object-missing");
    assert_eq!(lane_head(root), opening.head);
    assert_eq!(
        fs::read(git_state_path(root, "index").expect("index path")).expect("index bytes"),
        index
    );
    require_clean(root, "missing-object refusal").expect("lane unchanged");
}

#[test]
fn plan_release_parks_dirt_without_admitting_implementation_history() {
    let fixture = RepositoryFixture::new("plan-dirty-release");
    let root = &fixture.root;
    fs::write(root.join("draft.txt"), "in-flight draft\n").expect("plan draft");
    let release = prepare_plan_free(&fixture.repository, Some(&fixture.opening))
        .expect("plan release")
        .expect("draft parks");
    assert_eq!(lane_head(root), fixture.opening.head);
    let claim =
        prepare_task_claim(&fixture.repository, Some(release.parked())).expect("todo restoration");
    assert_eq!(claim.boundary.local_commits, 0);
    assert_eq!(lane_head(root), fixture.opening.head);
    assert_eq!(
        fs::read_to_string(root.join("draft.txt")).expect("restored draft"),
        "in-flight draft\n"
    );
    claim.rollback().expect("restore clean plan opening");
    commit_file(
        root,
        "implementation.txt",
        "committed\n",
        "bypassed plan commit",
    );
    let error = prepare_plan_free(&fixture.repository, Some(&fixture.opening))
        .expect_err("plan history must still refuse");
    assert_eq!(error.reason(), "git::plan-implementation");
}

#[test]
fn dirty_snapshot_waits_behind_conflicted_commits_and_restores_without_a_commit() {
    let fixture = published_fixture("deferred-snapshot");
    let root = &fixture.root;
    let opening = prepare_claim(&fixture.repository).expect("claim opening");
    commit_file(
        root,
        "tracked.txt",
        "task edit\n",
        "conflicting task commit",
    );
    fs::write(root.join("draft.txt"), "uncommitted draft\n").expect("draft");
    let release = prepare_free(&fixture.repository, Some(&opening))
        .expect("release")
        .expect("parked task");
    publish_peer_integrations(root);
    let claim =
        prepare_task_claim(&fixture.repository, Some(release.parked())).expect("paused claim");
    assert_eq!(
        claim.transplant().expect("transplant").snapshot,
        release.parked().snapshot
    );
    assert!(
        !root.join("draft.txt").exists(),
        "snapshot waits for real commits"
    );
    let refusal = prepare_free(&fixture.repository, Some(&opening))
        .expect_err("paused replay cannot release");
    assert_eq!(refusal.reason(), "git::parked-replay-paused");
    fs::write(root.join("tracked.txt"), "resolved commit\n").expect("resolve committed conflict");
    git(root, &["add", "tracked.txt"]);
    git(root, &["cherry-pick", "--continue"]);
    let head = lane_head(root);
    recover_pending(root).expect("restore deferred snapshot");
    assert_eq!(lane_head(root), head);
    assert_eq!(
        fs::read_to_string(root.join("draft.txt")).expect("draft restored"),
        "uncommitted draft\n"
    );
    assert_eq!(
        required_read(root, &["show", "HEAD:draft.txt"], "inspect committed draft")
            .unwrap_err()
            .reason(),
        "git::command-failed"
    );
    assert!(!transaction_path(root).expect("marker path").exists());
}

#[test]
fn a_snapshot_conflict_remains_uncommitted_and_claim_rollback_clears_it() {
    let fixture = published_fixture("snapshot-conflict");
    let root = &fixture.root;
    let opening = prepare_claim(&fixture.repository).expect("claim opening");
    fs::write(root.join("tracked.txt"), "draft edit\n").expect("dirty tracked file");
    let release = prepare_free(&fixture.repository, Some(&opening))
        .expect("release")
        .expect("snapshot");
    publish_peer_integrations(root);
    let claim = prepare_task_claim(&fixture.repository, Some(release.parked()))
        .expect("conflicted snapshot claim");
    let report = claim.transplant().expect("snapshot report");
    let conflict = report.conflict.as_ref().expect("snapshot conflict");
    assert_eq!(Some(&conflict.commit), release.parked().snapshot.as_ref());
    assert_eq!(conflict.paths, ["tracked.txt"]);
    assert_eq!(lane_head(root), report.onto);
    assert_eq!(claim.boundary.local_commits, 0);
    assert_eq!(
        prepare_free(&fixture.repository, Some(&opening))
            .expect_err("unresolved snapshot cannot park")
            .reason(),
        "git::worktree-dirty"
    );
    claim.rollback().expect("undo refused snapshot claim");
    assert_eq!(lane_head(root), opening.head);
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("original file"),
        "opening\n"
    );
    require_clean(root, "snapshot rollback").expect("unmerged stages cleared");
}

#[cfg(unix)]
#[test]
fn a_ref_hook_refusal_restores_dirty_work_and_the_staged_generation() {
    let fixture = RepositoryFixture::new("snapshot-hook-refusal");
    let root = &fixture.root;
    let head = commit_file(root, "committed.txt", "task commit\n", "task commit");
    fs::write(root.join("tracked.txt"), "staged generation\n").expect("staged edit");
    git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "working generation\n").expect("working edit");
    fs::write(root.join("new.bin"), [255, 0, 128]).expect("untracked bytes");
    let index =
        required_read(root, &["ls-files", "--stage"], "original staged entries").expect("entries");
    let hooks = git_state_path(root, "hooks").expect("hooks path");
    git(
        root,
        &[
            "config",
            "core.hooksPath",
            hooks.to_str().expect("hook path"),
        ],
    );
    let hook = hooks.join("reference-transaction");
    fs::write(&hook, "#!/bin/sh\n[ \"$1\" != prepared ]\n").expect("refusal hook");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("executable hook");
    let error = prepare_free(&fixture.repository, Some(&fixture.opening))
        .expect_err("reference hook refuses release");
    assert_eq!(error.reason(), "git::ref-update-refused");
    assert_eq!(lane_head(root), head);
    assert_eq!(
        required_read(root, &["ls-files", "--stage"], "restored staged entries").expect("entries"),
        index
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("working edit"),
        "working generation\n"
    );
    assert_eq!(
        fs::read(root.join("new.bin")).expect("untracked bytes"),
        [255, 0, 128]
    );
    assert!(!transaction_path(root).expect("transaction path").exists());
}

#[test]
fn explicit_recovery_restore_is_byte_exact_retired_and_refuses_drift() {
    let fixture = RepositoryFixture::new("explicit-recovery-restore");
    let root = &fixture.root;
    fs::write(root.join("tracked.txt"), "staged generation\n").expect("staged edit");
    git(root, &["add", "tracked.txt"]);
    let index_path = git_state_path(root, "index").expect("index path");
    let expected_index = fs::read(&index_path).expect("original index");
    fs::write(root.join("tracked.txt"), "working generation\n").expect("working edit");
    fs::write(root.join("untracked.bin"), [0, 255, 128]).expect("untracked bytes");

    let recovery = quarantine_for_recovery(&fixture.repository).expect("quarantine generation");
    synchronize_recovery(&fixture.repository, &recovery).expect("synchronize generation");
    fs::write(root.join("concurrent.txt"), "foreign dirt\n").expect("concurrent dirt");
    let refusal =
        restore_recovery(&fixture.repository, &recovery).expect_err("drift must refuse restore");
    assert_eq!(refusal.reason(), "git::recovery-rollback-drift");
    assert_eq!(
        pending_recovery(&fixture.repository)
            .expect("retained quarantine")
            .as_ref()
            .map(Recovery::transaction),
        Some(recovery.transaction()),
    );

    fs::remove_file(root.join("concurrent.txt")).expect("remove concurrent dirt");
    restore_recovery(&fixture.repository, &recovery).expect("restore original generation");
    assert_eq!(
        fs::read(&index_path).expect("restored index"),
        expected_index
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("restored working edit"),
        "working generation\n",
    );
    assert_eq!(
        fs::read(root.join("untracked.bin")).expect("restored untracked bytes"),
        [0, 255, 128],
    );
    assert!(
        pending_recovery(&fixture.repository)
            .expect("retired quarantine")
            .is_none(),
    );
    assert!(
        restore_recovery(&fixture.repository, &recovery).is_err(),
        "retired transaction cannot restore twice",
    );
    assert_eq!(
        fs::read(&index_path).expect("index remains exact"),
        expected_index
    );
    assert!(
        pending_recovery(&fixture.repository)
            .expect("retired quarantine remains absent")
            .is_none(),
    );
}
