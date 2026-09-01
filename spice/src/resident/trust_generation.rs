//! Generation-race coverage for resident startup and trust rulings.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bus::Bus;
use spice::repository::Repository;
use wire::intent::TrustRef;
use wire::{Actor, CanonicalUuid, CommandKind, Outcome, Producer, TrustHead, TrustState, Witness};

use super::policy_surface::{trust_pend, trust_pend_with, trust_rule, trust_rule_with};
use super::{RuntimePaths, commit_startup, commit_startup_with};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn startup_runtime_policy_and_trust_share_one_capture_generation() {
    const FIRST: &str = concat!(
        "[steer]\nexpiry = \"12h\"\nresend = [\"2m\", \"4m\", \"6m\"]\n",
        "[consent]\nposture = \"deny\"\n",
        "[taste.polling]\nwords = [\"polling\"]\nmessage = \"Wait for completion.\"\n",
    );
    const SECOND: &str = concat!(
        "[steer]\nexpiry = \"48h\"\nresend = [\"10m\", \"20m\", \"30m\"]\n",
        "[consent]\nposture = \"allow\"\n",
        "[taste.fallbacks]\nwords = [\"fallback\"]\nmessage = \"Keep one path.\"\n",
    );
    let (root, paths) = resident_fixture("startup-single-capture");
    let tracked = root.join("spice-rust.toml");
    fs::write(&tracked, FIRST).expect("write first startup policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");

    let first = super::load_startup_surface_with(&repository, || replace_policy(&tracked, SECOND))
        .expect("captured startup surface remains admitted");
    assert_eq!(first.config.steering.expiry().to_string(), "12h");
    assert_eq!(first.config.consent.posture(), consent::Answer::Deny);
    assert_eq!(
        first
            .config
            .taste
            .tastes()
            .iter()
            .map(|taste| taste.name().as_str())
            .collect::<Vec<_>>(),
        ["polling"],
        "taste and steering policy come from the first captured generation"
    );
    let named_current =
        super::load_startup_surface(&repository).expect("replacement startup surface loads");
    assert_ne!(first.trust, named_current.trust);

    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let before = bus.fact_snapshot().expect("empty authority reads");
    let refused = commit_startup(&bus, &repository, &first.trust, test_uuid(9), None)
        .expect_err("moved single capture must refuse startup");
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("refused authority reads"),
        before,
        "stale startup capture changed journal authority"
    );

    let second = super::load_startup_surface(&repository).expect("replacement surface loads");
    assert_eq!(second.config.steering.expiry().to_string(), "48h");
    assert_eq!(second.config.consent.posture(), consent::Answer::Allow);
    assert_eq!(
        second
            .config
            .taste
            .tastes()
            .iter()
            .map(|taste| taste.name().as_str())
            .collect::<Vec<_>>(),
        ["fallbacks"]
    );
    assert_eq!(second.trust, named_current.trust);
    commit_startup(&bus, &repository, &second.trust, test_uuid(10), None)
        .expect("retry seats replacement capture");
    let after_retry = bus.fact_snapshot().expect("retried authority reads");
    assert_eq!(
        trust_pend_digests(&after_retry),
        [second.trust.digest().clone()]
    );
    assert_eq!(
        after_retry
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::UpSeated)
            .count(),
        1
    );
    bus.verify().expect("retried startup authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn policy_movement_after_pend_refuses_seating_and_retry_pends_the_new_digest() {
    let (root, paths) = resident_fixture("trust-pend-generation");
    let tracked = root.join("spice-rust.toml");
    fs::write(&tracked, "# tracked policy v1\n").expect("write first policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface = crate::trust_command::Surface::load(&repository).expect("first surface loads");
    let first_digest = surface.digest().clone();
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");

    let refused = commit_startup_with(&bus, &repository, &surface, test_uuid(10), None, || {
        replace_policy(&tracked, "# tracked policy v2\n")
    })
    .expect_err("moved policy must refuse seating");
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    let after_refusal = bus.fact_snapshot().expect("refused startup facts read");
    assert_eq!(trust_pend_digests(&after_refusal), [first_digest]);
    assert!(
        after_refusal
            .facts()
            .iter()
            .all(|fact| fact.fact.kind() != wire::FactKind::UpSeated)
    );

    let current =
        crate::trust_command::Surface::load(&repository).expect("replacement surface loads");
    let current_digest = current.digest().clone();
    assert_ne!(current_digest, surface.digest().clone());
    commit_startup(&bus, &repository, &current, test_uuid(11), None)
        .expect("retry seats current surface");
    let after_retry = bus.fact_snapshot().expect("retry facts read");
    assert_eq!(
        trust_pend_digests(&after_retry),
        [surface.digest().clone(), current_digest]
    );
    assert_eq!(
        after_retry
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::UpSeated)
            .count(),
        1
    );
    bus.verify().expect("retried startup authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn equal_byte_replacement_with_an_existing_head_refuses_zero_write_seating() {
    let (root, paths) = resident_fixture("trust-existing-generation");
    let tracked = root.join("spice-rust.toml");
    let bytes = "# stable tracked policy\n";
    fs::write(&tracked, bytes).expect("write tracked policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface = crate::trust_command::Surface::load(&repository).expect("first surface loads");
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &surface, test_uuid(12), None)
        .expect("first generation seats");
    let before = bus.fact_snapshot().expect("authority before replacement");

    let refused = commit_startup_with(&bus, &repository, &surface, test_uuid(13), None, || {
        replace_policy(&tracked, bytes)
    })
    .expect_err("same-byte generation replacement must refuse");
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("authority after refusal"),
        before,
        "refused seating changed journal authority"
    );
    bus.verify().expect("refused startup authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn running_resident_pends_current_surface_then_existing_grant_succeeds() {
    let (root, paths) = resident_fixture("trust-live-pend");
    let tracked = root.join("spice-rust.toml");
    fs::write(&tracked, "# initial policy\n").expect("write initial policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let initial = crate::trust_command::Surface::load(&repository).expect("initial surface loads");
    let epoch = test_uuid(30);
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &initial, epoch, None)
        .expect("resident seats with initial surface");

    let current_bytes = "# changed executable policy\n";
    replace_policy(&tracked, current_bytes);
    let current = crate::trust_command::Surface::load(&repository).expect("changed surface loads");
    let digest = current.digest().clone();
    let worktree = repository
        .worktree_root()
        .to_str()
        .expect("fixture worktree is UTF-8");
    let reference = TrustRef {
        digest: digest.clone(),
        worktree: Some(worktree.to_owned()),
    };
    let lane_pend = admitted_trust_command_as(
        CommandKind::TrustPend,
        digest.clone(),
        TrustState::Pended,
        epoch,
        test_uuid(31),
        Some(worktree),
        Actor::Lane(test_uuid(32)),
    );
    let before_lane = bus.fact_snapshot().expect("facts before lane pend");
    let refused = trust_pend(&bus, &repository, epoch, &lane_pend, &reference);
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::operator-required"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("facts after lane pend"),
        before_lane
    );

    let pend = admitted_trust_pend_command(
        CommandKind::TrustPend,
        digest.clone(),
        TrustState::Pended,
        epoch,
        test_uuid(33),
        worktree,
    );
    let before_movement = bus.fact_snapshot().expect("facts before moved pend");
    let head_before_movement = bus
        .trust_head(&digest)
        .expect("head before moved pend reads");
    let receipts_before_movement = receipt_count(&paths.journal);
    let refused = trust_pend_with(&bus, &repository, epoch, &pend, &reference, || {
        replace_policy(&tracked, current_bytes)
    });
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("facts after moved pend"),
        before_movement
    );
    assert_eq!(
        bus.trust_head(&digest)
            .expect("head after moved pend reads"),
        head_before_movement,
    );
    assert_eq!(receipt_count(&paths.journal), receipts_before_movement);

    let pended = trust_pend(&bus, &repository, epoch, &pend, &reference);
    assert!(matches!(pended, Outcome::Reply(_)));
    let pended_head = bus
        .trust_head(&digest)
        .expect("pended head reads")
        .expect("pended head exists");
    assert_eq!(pended_head.body().state, TrustState::Pended);
    assert_eq!(pended_head.body().worktree.as_deref(), Some(worktree));
    let grant = admitted_trust_command(
        CommandKind::TrustGrant,
        digest.clone(),
        TrustState::Pended,
        epoch,
        test_uuid(34),
    );
    let ruling = TrustRef {
        digest: digest.clone(),
        worktree: None,
    };
    let granted = trust_rule(&bus, &repository, epoch, &grant, &ruling);
    assert!(matches!(granted, Outcome::Reply(_)), "{granted:?}");
    assert_eq!(
        bus.trust_head(&digest)
            .expect("granted head reads")
            .expect("granted head exists")
            .body()
            .state,
        TrustState::Granted,
    );
    bus.verify()
        .expect("live pend and grant authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn pend_refuses_unresolved_nonroot_and_foreign_worktrees_without_writes() {
    let (root, paths) = resident_fixture("trust-worktree-admission");
    let tracked = root.join("spice-rust.toml");
    fs::write(&tracked, "# resident policy\n").expect("write resident policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface = crate::trust_command::Surface::load(&repository).expect("trust surface loads");
    let digest = surface.digest().clone();
    let epoch = test_uuid(40);
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");

    let nested = root.join("nested");
    fs::create_dir(&nested).expect("nested worktree path exists");
    let missing = root.join("removed");
    let (foreign, _) = resident_fixture("trust-foreign-worktree");
    fs::write(foreign.join("spice-rust.toml"), "# foreign policy\n").expect("write foreign policy");
    let foreign = fs::canonicalize(foreign).expect("canonicalize foreign worktree");
    let cases = [
        (
            missing.to_str().expect("missing path is UTF-8"),
            "trust::worktree-unresolved",
            test_uuid(41),
        ),
        (
            nested.to_str().expect("nested path is UTF-8"),
            "trust::worktree-not-root",
            test_uuid(42),
        ),
        (
            foreign.to_str().expect("foreign path is UTF-8"),
            "trust::foreign-worktree",
            test_uuid(43),
        ),
    ];
    for (worktree, reason, id) in cases {
        let reference = TrustRef {
            digest: digest.clone(),
            worktree: Some(worktree.to_owned()),
        };
        let command = admitted_trust_pend_command(
            CommandKind::TrustPend,
            digest.clone(),
            TrustState::Pended,
            epoch,
            id,
            worktree,
        );
        let before = bus.fact_snapshot().expect("facts before refused pend");
        let head = bus.trust_head(&digest).expect("head before refused pend");
        let receipts = receipt_count(&paths.journal);
        let refused = trust_pend(&bus, &repository, epoch, &command, &reference);
        assert!(
            matches!(refused, Outcome::Refusal(ref refusal) if refusal.reason() == reason),
            "unexpected {reason} outcome: {refused:?}",
        );
        assert_eq!(
            bus.fact_snapshot().expect("facts after refused pend"),
            before
        );
        assert_eq!(
            bus.trust_head(&digest).expect("head after refused pend"),
            head
        );
        assert_eq!(receipt_count(&paths.journal), receipts);
    }

    bus.verify().expect("refused pend authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(foreign).expect("foreign fixture cleanup");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn legacy_unbound_head_refuses_ruling_without_writes() {
    let (root, paths) = resident_fixture("trust-legacy-unbound");
    let tracked = root.join("spice-rust.toml");
    fs::write(&tracked, "# current policy\n").expect("write current policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let current = crate::trust_command::Surface::load(&repository).expect("current surface loads");
    let epoch = test_uuid(50);
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &current, epoch, None).expect("resident seats");

    replace_policy(&tracked, "# legacy policy still current\n");
    let legacy = crate::trust_command::Surface::load(&repository).expect("legacy surface loads");
    let digest = legacy.digest().clone();
    let mut seed = super::write(Actor::Spice, epoch, Producer::CommandDoor)
        .expect("legacy authority write builds");
    let pended = seed
        .fact(
            wire::body::TrustPended {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: digest.clone(),
                worktree: None,
                previous: Some(current.digest().clone()),
            },
            journal::FactCause::Root,
        )
        .expect("legacy trust fact builds");
    let head_digest = digest.clone();
    seed.trust_head(pended, move |_| TrustHead {
        digest: head_digest,
        state: TrustState::Pended,
        worktree: None,
    });
    super::commit(&bus, seed).expect("legacy authority seeds");

    let command = admitted_trust_command(
        CommandKind::TrustGrant,
        digest.clone(),
        TrustState::Pended,
        epoch,
        test_uuid(51),
    );
    let reference = TrustRef {
        digest: digest.clone(),
        worktree: None,
    };
    let before = bus.fact_snapshot().expect("facts before legacy refusal");
    let head = bus.trust_head(&digest).expect("legacy head reads");
    let receipts = receipt_count(&paths.journal);
    let refused = trust_rule(&bus, &repository, epoch, &command, &reference);
    assert!(
        matches!(refused, Outcome::Refusal(ref refusal) if refusal.reason() == "trust::worktree-unbound"),
        "unexpected legacy ruling outcome: {refused:?}",
    );
    assert_eq!(
        bus.fact_snapshot().expect("facts after legacy refusal"),
        before
    );
    assert_eq!(bus.trust_head(&digest).expect("legacy head rereads"), head);
    assert_eq!(receipt_count(&paths.journal), receipts);

    bus.verify().expect("legacy refusal authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn grant_and_revoke_generation_movement_preserve_facts_heads_and_receipts() {
    let (root, paths) = resident_fixture("trust-ruling-generation");
    let tracked = root.join("spice-rust.toml");
    let bytes = "# stable ruling policy\n";
    fs::write(&tracked, bytes).expect("write tracked policy");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface = crate::trust_command::Surface::load(&repository).expect("trust surface loads");
    let digest = surface.digest().clone();
    let epoch = test_uuid(20);
    let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    commit_startup(&bus, &repository, &surface, epoch, None).expect("surface pends and seats");
    let ruling_epoch = test_uuid(23);
    commit_startup(&bus, &repository, &surface, ruling_epoch, None)
        .expect("replacement resident seats without repending the surface");

    let grant = admitted_trust_command(
        CommandKind::TrustGrant,
        digest.clone(),
        TrustState::Pended,
        ruling_epoch,
        test_uuid(21),
    );
    let reference = TrustRef {
        digest: digest.clone(),
        worktree: None,
    };
    let before_grant = bus.fact_snapshot().expect("facts before refused grant");
    let head_before_grant = bus
        .trust_head(&digest)
        .expect("head before refused grant reads");
    let receipts_before_grant = receipt_count(&paths.journal);
    let refused = trust_rule_with(&bus, &repository, ruling_epoch, &grant, &reference, || {
        replace_policy(&tracked, bytes)
    });
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("facts after refused grant"),
        before_grant
    );
    assert_eq!(
        bus.trust_head(&digest)
            .expect("head after refused grant reads"),
        head_before_grant
    );
    assert_eq!(receipt_count(&paths.journal), receipts_before_grant);

    let current = crate::trust_command::Surface::load(&repository).expect("current surface loads");
    assert_eq!(current.digest(), &digest);
    let granted = trust_rule(&bus, &repository, ruling_epoch, &grant, &reference);
    assert!(matches!(granted, Outcome::Reply(_)));
    let head_after_grant = bus
        .trust_head(&digest)
        .expect("granted head reads")
        .expect("granted head exists");
    assert_same_origin_cause(
        &bus.fact_snapshot().expect("granted facts read"),
        head_before_grant
            .as_ref()
            .expect("pended head exists")
            .position(),
        head_after_grant.position(),
    );

    let revoke = admitted_trust_command(
        CommandKind::TrustRevoke,
        digest.clone(),
        TrustState::Granted,
        ruling_epoch,
        test_uuid(22),
    );
    let before_revoke = bus.fact_snapshot().expect("facts before refused revoke");
    let head_before_revoke = bus
        .trust_head(&digest)
        .expect("head before refused revoke reads");
    let receipts_before_revoke = receipt_count(&paths.journal);
    let refused = trust_rule_with(&bus, &repository, ruling_epoch, &revoke, &reference, || {
        publish_local_policy(&root, "# newly published local policy\n")
    });
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "trust::surface-moved"
    ));
    assert_eq!(
        bus.fact_snapshot().expect("facts after refused revoke"),
        before_revoke
    );
    assert_eq!(
        bus.trust_head(&digest)
            .expect("head after refused revoke reads"),
        head_before_revoke
    );
    assert_eq!(receipt_count(&paths.journal), receipts_before_revoke);
    bus.verify().expect("ruling authority verifies");
    bus.shutdown().expect("fixture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

fn assert_same_origin_cause(snapshot: &journal::FactSnapshot, cause: wire::Seq, child: wire::Seq) {
    let cause = snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == cause)
        .expect("cause fact exists");
    let child = snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == child)
        .expect("child fact exists");
    assert_eq!(child.cause, Some(cause.seq));
    assert_eq!(child.origin, cause.origin);
}

fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
    let root = std::env::temp_dir().join(format!(
        "spice-resident-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join(".git")).expect("git directory");
    let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
    fs::create_dir_all(&paths.data).expect("data directory");
    (root, paths)
}

fn replace_policy(path: &Path, bytes: &str) {
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, bytes).expect("write policy replacement");
    fs::rename(&replacement, path).expect("publish policy replacement");
}

fn publish_local_policy(root: &Path, bytes: &str) {
    let parent = root.join(".git/.spice-rs");
    fs::create_dir_all(&parent).expect("publish local policy parent");
    fs::write(parent.join("config.toml"), bytes).expect("publish local policy");
}

fn trust_pend_digests(snapshot: &journal::FactSnapshot) -> Vec<wire::Digest> {
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == wire::FactKind::TrustPended)
        .map(|envelope| {
            let bytes = wire::encode(&envelope.fact).expect("trust fact encodes");
            wire::decode::<wire::Fact<wire::body::TrustPended>>(&bytes)
                .expect("trust fact decodes")
                .body
                .digest
        })
        .collect()
}

fn admitted_trust_command(
    kind: CommandKind,
    digest: wire::Digest,
    standing: TrustState,
    epoch: CanonicalUuid,
    id: CanonicalUuid,
) -> wire::AdmittedCommand {
    admitted_trust_command_as(kind, digest, standing, epoch, id, None, Actor::Operator)
}

