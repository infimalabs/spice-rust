//! Generation-race coverage for resident startup and trust rulings.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bus::Bus;
use spice::repository::Repository;
use wire::intent::TrustRef;
use wire::{Actor, CanonicalUuid, CommandKind, Outcome, TrustState, Witness};

use super::policy_surface::{trust_rule, trust_rule_with};
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
    let reference = TrustRef {
        digest: digest.clone(),
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
        actor: Actor::Operator,
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
