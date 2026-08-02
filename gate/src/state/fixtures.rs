use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn bound_decision_records_the_explicit_predecessor_order() {
    let actors = ActorInputs::new("src/lib.rs").expect("actor inputs");
    let rule = BoundRule::new("file-shape", "file-loc", 10, 15, false, false).expect("valid bound");

    let breached = rule.decide(16, false, &actors, 0).expect("decision");
    assert_eq!(breached.rule(), "file-shape");
    assert_eq!(breached.metric(), "file-loc");
    assert_eq!(breached.measured(), 16);
    assert_eq!(breached.base_limit(), 10);
    assert_eq!(breached.configured_elastic_limit(), 15);
    assert_eq!(breached.variation_percent(), 0);
    assert_eq!(breached.elastic_limit(), 15);
    assert_eq!(breached.active_limit(), 15);
    assert!(!breached.unlimited());
    assert!(breached.over_limit());
    assert!(breached.over_base());
    assert!(breached.elastic_breach());
    assert_eq!(breached.latch_transition(), LatchTransition::Latched);
    assert_eq!(breached.actor_inputs(), &actors);
    assert!(breached.actor().is_none());

    let held = rule.decide(14, true, &actors, 0).expect("held decision");
    assert_eq!(held.active_limit(), 10);
    assert!(held.over_limit());
    assert!(held.over_base());
    assert!(!held.elastic_breach());
    assert_eq!(held.latch_transition(), LatchTransition::Held);

    let healed = rule.decide(10, true, &actors, 0).expect("healed decision");
    assert!(!healed.over_limit());
    assert!(!healed.over_base());
    assert!(!healed.elastic_breach());
    assert_eq!(healed.latch_transition(), LatchTransition::Healed);

    let unlimited = BoundRule::new("docs", "chars", 10, 15, true, false)
        .expect("unlimited bound")
        .decide(100, false, &actors, 0)
        .expect("unlimited decision");
    assert!(unlimited.unlimited());
    assert!(!unlimited.over_limit());
    assert!(!unlimited.over_base());
    assert!(!unlimited.elastic_breach());

    let unlimited_heal = BoundRule::new("docs", "chars", 10, 15, true, false)
        .expect("unlimited bound")
        .decide(100, true, &actors, 0)
        .expect("unlimited latched decision");
    assert_eq!(unlimited_heal.latch_transition(), LatchTransition::Healed);
}

#[test]
fn actor_order_and_blake2b_variation_match_the_predecessor() {
    let actors = ActorInputs::new("./src\\lib.rs")
        .expect("normalized path")
        .with_candidate_blob("candidate-blob")
        .with_active_actor("active")
        .with_published_commit("published-commit")
        .with_published_task_session("published-session");
    assert_eq!(actors.candidate_path(), "src/lib.rs");
    assert_eq!(
        actors.resolve_seed().source(),
        ActorSource::PublishedTaskSession
    );
    assert_eq!(actors.resolve_seed().value(), "published-session");
    assert_eq!(
        actors.require_actor().expect("required actor").value(),
        "published-session"
    );

    let no_session = ActorInputs::new("src/lib.rs")
        .expect("inputs")
        .with_published_commit("published-commit")
        .with_active_actor("active")
        .with_candidate_blob("candidate-blob");
    assert_eq!(
        no_session.resolve_seed().source(),
        ActorSource::PublishedCommit
    );
    assert_eq!(
        no_session.require_actor().expect("active actor").value(),
        "active"
    );

    let provenance_only = ActorInputs::new("src/lib.rs")
        .expect("inputs")
        .with_candidate_blob("candidate-blob");
    assert_eq!(
        provenance_only.resolve_seed().source(),
        ActorSource::CandidateBlob
    );
    assert_eq!(
        provenance_only.require_actor(),
        Err(BoundError::MissingActor)
    );
    let required = BoundRule::new("slice", "file-loc", 100, 150, false, true).expect("actor rule");
    assert_eq!(
        required.decide(120, false, &provenance_only, 10),
        Err(BoundError::MissingActor)
    );

    // Python 0.31.0: blake2b(digest_size=2) => 04b1, bucket offset -6.
    assert_eq!(
        varied_elastic_limit(100, 150, "./src\\lib.rs", "actor-a", 10)
            .expect("varied elastic limit"),
        147
    );
    assert_eq!(
        varied_elastic_limit(100, 150, "src/lib.rs", "actor-a", 10).expect("same normalized path"),
        147
    );
    // Python 0.31.0 converts the exact product through binary64 before
    // truncation: int(5059961668777554089 * 88 / 100) = 4452766268524247552.
    assert_eq!(
        varied_elastic_limit(1, 5_059_961_668_777_554_090, "src/20.rs", "actor-a", 100,)
            .expect("large predecessor rounding vector"),
        9_512_727_937_301_801_642
    );
    let varied = BoundRule::new("slice", "file-loc", 100, 150, false, false)
        .expect("varied bound")
        .decide(
            148,
            false,
            &ActorInputs::new("src/lib.rs")
                .expect("varied inputs")
                .with_active_actor("actor-a"),
            10,
        )
        .expect("varied decision");
    assert_eq!(varied.elastic_limit(), 147);
    assert!(varied.elastic_breach());
    assert_eq!(
        varied.actor().expect("selected variation actor").source(),
        ActorSource::ActiveActor
    );
}

