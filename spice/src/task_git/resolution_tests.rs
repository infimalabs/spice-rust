//! Allocator-owned conflict-resolution admission regressions.

use super::*;

use std::cell::{Cell, RefCell};
use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicU64, Ordering};

use board::{Active, Entry, ProjectPolicy};
use wire::{
    Actor, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Instant, LeaseHead, Phase, Seq,
    TaskHead, TaskStamp, WorkflowHead,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    agent: PathBuf,
    peer: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-task-resolution-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create resolution scratch");
        let remote = root.join("remote.git");
        git(&root, &["init", "--bare", "-b", "main", text(&remote)]);
        let agent = root.join("agent");
        git(&root, &["init", "-b", "main", text(&agent)]);
        identity(&agent);
        fs::create_dir_all(agent.join("gate/src")).expect("create authority parent");
        fs::write(agent.join("gate/src/change_selection.rs"), "base\n")
            .expect("write authority base");
        git(&agent, &["add", "gate/src/change_selection.rs"]);
        git(&agent, &["commit", "-m", "base"]);
        git(&agent, &["remote", "add", "origin", text(&remote)]);
        git(&agent, &["push", "-u", "origin", "main"]);
        git(&agent, &["remote", "set-head", "origin", "--auto"]);
        let peer = root.join("peer");
        git(&root, &["clone", text(&remote), text(&peer)]);
        identity(&peer);
        Self { root, agent, peer }
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.agent).expect("discover resolution repository")
    }

    fn pause(&self, meta: Metadata<'_>) -> (String, String) {
        fs::write(self.agent.join("gate/src/change_selection.rs"), "agent\n")
            .expect("write agent authority");
        git(&self.agent, &["add", "gate/src/change_selection.rs"]);
        git(&self.agent, &["commit", "-m", "agent authority"]);
        let agent = output(&self.agent, &["rev-parse", "HEAD"]);
        fs::write(self.peer.join("gate/src/change_selection.rs"), "peer\n")
            .expect("write peer authority");
        git(&self.peer, &["add", "gate/src/change_selection.rs"]);
        git(&self.peer, &["commit", "-m", "peer authority"]);
        git(&self.peer, &["push", "origin", "main"]);
        let upstream = output(&self.peer, &["rev-parse", "HEAD"]);
        let refusal = publish(&self.repository(), meta).expect_err("authority conflict pauses");
        assert_eq!(refusal.reason(), "git::recoverable-conflict");
        assert!(refusal.remedy().contains("spice task done --validation"));
        (agent, upstream)
    }

    fn resolve(&self) {
        fs::write(
            self.agent.join("gate/src/change_selection.rs"),
            "resolved\n",
        )
        .expect("write authority resolution");
        git(&self.agent, &["add", "gate/src/change_selection.rs"]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove resolution scratch");
    }
}

struct ClaimFixture {
    entry: Entry,
    actor: Actor,
    now: Instant,
}

impl ClaimFixture {
    fn new(millis: i64, disposition: ClaimDisposition) -> Self {
        let born = TaskStamp::for_millis(millis).expect("fixture task stamp");
        let phase = "todo".parse::<Phase>().expect("fixture phase");
        let actor = Actor::Lane(uuid(1));
        let statement = TaskHead {
            born: born.clone(),
            queue_origin: seq(1),
            title: "Resolve authority conflict".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "task.git".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: format!("task:{born}").parse().expect("fixture origin"),
            revision: seq(1),
        };
        let workflow = WorkflowHead {
            flow: vec![phase.clone()],
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
                project: "task.git".to_owned(),
                phase,
            },
            lane: uuid(1),
            occupant: uuid(2),
            epoch: uuid(3),
            recovery: None,
            disposition,
        };
        let lease = LeaseHead {
            start: instant("2026-08-14T20:00:00.000Z"),
            renewal: instant("2026-08-14T20:00:00.000Z"),
            expiry: instant("2026-08-14T22:00:00.000Z"),
            maximum: instant("2026-08-14T23:00:00.000Z"),
            epoch: uuid(3),
        };
        let projects =
            ProjectPolicy::new(["task".to_owned()], Vec::new(), 2, 3).expect("fixture projects");
        let entry = Entry::new(statement, workflow, Some(claim), Some(lease), &projects)
            .expect("fixture active entry");
        Self {
            entry,
            actor,
            now: instant("2026-08-14T21:00:00.000Z"),
        }
    }

    fn active(&self) -> Active<'_> {
        Active::admit(&self.entry, &self.actor, self.now).expect("active fixture claim")
    }

    fn metadata<'a>(&'a self, actor: &'a str) -> Metadata<'a> {
        Metadata {
            label: self.entry.handle().as_str(),
            title: "Resolve authority conflict",
            project: "task.git",
            phase: "todo",
            actor,
        }
    }
}

