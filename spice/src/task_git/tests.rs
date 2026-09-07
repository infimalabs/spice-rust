use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

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