#[test]
fn current_policy_variation_domain_reaches_canonical_state_evidence() {
    const ABOVE_U32: u64 = 4_294_967_296;
    let policy = crate::migrate_predecessor_policy(
        "fixtures/policy.toml",
        "[policy.flex]\njitter_percent = 4294967296\n",
    )
    .expect("the predecessor percentage has an exact current meaning");
    assert_eq!(policy.document().ratchet().variation_percent(), ABOVE_U32);

    let actors = ActorInputs::new("src/lib.rs")
        .expect("actor inputs")
        .with_active_actor("actor-a");
    let decision = BoundRule::new("slice", "file-loc", 100, 150, false, false)
        .expect("varied bound")
        .decide(101, false, &actors, ABOVE_U32)
        .expect("the complete policy percentage enters state evaluation");
    assert_eq!(decision.variation_percent(), ABOVE_U32);
    assert_eq!(decision.elastic_limit(), 100);
    assert_eq!(decision.active_limit(), 100);
    assert!(decision.over_limit());
    assert_eq!(
        decision.actor().expect("selected variation actor").source(),
        ActorSource::ActiveActor
    );

    let encoded = wire::encode(&decision).expect("bound decision is canonical evidence");
    let evidence: serde_json::Value =
        wire::decode(&encoded).expect("canonical decision evidence decodes");
    assert_eq!(evidence["variation_percent"], ABOVE_U32);
    assert_eq!(evidence["configured_elastic_limit"], 150);
    assert_eq!(evidence["elastic_limit"], 100);
    assert_eq!(evidence["actor"]["source"], "active-actor");

    assert_eq!(
        varied_elastic_limit(100, 150, "src/lib.rs", "actor-a", u64::MAX)
            .expect("the widest percentage clamps exactly below base"),
        100
    );
}

#[test]
fn one_shot_import_translates_all_ledgers_and_both_key_shapes() {
    let fixture = StateFixture::new();
    fixture.write_complete_predecessor();

    let state = CurrentState::load(&fixture.current, &fixture.sources).expect("strict import");
    assert!(!fixture.current.exists());
    assert_eq!(state.before().version(), STATE_LEDGER_VERSION);
    assert_eq!(state.before().latches().len(), 5);
    assert_eq!(state.before().claims().len(), 1);
    assert_eq!(state.before().claims()[0].path(), "src/old.rs");
    assert_eq!(state.before().claims()[0].actor(), "peer");
    assert!(state.before().latches().iter().any(|entry| {
        entry.kind() == LatchKind::ComplexityCcn
            && entry.key().repo_path() == "src/old.rs"
            && entry.key().function_name() == Some("parse")
    }));
    assert!(state.before().latches().iter().any(|entry| {
        entry.kind() == LatchKind::FileLoc
            && entry.key().repo_path() == "src/old.rs"
            && entry.key().function_name().is_none()
    }));

    let error = import_error(
        &fixture,
        &fixture.sources.file_loc,
        br#"{"version":2,"paths":[]}"#,
    );
    assert_eq!(error.kind(), StateErrorKind::UnsupportedVersion);
    assert_eq!(error.source_path(), fixture.sources.file_loc);
    assert_eq!(error.release(), PREDECESSOR_STATE_RELEASE);
    assert!(!error.recovery().is_empty());
}

