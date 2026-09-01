#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use journal::{FactCause, Journal, JournalWrite};
use wire::body::{ClaimWon, SourceBoundary, TaskAdded};
use wire::{
    Actor, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Instant, LeaseHead, Producer,
    TaskHandle, TaskHead, TaskStamp, WorkflowHead,
};

#[path = "support/git_repository.rs"]
mod git_repository;

struct Fixture {
    root: PathBuf,
    lane: CanonicalUuid,
    task: TaskHandle,
    opening: String,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-plan-hook-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("src")).expect("create plan hook fixture");
        git_repository::initialize(&root);
        fs::write(
            root.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write fixture policy");
        fs::write(
            root.join("baseline.toml"),
            "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
        )
        .expect("write fixture baseline");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"plan-hook-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write fixture manifest");
        fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").expect("write fixture source");
        fs::write(root.join("implementation.txt"), "opening\n")
            .expect("write implementation fixture");

        let hook = root.join(".githooks/pre-commit");
        fs::create_dir_all(hook.parent().expect("hook has parent")).expect("create hook directory");
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\nexec {} gate pre-commit\n",
                shell_quote(env!("CARGO_BIN_EXE_spice")),
            ),
        )
        .expect("write fixture pre-commit hook");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
            .expect("make fixture hook executable");

        git_ok(&root, &["config", "core.hooksPath", ".githooks"]);
        git_ok(&root, &["add", "."]);
        git_ok(
            &root,
            &[
                "-c",
                "user.name=Spice Test",
                "-c",
                "user.email=spice@example.invalid",
                "commit",
                "--no-verify",
                "-m",
                "fixture opening",
            ],
        );
        let opening = git_text(&root, &["rev-parse", "HEAD"]);

        let repository =
            spice::repository::Repository::discover(&root).expect("discover plan hook fixture");
        let state = repository.admit_state().expect("admit plan hook state");
        fs::create_dir_all(state.data()).expect("create journal parent");
        let lane = uuid(1);
        let task = seed_plan_claim(state.journal(), lane);
        Self {
            root,
            lane,
            task,
            opening,
        }
    }

    fn attempt_commit(&self) -> Output {
        fs::write(self.root.join("implementation.txt"), "implementation\n")
            .expect("write attempted implementation");
        git_ok(&self.root, &["add", "implementation.txt"]);
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args([
                "-c",
                "user.name=Spice Test",
                "-c",
                "user.email=spice@example.invalid",
                "commit",
                "-m",
                "implementation during plan",
            ])
            .env(wire::SpiceVariable::Lane.as_str(), self.lane.to_string())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT)
            .output()
            .expect("attempt commit through fixture hook")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn git_hook_captures_the_plan_claim_and_refuses_the_commit() {
    let fixture = Fixture::new();
    let output = fixture.attempt_commit();
    let rendered = streams(&output);
    assert!(
        !output.status.success(),
        "plan commit unexpectedly succeeded: {rendered}"
    );
    assert!(rendered.contains("git commit refused"), "{rendered}");
    assert!(rendered.contains(fixture.task.as_str()), "{rendered}");
    assert_eq!(
        git_text(&fixture.root, &["rev-parse", "HEAD"]),
        fixture.opening
    );
    assert_eq!(
        git_text(&fixture.root, &["diff", "--cached", "--name-only"]),
        "implementation.txt",
        "the refusing hook must leave the attempted implementation staged",
    );
}

fn seed_plan_claim(path: &Path, lane: CanonicalUuid) -> TaskHandle {
    let mut journal = Journal::open(path).expect("open plan hook authority");
    let task = TaskStamp::for_millis(1).expect("fixture task stamp");
    let project = "task.phase";
    let plan: wire::Phase = "plan".parse().expect("fixture plan phase");
    let origin: wire::Origin = "task:plan-hook-fixture".parse().expect("fixture origin");
    let at = instant("2030-04-12T18:44:00.000Z");
    let flow = ["plan", "todo", "review"]
        .map(|phase| phase.parse().expect("fixture workflow phase"))
        .to_vec();

    let mut add = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let added = add
        .fact(
            TaskAdded {
                task: task.clone(),
                title: "Organize implementation work".to_owned(),
                description: None,
                acceptance: vec!["implementation is delegated".to_owned()],
                project: project.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: plan.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("build fixture task fact");
    let statement_task = task.clone();
    add.task_head(added, move |revision| TaskHead {
        born: statement_task,
        queue_origin: revision,
        title: "Organize implementation work".to_owned(),
        description: None,
        acceptance: vec!["implementation is delegated".to_owned()],
        project: project.to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        revision,
    });
    let workflow_task = task.clone();
    add.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow,
        phase: plan.clone(),
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    journal.apply(add).expect("commit fixture task");

    let occupant = uuid(2);
    let epoch = uuid(3);
    let actor = Actor::Lane(lane);
    let cell = ClaimCell {
        project: project.to_owned(),
        phase: "plan".parse().expect("fixture claim phase"),
    };
    let mut claim = JournalWrite::new(
        actor.clone(),
        "task:plan-hook-fixture".parse().expect("fixture origin"),
        at,
        Producer::CommandDoor,
    );
    let won = claim
        .fact(
            ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(cell.clone()),
                until: instant("2030-04-12T19:44:00.000Z"),
                git: None,
                recovery: None,
                source: SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("build fixture claim fact");
    let claim_task = task.clone();
    claim.claim_head(claim_task, won, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    let lease_task = task.clone();
    claim.lease_head(lease_task, won, move |_| LeaseHead {
        start: at,
        renewal: at,
        expiry: instant("2030-04-12T19:44:00.000Z"),
        maximum: instant("2030-04-12T20:44:00.000Z"),
        epoch,
    });
    journal.apply(claim).expect("commit fixture plan claim");
    TaskHandle::for_project(project, &task)
}

fn git_ok(root: &Path, arguments: &[&str]) {
    let output = git(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        streams(&output)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = git(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        streams(&output)
    );
    String::from_utf8(output.stdout)
        .expect("fixture Git output is UTF-8")
        .trim()
        .to_owned()
}

fn git(root: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("launch fixture Git")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("fixture instant")
}