#[test]
fn allocator_owned_authority_resolution_runs_evidence_then_publishes() {
    let claim = ClaimFixture::new(1, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("success");
    let (agent, upstream) = fixture.pause(meta);
    fixture.resolve();
    let calls = Cell::new(0);
    let completion_calls = Cell::new(0);
    let completion_head = RefCell::new(String::new());
    let expected_verifier = verifier_receipt("resolved-tree");

    let (published, verifier) = publish_claimed_with_completion_evidence(
        &fixture.repository(),
        meta,
        &claim.active(),
        || {
            calls.set(calls.get() + 1);
            Ok(())
        },
        |candidate| {
            completion_calls.set(completion_calls.get() + 1);
            *completion_head.borrow_mut() = output(&fixture.agent, &["rev-parse", "HEAD"]);
            assert_eq!(candidate, completion_head.borrow().as_str());
            Ok(Some(expected_verifier.clone()))
        },
    )
    .expect("admitted resolution publishes");

    assert_eq!(calls.get(), 1);
    assert_eq!(completion_calls.get(), 1);
    assert_eq!(*completion_head.borrow(), published.integrated);
    assert_eq!(verifier, Some(expected_verifier));
    assert_eq!(published.local_commits, 1);
    assert_eq!(
        parents(&fixture.agent, &published.integrated),
        [upstream, agent]
    );
    assert_eq!(output(&fixture.agent, &["status", "--porcelain"]), "");
    assert!(
        !git_state_path(&fixture.agent, "MERGE_HEAD")
            .expect("merge path")
            .exists()
    );
}

#[test]
fn completion_evidence_refuses_the_materialized_candidate_before_push() {
    let claim = ClaimFixture::new(2, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("completion-evidence-refusal");
    let (_, upstream) = fixture.pause(meta);
    fixture.resolve();

    let refusal = publish_claimed_with_completion_evidence(
        &fixture.repository(),
        meta,
        &claim.active(),
        || Ok(()),
        |_| {
            Err(CompletionEvidenceError::refusal(
                "gate::full-verifier-completion-unavailable",
                "stage=source: retained source differs from current source",
                "run `cargo xtask verify` on this exact current tree",
            ))
        },
    )
    .expect_err("stale completion evidence refuses publication");

    assert_eq!(refusal.reason(), "git::completion-evidence-refused");
    assert!(refusal.detail().contains("stage=source"));
    assert!(refusal.remedy().contains("cargo xtask verify"));
    assert_eq!(
        output(
            &fixture.root.join("remote.git"),
            &["rev-parse", "refs/heads/main"]
        ),
        upstream,
        "the exact-tree refusal must happen before remote publication",
    );
}

#[test]
fn a_candidate_outside_the_full_verifier_tier_publishes_without_a_citation() {
    let claim = ClaimFixture::new(3, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("completion-evidence-not-applicable");
    fixture.pause(meta);
    fixture.resolve();
    let completion_calls = Cell::new(0);

    let (published, verifier) = publish_claimed_with_completion_evidence(
        &fixture.repository(),
        meta,
        &claim.active(),
        || Ok(()),
        |_| {
            completion_calls.set(completion_calls.get() + 1);
            Ok(None)
        },
    )
    .expect("generic candidate publishes without source-only evidence");

    assert_eq!(completion_calls.get(), 1);
    assert_eq!(verifier, None);
    assert_eq!(
        output(&fixture.agent, &["rev-parse", "refs/remotes/origin/main"]),
        published.integrated,
    );
}

#[test]
fn incomplete_or_tampered_merge_control_state_refuses_before_evidence() {
    let claim = ClaimFixture::new(10, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("merge-control-refusal");
    fixture.pause(meta);
    fixture.resolve();
    for (name, state, value) in [
        ("missing-head", "MERGE_HEAD", None),
        ("multiple-heads", "MERGE_HEAD", Some("bad\nbad\n")),
        ("malformed-head", "MERGE_HEAD", Some("not-an-object\n")),
        ("wrong-origin", "ORIG_HEAD", Some("bad\n")),
        ("wrong-mode", "MERGE_MODE", Some("ff\n")),
        ("wrong-message", "MERGE_MSG", Some("tampered\n")),
    ] {
        let path = git_state_path(&fixture.agent, state).expect("state path");
        let original = fs::read(&path).expect("read original merge state");
        match value {
            Some(value) => fs::write(&path, value).expect("tamper state"),
            None => fs::remove_file(&path).expect("remove state"),
        }
        let called = Cell::new(false);

        let refusal = publish_claimed(&fixture.repository(), meta, &claim.active(), || {
            called.set(true);
            Ok(())
        })
        .expect_err("tampered merge state refuses");

        assert!(!called.get(), "{name} reached evidence");
        assert!(
            matches!(
                refusal.reason(),
                "git::resolution-state-missing" | "git::resolution-state-mismatch"
            ),
            "{name}: {refusal}",
        );
        fs::write(&path, &original).expect("restore original merge state");
        assert_eq!(fs::read(path).expect("read restored merge state"), original,);
    }
}

#[test]
fn unresolved_index_and_wrong_claim_refuse_before_evidence() {
    let claim = ClaimFixture::new(100, ClaimDisposition::Active);
    let wrong = ClaimFixture::new(101, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("unresolved");
    fixture.pause(meta);

    let unresolved = publish_claimed(&fixture.repository(), meta, &claim.active(), || Ok(()))
        .expect_err("unresolved stages refuse");
    assert_eq!(unresolved.reason(), "git::resolution-unresolved");

    fixture.resolve();
    let mismatch = publish_claimed(&fixture.repository(), meta, &wrong.active(), || Ok(()))
        .expect_err("wrong claim refuses");
    assert_eq!(mismatch.reason(), "git::resolution-claim-mismatch");

    let inactive = ClaimFixture::new(102, ClaimDisposition::Freed);
    assert!(Active::admit(&inactive.entry, &inactive.actor, inactive.now).is_err());
}

#[test]
fn moved_baseline_and_out_of_footprint_resolution_refuse() {
    let claim = ClaimFixture::new(200, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("moved-baseline");
    fixture.pause(meta);
    fixture.resolve();
    fs::write(fixture.peer.join("peer.txt"), "later\n").expect("write later peer work");
    git(&fixture.peer, &["add", "peer.txt"]);
    git(&fixture.peer, &["commit", "-m", "later peer"]);
    git(&fixture.peer, &["push", "origin", "main"]);

    let moved = publish_claimed(&fixture.repository(), meta, &claim.active(), || Ok(()))
        .expect_err("moved baseline refuses stale handoff");
    assert_eq!(moved.reason(), "git::resolution-state-mismatch");

    let fixture = Fixture::new("footprint-drift");
    fixture.pause(meta);
    fixture.resolve();
    fs::write(fixture.agent.join("invented.txt"), "drift\n").expect("write drift");
    git(&fixture.agent, &["add", "invented.txt"]);
    let drift = publish_claimed(&fixture.repository(), meta, &claim.active(), || Ok(()))
        .expect_err("out-of-footprint drift refuses");
    assert_eq!(drift.reason(), "git::landing-out-of-scope");
}

#[test]
fn source_evidence_cannot_authorize_a_merge_only_peer_path_rewrite() {
    let claim = ClaimFixture::new(250, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("peer-path-authorship");
    fs::write(fixture.peer.join("peer.txt"), "peer\n").expect("write peer-owned path");
    git(&fixture.peer, &["add", "peer.txt"]);
    let (agent, _) = fixture.pause(meta);
    fixture.resolve();
    fs::write(fixture.agent.join("peer.txt"), "clobbered\n")
        .expect("rewrite non-conflicting peer path");
    git(&fixture.agent, &["add", "peer.txt"]);
    let evidence_called = Cell::new(false);

    let refusal = publish_claimed(&fixture.repository(), meta, &claim.active(), || {
        evidence_called.set(true);
        Ok(())
    })
    .expect_err("source-valid peer rewrite remains outside the task footprint");

    assert_eq!(refusal.reason(), "git::landing-out-of-scope");
    assert!(
        refusal.detail().contains("rewrites-baseline peer.txt"),
        "the refusal names the baseline path whose authorship is missing: {refusal}"
    );
    assert!(
        !evidence_called.get(),
        "tree validity cannot replace landing-path authorship"
    );
    assert_eq!(output(&fixture.agent, &["rev-parse", "HEAD"]), agent);
    assert_eq!(output(&fixture.agent, &["show", ":peer.txt"]), "clobbered");
    assert!(
        git_state_path(&fixture.agent, "MERGE_HEAD")
            .expect("merge path")
            .exists(),
        "the refused resolution remains recoverable"
    );
}

#[test]
fn post_evidence_index_ref_and_configuration_movement_refuse_before_commit() {
    let claim = ClaimFixture::new(300, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("post-evidence-movement");
    let (agent, _) = fixture.pause(meta);
    fixture.resolve();
    let before = output(&fixture.agent, &["rev-parse", "HEAD"]);
    let resolved = fixture.agent.join("gate/src/change_selection.rs");
    let resolved_bytes = fs::read(&resolved).expect("read resolved authority");
    let index = git_state_path(&fixture.agent, "index").expect("index path");
    let index_bytes = fs::read(&index).expect("read resolved index");
    let tree = output(&fixture.agent, &["write-tree"]);
    let upstream = output(&fixture.agent, &["rev-parse", "refs/remotes/origin/main"]);
    for name in ["index", "ref", "configuration"] {
        let refusal = publish_claimed(&fixture.repository(), meta, &claim.active(), || {
            match name {
                "index" => {
                    fs::write(&resolved, "moved after evidence\n").expect("move resolution index");
                    git(&fixture.agent, &["add", "gate/src/change_selection.rs"]);
                }
                "ref" => git(
                    &fixture.agent,
                    &["update-ref", "refs/remotes/origin/main", &agent],
                ),
                "configuration" => git(&fixture.agent, &["config", "resolution.moved", "yes"]),
                _ => unreachable!(),
            }
            Ok(())
        })
        .expect_err("post-evidence movement refuses");

        assert!(
            matches!(
                refusal.reason(),
                "git::resolution-state-moved" | "git::resolution-configuration-moved"
            ),
            "{name}: {refusal}",
        );
        assert_eq!(output(&fixture.agent, &["rev-parse", "HEAD"]), before);
        match name {
            "index" => {
                fs::write(&resolved, &resolved_bytes).expect("restore resolved authority");
                fs::write(&index, &index_bytes).expect("restore resolved index");
            }
            "ref" => git(
                &fixture.agent,
                &["update-ref", "refs/remotes/origin/main", &upstream],
            ),
            "configuration" => git(&fixture.agent, &["config", "--unset", "resolution.moved"]),
            _ => unreachable!(),
        }
        assert_eq!(output(&fixture.agent, &["write-tree"]), tree);
        assert_eq!(
            output(&fixture.agent, &["rev-parse", "refs/remotes/origin/main"]),
            upstream,
        );
        let configuration = std::process::Command::new("git")
            .arg("-C")
            .arg(&fixture.agent)
            .args(["config", "--get", "resolution.moved"])
            .output()
            .expect("inspect restored resolution configuration");
        assert_eq!(configuration.status.code(), Some(1));
        assert!(configuration.stdout.is_empty());
    }
}

#[test]
fn evidence_refusal_keeps_the_resolved_index_and_merge_state() {
    let claim = ClaimFixture::new(400, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("evidence-refusal");
    let (agent, _) = fixture.pause(meta);
    fixture.resolve();

    let refusal = publish_claimed(&fixture.repository(), meta, &claim.active(), || {
        Err(ResolutionEvidenceError::refusal(
            "gate::fixture-regressed",
            "fixture evidence",
            "repair fixture",
        ))
    })
    .expect_err("evidence refusal stops commit creation");

    assert_eq!(refusal.reason(), "git::resolution-evidence-refused");
    assert_eq!(output(&fixture.agent, &["rev-parse", "HEAD"]), agent);
    assert!(
        git_state_path(&fixture.agent, "MERGE_HEAD")
            .expect("merge path")
            .exists()
    );
}

#[test]
fn reference_refusal_restores_the_sealed_resolution_for_retry() {
    let claim = ClaimFixture::new(500, ClaimDisposition::Active);
    let actor = claim.actor.to_string();
    let meta = claim.metadata(&actor);
    let fixture = Fixture::new("reference-refusal");
    let (agent, _) = fixture.pause(meta);
    fixture.resolve();
    let tree = output(&fixture.agent, &["write-tree"]);
    let hook =
        git_state_path(&fixture.agent, "hooks/reference-transaction").expect("reference hook path");
    fs::write(
        &hook,
        "#!/bin/sh\ncase \"$1\" in prepared) exit 1 ;; *) exit 0 ;; esac\n",
    )
    .expect("write refusing reference hook");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
        .expect("make reference hook executable");

    let refusal = publish_claimed(&fixture.repository(), meta, &claim.active(), || Ok(()))
        .expect_err("reference hook refuses resolution update");

    assert_eq!(refusal.reason(), "git::resolution-ref-race");
    assert_eq!(output(&fixture.agent, &["rev-parse", "HEAD"]), agent);
    assert_eq!(output(&fixture.agent, &["write-tree"]), tree);
    assert!(
        git_state_path(&fixture.agent, "MERGE_HEAD")
            .expect("merge path")
            .exists(),
    );
    assert!(
        !transaction_path(&fixture.agent)
            .expect("transaction path")
            .exists(),
    );
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("nonzero sequence")
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
        .parse()
        .expect("fixture uuid")
}

fn instant(value: &str) -> Instant {
    value.parse().expect("fixture instant")
}

fn verifier_receipt(label: &str) -> wire::body::FullVerifierReceipt {
    wire::body::FullVerifierReceipt {
        tree: wire::Digest::compute(
            wire::Domain::GateReceipt,
            format!("tree-{label}").as_bytes(),
        ),
        evidence: wire::Digest::compute(
            wire::Domain::GateReceipt,
            format!("evidence-{label}").as_bytes(),
        ),
        battery: wire::Digest::compute(
            wire::Domain::GateBatteryReceipt,
            format!("battery-{label}").as_bytes(),
        ),
    }
}

fn text(path: &Path) -> &str {
    path.to_str().expect("fixture path is UTF-8")
}

fn identity(root: &Path) {
    git(root, &["config", "user.email", "lane@example.invalid"]);
    git(root, &["config", "user.name", "Lane Fixture"]);
}

fn output(root: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
}

fn git(root: &Path, arguments: &[&str]) {
    let _ = output(root, arguments);
}

fn parents(root: &Path, commit: &str) -> Vec<String> {
    output(root, &["rev-list", "--parents", "-n", "1", commit])
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect()
}