#[test]
fn malformed_colliding_unreadable_and_inexact_predecessor_state_is_loud() {
    let malformed = StateFixture::new();
    malformed.write_complete_predecessor();
    let error = import_error(
        &malformed,
        &malformed.sources.complexity_ccn,
        br#"{"version":1,"functions":[["src/lib.rs"]]}"#,
    );
    assert_eq!(error.kind(), StateErrorKind::Decode);
    assert_eq!(error.release(), PREDECESSOR_STATE_RELEASE);

    let unknown = StateFixture::new();
    unknown.write_complete_predecessor();
    let error = import_error(
        &unknown,
        &unknown.sources.file_loc,
        br#"{"version":1,"paths":[],"unknown":true}"#,
    );
    assert_eq!(error.kind(), StateErrorKind::Decode);
    assert_eq!(error.source_path(), unknown.sources.file_loc);

    let collision = StateFixture::new();
    collision.write_complete_predecessor();
    let error = import_error(
        &collision,
        &collision.sources.file_loc,
        br#"{"version":1,"paths":["src/lib.rs","./src/lib.rs"]}"#,
    );
    assert_eq!(error.kind(), StateErrorKind::Collision);

    let unreadable = StateFixture::new();
    unreadable.write_complete_predecessor();
    fs::remove_file(&unreadable.sources.file_bytes).expect("remove source file");
    fs::create_dir(&unreadable.sources.file_bytes).expect("replace source with directory");
    let error = CurrentState::load(&unreadable.current, &unreadable.sources)
        .expect_err("directory source must not be swallowed");
    assert_eq!(error.kind(), StateErrorKind::Read);
    assert_eq!(error.source_path(), unreadable.sources.file_bytes);

    let claim_collision = StateFixture::new();
    claim_collision.write_complete_predecessor();
    let error = import_error(
        &claim_collision,
        &claim_collision.sources.claims,
        concat!(
            r#"{"version":1,"ttl_seconds":21600,"claims":["#,
            r#"{"path":"src/a.rs","actor":"peer","created_at":1.0,"expires_at":2.0},"#,
            r#"{"path":"./src\\a.rs","actor":" peer ","created_at":1.0,"expires_at":2.0}"#,
            "]}"
        )
        .as_bytes(),
    );
    assert_eq!(error.kind(), StateErrorKind::Collision);

    let inexact = StateFixture::new();
    inexact.write_complete_predecessor();
    let error = import_error(
        &inexact,
        &inexact.sources.claims,
        br#"{"version":1,"ttl_seconds":1,"claims":[]}"#,
    );
    assert_eq!(error.kind(), StateErrorKind::Malformed);
    assert!(error.to_string().contains("no exact current meaning"));
}