fn admitted_trust_pend_command(
    kind: CommandKind,
    digest: wire::Digest,
    standing: TrustState,
    epoch: CanonicalUuid,
    id: CanonicalUuid,
    worktree: &str,
) -> wire::AdmittedCommand {
    admitted_trust_command_as(
        kind,
        digest,
        standing,
        epoch,
        id,
        Some(worktree),
        Actor::Operator,
    )
}

fn admitted_trust_command_as(
    kind: CommandKind,
    digest: wire::Digest,
    standing: TrustState,
    epoch: CanonicalUuid,
    id: CanonicalUuid,
    worktree: Option<&str>,
    actor: Actor,
) -> wire::AdmittedCommand {
    let reference = TrustRef {
        digest: digest.clone(),
        worktree: worktree.map(str::to_owned),
    };
    wire::Command {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        kind,
        expected: vec![Witness::Trust { digest, standing }],
        body: serde_json::to_value(reference).expect("trust reference encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor,
        epoch,
    })
    .expect("fixture trust command admits")
}

fn receipt_count(path: &Path) -> u64 {
    let count: i64 = rusqlite::Connection::open(path)
        .expect("receipt fixture opens")
        .query_row("SELECT COUNT(*) FROM command_receipts", [], |row| {
            row.get(0)
        })
        .expect("receipt count reads");
    u64::try_from(count).expect("receipt count is nonnegative")
}

fn test_uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}
