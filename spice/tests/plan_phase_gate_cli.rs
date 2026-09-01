#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use gix::bstr::BStr;
use gix::object::tree::EntryKind;
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

        let opening = commit_all(&root);

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

    fn run_pre_commit(&self) -> Output {
        fs::write(self.root.join("implementation.txt"), "implementation\n")
            .expect("write attempted implementation");
        stage_all(&self.root);
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["gate", "pre-commit"])
            .current_dir(&self.root)
            .env(wire::SpiceVariable::Lane.as_str(), self.lane.to_string())
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT)
            .output()
            .expect("run plan-phase pre-commit gate")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pre_commit_gate_captures_the_plan_claim_and_preserves_the_staged_attempt() {
    let fixture = Fixture::new();
    let output = fixture.run_pre_commit();
    let rendered = streams(&output);
    assert!(
        !output.status.success(),
        "plan-phase gate unexpectedly succeeded: {rendered}"
    );
    assert!(rendered.contains("git commit refused"), "{rendered}");
    assert!(rendered.contains(fixture.task.as_str()), "{rendered}");
    assert_eq!(head(&fixture.root), fixture.opening);
    assert_eq!(
        staged_blob(&fixture.root, "implementation.txt"),
        b"implementation\n",
        "the refusing gate must leave the attempted implementation staged",
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

fn commit_all(root: &Path) -> String {
    let repository = open_repository(root);
    let tree = snapshot_tree(&repository, root);
    let identity = gix::actor::SignatureRef {
        name: "Spice Test".into(),
        email: "spice@example.invalid".into(),
        time: "0 +0000",
    };
    let commit = repository
        .commit_as(
            identity,
            identity,
            "refs/heads/main",
            "fixture opening",
            tree,
            gix::commit::NO_PARENT_IDS,
        )
        .expect("commit in-process plan fixture")
        .detach()
        .to_string();
    write_index(&repository, tree);
    commit
}

fn stage_all(root: &Path) {
    let repository = open_repository(root);
    let tree = snapshot_tree(&repository, root);
    write_index(&repository, tree);
}

fn snapshot_tree(repository: &gix::Repository, root: &Path) -> gix::ObjectId {
    write_tree(repository, root, root)
}

fn write_tree(repository: &gix::Repository, root: &Path, directory: &Path) -> gix::ObjectId {
    let mut entries = fs::read_dir(directory)
        .expect("read plan fixture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read plan fixture entries");
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut tree_entries = entries
        .into_iter()
        .filter(|entry| directory != root || entry.file_name() != ".git")
        .map(|entry| {
            let path = entry.path();
            let (kind, oid) = if entry
                .file_type()
                .expect("inspect plan fixture entry")
                .is_dir()
            {
                (EntryKind::Tree, write_tree(repository, root, &path))
            } else {
                let blob = repository
                    .write_blob(fs::read(&path).expect("read plan fixture file"))
                    .expect("write plan fixture blob");
                (EntryKind::Blob, blob.detach())
            };
            gix::objs::tree::Entry {
                mode: kind.into(),
                filename: entry
                    .file_name()
                    .to_str()
                    .expect("plan fixture paths are UTF-8")
                    .into(),
                oid,
            }
        })
        .collect::<Vec<_>>();
    tree_entries.sort();
    repository
        .write_object(gix::objs::Tree {
            entries: tree_entries,
        })
        .expect("write plan fixture tree")
        .detach()
}

fn write_index(repository: &gix::Repository, tree: gix::ObjectId) {
    let mut index = repository
        .index_from_tree(&tree)
        .expect("derive plan fixture index from tree");
    index.set_path(repository.index_path());
    index
        .write(Default::default())
        .expect("write plan fixture index");
}

fn open_repository(root: &Path) -> gix::Repository {
    gix::open_opts(root, gix::open::Options::isolated()).expect("open in-process plan repository")
}

fn head(root: &Path) -> String {
    open_repository(root)
        .head_id()
        .expect("plan fixture HEAD resolves")
        .detach()
        .to_string()
}

fn staged_blob(root: &Path, path: &str) -> Vec<u8> {
    let repository = open_repository(root);
    let index = repository.index().expect("open plan fixture index");
    let entry = index
        .entry_index_by_path(BStr::new(path.as_bytes()))
        .ok()
        .map(|position| index.entry(position))
        .expect("plan fixture path is staged");
    repository
        .find_blob(entry.id)
        .expect("read staged plan fixture blob")
        .data
        .to_vec()
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