#[test]
fn malformed_current_state_never_falls_back_to_predecessor_state() {
    let fixture = StateFixture::new();
    fixture.write_complete_predecessor();
    fs::write(&fixture.current, br#"{"version":99}"#).expect("write current state");

    let error = CurrentState::load(&fixture.current, &fixture.sources)
        .expect_err("current state must own the failure");
    assert_eq!(error.kind(), StateErrorKind::UnsupportedVersion);
    assert_eq!(error.source_path(), fixture.current);
    assert_eq!(error.release(), CURRENT_STATE_RELEASE);
}

#[test]
fn a_complete_observation_inventory_is_required_exactly_once() {
    let fixture = StateFixture::new();
    let mut state = CurrentState::load(&fixture.current, &fixture.sources).expect("empty import");
    state
        .begin_run(&RenameMap::default(), std::iter::empty::<&str>(), 100.0)
        .expect("begin run");

    let error = state
        .clone()
        .finish()
        .expect_err("finish must require complete observations");
    assert_eq!(error.kind(), StateErrorKind::Malformed);
    state.reconcile([]).expect("empty inventory is complete");
    let error = state
        .reconcile([])
        .expect_err("a second inventory would make ordering ambiguous");
    assert_eq!(error.kind(), StateErrorKind::Malformed);
    state.finish().expect("finish after one complete inventory");
}

#[test]
fn rename_pruning_latching_healing_and_acceptance_are_one_transaction() {
    let fixture = StateFixture::new();
    fixture.write_complete_predecessor();
    let mut state = CurrentState::load(&fixture.current, &fixture.sources).expect("import");
    let before = state.before().digest();
    let renames = RenameMap::new([("src/old.rs", "src/new.rs")]).expect("rename map");
    state
        .begin_run(&renames, ["src/new.rs"], 1_000.0)
        .expect("begin fixed run");

    let held = observation(
        LatchKind::FileLoc,
        StateKey::path("src/new.rs").expect("path key"),
        12,
        "actor-a",
    );
    let decisions = state
        .reconcile([held])
        .expect("reconcile complete inventory");
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].bound().latch_transition(),
        LatchTransition::Held
    );
    let pending = state.finish().expect("finish state");
    assert_eq!(pending.after().latches().len(), 1);
    assert_eq!(pending.after().latches()[0].key().repo_path(), "src/new.rs");
    assert_eq!(pending.after().claims().len(), 1);
    assert_eq!(pending.after().claims()[0].path(), "src/new.rs");
    assert_ne!(pending.after().digest(), before);

    let rejected = pending.clone().reject();
    assert!(!rejected.accepted());
    assert!(!fixture.current.exists());
    let accepted = pending.accept().expect("accepted atomic publication");
    assert!(accepted.accepted());
    assert_eq!(accepted.before(), &before);
    assert_eq!(
        accepted.after(),
        &CurrentState::load(&fixture.current, &fixture.sources)
            .expect("load current only")
            .before()
            .digest()
    );
    fs::write(&fixture.sources.file_loc, b"not JSON")
        .expect("corrupt predecessor after accepted import");
    CurrentState::load(&fixture.current, &fixture.sources)
        .expect("current ledger permanently supersedes predecessor state");

    let mut heal = CurrentState::load(&fixture.current, &fixture.sources).expect("reload");
    let bytes_before_heal = fs::read(&fixture.current).expect("accepted bytes before heal");
    heal.begin_run(&RenameMap::default(), ["src/new.rs"], 1_001.0)
        .expect("begin heal");
    let healed = heal
        .reconcile([observation(
            LatchKind::FileLoc,
            StateKey::path("src/new.rs").expect("path key"),
            10,
            "actor-a",
        )])
        .expect("heal observation");
    assert_eq!(
        healed[0].bound().latch_transition(),
        LatchTransition::Healed
    );
    let pending_heal = heal.finish().expect("finish heal");
    let rejected_heal = pending_heal.clone().reject();
    assert!(!rejected_heal.accepted());
    assert_eq!(
        fs::read(&fixture.current).expect("state after rejected heal"),
        bytes_before_heal
    );
    let receipt = pending_heal.accept().expect("accept heal");
    assert_ne!(receipt.before(), receipt.after());
    assert!(
        CurrentState::load(&fixture.current, &fixture.sources)
            .expect("load healed")
            .before()
            .latches()
            .is_empty()
    );
}

#[test]
fn stale_linked_worktree_update_refuses_instead_of_losing_a_claim() {
    let fixture = StateFixture::new();
    let mut first = CurrentState::load(&fixture.current, &fixture.sources).expect("first load");
    let mut second = CurrentState::load(&fixture.current, &fixture.sources).expect("second load");
    for state in [&mut first, &mut second] {
        state
            .begin_run(&RenameMap::default(), ["src/live.rs"], 100.0)
            .expect("begin concurrent run");
        state.reconcile([]).expect("empty latch inventory");
    }
    first
        .claim(
            ["src/live.rs"],
            &ActorInputs::new("src/live.rs")
                .expect("first actor inputs")
                .with_active_actor("actor-a"),
        )
        .expect("first claim");
    second
        .claim(
            ["src/live.rs"],
            &ActorInputs::new("src/live.rs")
                .expect("second actor inputs")
                .with_active_actor("actor-b"),
        )
        .expect("second claim");

    first
        .finish()
        .expect("finish first")
        .accept()
        .expect("accept first");
    let error = second
        .finish()
        .expect("finish stale second")
        .accept()
        .expect_err("stale imported transaction must refuse");
    assert_eq!(error.kind(), StateErrorKind::Stale);
    let current = CurrentState::load(&fixture.current, &fixture.sources).expect("current state");
    assert_eq!(current.before().claims().len(), 1);
    assert_eq!(current.before().claims()[0].actor(), "actor-a");
}

#[test]
fn accepted_persist_failures_are_loud_and_path_owned() {
    let fixture = StateFixture::new();
    let mut state = CurrentState::load(&fixture.current, &fixture.sources).expect("empty import");
    state
        .begin_run(&RenameMap::default(), std::iter::empty::<&str>(), 100.0)
        .expect("begin empty run");
    state.reconcile([]).expect("empty observations");
    let pending = state.finish().expect("finish empty run");
    let parent = fixture.current.parent().expect("current parent");
    fs::remove_dir_all(parent).expect("remove current parent");
    fs::write(parent, b"not a directory").expect("block current parent");

    let error = pending.accept().expect_err("persist failure must surface");
    assert_eq!(error.kind(), StateErrorKind::Persist);
    assert_eq!(error.source_path(), fixture.current);
    assert!(!error.recovery().is_empty());
}

#[test]
fn claims_refresh_redirect_expire_and_share_one_git_common_ledger() {
    let fixture = StateFixture::new();
    let common_path = current_state_path(&fixture.common_git);
    assert_eq!(common_path, fixture.current);

    let first = claim_and_accept(&fixture, 100.0, "actor-a");
    assert_eq!(first.status(), ClaimStatus::Claimed);
    assert_eq!(first.claim().created_at(), 100.0);
    assert_eq!(first.claim().expires_at(), 21_700.0);

    // A second linked-worktree handle resolves the same Git-common path.
    let refreshed = claim_and_accept(&fixture, 200.0, "actor-a");
    assert_eq!(refreshed.status(), ClaimStatus::Owned);
    assert_eq!(refreshed.claim().created_at(), 100.0);
    assert_eq!(refreshed.claim().expires_at(), 21_800.0);

    let mut peer = CurrentState::load(&common_path, &fixture.sources).expect("shared load");
    peer.begin_run(&RenameMap::default(), ["src/live.rs"], 300.0)
        .expect("peer run");
    peer.reconcile([]).expect("complete empty latch inventory");
    let peer_decision = peer
        .claim(
            ["./src\\live.rs"],
            &ActorInputs::new("src/live.rs")
                .expect("peer inputs")
                .with_active_actor("actor-b"),
        )
        .expect("peer disposition")
        .remove(0);
    assert_eq!(peer_decision.status(), ClaimStatus::PeerHeld);
    assert_eq!(peer_decision.claim().actor(), "actor-a");
    let rejected = peer.finish().expect("peer finish").reject();
    assert!(!rejected.accepted());

    let expired = claim_and_accept(&fixture, 21_800.0, "actor-b");
    assert_eq!(expired.status(), ClaimStatus::Claimed);
    assert_eq!(expired.claim().actor(), "actor-b");
    assert_eq!(expired.claim().created_at(), 21_800.0);
}

#[test]
fn receipts_are_canonical_immutable_evidence_and_replay_is_deterministic() {
    let left = StateFixture::new();
    let right = StateFixture::new();
    left.write_complete_predecessor();
    right.write_complete_predecessor();

    let left_receipt = deterministic_run(&left);
    let right_receipt = deterministic_run(&right);
    assert_eq!(left_receipt.before(), right_receipt.before());
    assert_eq!(left_receipt.after(), right_receipt.after());
    assert_eq!(left_receipt.digest(), right_receipt.digest());
    let bytes = left_receipt.encode().expect("receipt bytes");
    assert_eq!(
        StateReceipt::decode(&bytes).expect("canonical receipt"),
        left_receipt
    );

    let receipt_path = left.root.join("receipt.json");
    fs::write(
        &receipt_path,
        left_receipt.encode().expect("receipt evidence bytes"),
    )
    .expect("fixture stores immutable evidence");
    let immutable = fs::read(&receipt_path).expect("read receipt evidence");
    let _ = claim_and_accept(&left, 3_000.0, "actor-a");
    assert_eq!(
        fs::read(&receipt_path).expect("receipt remains"),
        immutable,
        "state migration and later acceptance must not rewrite prior receipts"
    );
}

fn observation(kind: LatchKind, key: StateKey, measured: u64, actor: &str) -> LatchObservation {
    let path = key.repo_path().to_owned();
    LatchObservation::new(
        LatchEntry::new(kind, key).expect("matching key shape"),
        BoundRule::new("fixture-rule", "fixture-metric", 10, 15, false, false)
            .expect("fixture bound"),
        measured,
        ActorInputs::new(path)
            .expect("fixture actor inputs")
            .with_active_actor(actor),
        0,
    )
    .expect("fixture observation")
}

fn claim_and_accept(fixture: &StateFixture, now: f64, actor: &str) -> ClaimDecision {
    let mut state = CurrentState::load(&fixture.current, &fixture.sources).expect("load state");
    state
        .begin_run(&RenameMap::default(), ["src/live.rs"], now)
        .expect("begin claim run");
    state.reconcile([]).expect("complete latch inventory");
    let decision = state
        .claim(
            ["src/live.rs"],
            &ActorInputs::new("src/live.rs")
                .expect("claim inputs")
                .with_active_actor(actor),
        )
        .expect("claim path")
        .remove(0);
    state
        .finish()
        .expect("finish claim")
        .accept()
        .expect("accept claim");
    decision
}

fn deterministic_run(fixture: &StateFixture) -> StateReceipt {
    let mut state = CurrentState::load(&fixture.current, &fixture.sources).expect("import");
    state
        .begin_run(
            &RenameMap::new([("src/old.rs", "src/new.rs")]).expect("renames"),
            ["src/new.rs"],
            2_000.0,
        )
        .expect("begin deterministic run");
    state
        .reconcile([observation(
            LatchKind::FileLoc,
            StateKey::path("src/new.rs").expect("path key"),
            16,
            "actor-a",
        )])
        .expect("deterministic decision");
    state
        .finish()
        .expect("finish deterministic run")
        .accept()
        .expect("accept deterministic run")
}

fn import_error(fixture: &StateFixture, path: &Path, bytes: &[u8]) -> StateError {
    fs::write(path, bytes).expect("replace predecessor source");
    CurrentState::load(&fixture.current, &fixture.sources).expect_err("import must fail")
}

struct StateFixture {
    root: PathBuf,
    common_git: PathBuf,
    current: PathBuf,
    sources: PredecessorSources,
}

impl StateFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-state-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let common_git = root.join("common.git");
        let worktree_state = root.join("worktree.git/.spice");
        let shared_state = common_git.join(".spice");
        let current = current_state_path(&common_git);
        fs::create_dir_all(&worktree_state).expect("create worktree predecessor state");
        fs::create_dir_all(&shared_state).expect("create shared predecessor state");
        fs::create_dir_all(current.parent().expect("current state parent"))
            .expect("create current state parent");
        Self {
            root,
            common_git,
            current,
            sources: PredecessorSources::new(&worktree_state, &shared_state),
        }
    }

    fn write_complete_predecessor(&self) {
        fs::write(
            &self.sources.file_loc,
            br#"{"version":1,"paths":["src/old.rs"]}"#,
        )
        .expect("file-loc predecessor");
        fs::write(
            &self.sources.file_bytes,
            br#"{"version":1,"paths":["src/bytes.rs"]}"#,
        )
        .expect("file-byte predecessor");
        fs::write(
            &self.sources.complexity_ccn,
            br#"{"version":1,"functions":[["src/old.rs","parse"]]}"#,
        )
        .expect("ccn predecessor");
        fs::write(
            &self.sources.complexity_length,
            br#"{"version":1,"functions":[["src/long.rs","render"]]}"#,
        )
        .expect("length predecessor");
        fs::write(
            &self.sources.repo_doc_chars,
            br#"{"version":1,"paths":["README.md"]}"#,
        )
        .expect("repo-doc predecessor");
        fs::write(
            &self.sources.claims,
            [
                br#"{"version":1,"ttl_seconds":21600,"claims":["#.as_slice(),
                br#"{"path":" ./src\\old.rs ","actor":" peer ","created_at":10.0,"#.as_slice(),
                br#""expires_at":3000.0}]}"#.as_slice(),
            ]
            .concat(),
        )
        .expect("claim predecessor");
    }
}

impl Drop for StateFixture {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove fixture {}: {error}", self.root.display()),
        }
    }
}
