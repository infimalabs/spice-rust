use std::fs::{self, File, FileTimes};
use std::time::{Duration, SystemTime};

use super::*;

const TREE: &str = "0123456789abcdef0123456789abcdef01234567";
const BRANCH: &str = "refs/heads/main";

#[test]
fn collection_preserves_every_retained_root_and_its_receipts() {
    let fixture = Fixture::new("collection-reachability");
    let store = fixture.store();
    let orphan = publish_at(&store, b"orphan", "12:00", "13:00");
    let pending_admission = publish_at(&store, b"pending", "12:10", "13:10");
    let pending = prepare(&store, b"pending transaction", "1", "12:15");
    let credit_admission = publish_at(&store, b"credit", "12:20", "13:20");
    let credit = prepare(&store, b"credit transaction", "2", "12:25");
    store.commit_admission_consumption(&credit).unwrap();
    let current = publish_at(&store, b"current", "12:30", "13:30");

    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    for (identity, admission) in [
        (b"orphan".as_slice(), &orphan),
        (b"pending".as_slice(), &pending_admission),
        (b"credit".as_slice(), &credit_admission),
        (b"current".as_slice(), &current),
    ] {
        set_modified(&admission_path(&store, admission), old);
        set_modified(&receipt_path(&store, identity), old);
        fs::remove_file(store.layout.index_path(&derive_action_key(identity))).unwrap();
    }

    let report = fixture
        .store()
        .collect(SystemTime::now(), Duration::from_secs(60), 20)
        .unwrap();

    assert_eq!(report.removed_admissions(), 1);
    assert_eq!(report.removed_objects(), 1);
    assert_eq!(report.removed_pending_consumptions(), 0);
    assert_eq!(report.removed_admission_credits(), 0);
    assert!(!admission_path(&store, &orphan).exists());
    assert!(!receipt_path(&store, b"orphan").exists());
    for (identity, admission) in [
        (b"pending".as_slice(), pending_admission),
        (b"credit".as_slice(), credit_admission),
        (b"current".as_slice(), current),
    ] {
        assert!(admission_path(&store, &admission).exists());
        assert!(receipt_path(&store, identity).exists());
    }
    assert!(store.layout.pending_admission_path(&pending).exists());
    assert!(store.layout.admission_credit_path(&credit).exists());
}

#[test]
fn bounded_collection_removes_roots_before_their_evidence_graph() {
    let fixture = Fixture::new("collection-replay-order");
    let store = fixture.store();
    let historical = publish_at(&store, b"historical", "12:00", "13:00");
    let transaction = prepare(&store, b"interrupted transaction", "1", "12:05");
    store
        .with_lock(|layout| {
            consumption::commit_with(layout, &transaction, || {
                Err(EvidenceStoreError::new(
                    "fixture::interrupted",
                    "after credit",
                ))
            })
        })
        .unwrap_err();
    publish_at(&store, b"current", "12:10", "13:10");
    let now = SystemTime::now() + Duration::from_secs(1);

    let first = store.collect(now, Duration::ZERO, 1).unwrap();
    assert_eq!(first.removed_pending_consumptions(), 1);
    assert!(store.layout.admission_credit_path(&transaction).exists());
    assert!(admission_path(&store, &historical).exists());

    let second = store.collect(now, Duration::ZERO, 1).unwrap();
    assert_eq!(second.removed_admission_credits(), 1);
    assert!(admission_path(&store, &historical).exists());

    let third = store.collect(now, Duration::ZERO, 1).unwrap();
    assert_eq!(third.removed_admissions(), 1);
    assert!(!admission_path(&store, &historical).exists());
}

#[test]
fn malformed_roots_are_named_and_fail_closed_for_descendant_collection() {
    let fixture = Fixture::new("collection-malformed-root");
    let store = fixture.store();
    let historical = publish_at(&store, b"historical", "12:00", "13:00");
    publish_at(&store, b"current", "12:10", "13:10");
    for identity in [b"historical".as_slice(), b"current".as_slice()] {
        fs::remove_file(store.layout.index_path(&derive_action_key(identity))).unwrap();
    }
    let malformed = AdmissionTransactionId::derive(b"malformed collection root");
    let malformed_path = store.layout.pending_admission_path(&malformed);
    fs::write(&malformed_path, b"not canonical").unwrap();

    let report = store
        .collect(
            SystemTime::now() + Duration::from_secs(1),
            Duration::ZERO,
            20,
        )
        .unwrap();

    assert_eq!(report.removed_admissions(), 0);
    assert_eq!(report.removed_objects(), 0);
    assert!(malformed_path.exists());
    assert!(admission_path(&store, &historical).exists());
    assert!(receipt_path(&store, b"historical").exists());
    assert!(
        report
            .retained()
            .iter()
            .any(|entry| entry.contains(malformed.as_str()))
    );
}

#[test]
fn exact_prepare_and_credit_lookup_ignore_large_history_inventories() {
    let fixture = Fixture::new("collection-constant-lookup");
    let store = fixture.store();
    publish_at(&store, b"current", "12:00", "13:00");
    let transaction = AdmissionTransactionId::derive(b"indexed transaction");
    let candidate = candidate("1");
    let admissions = store.layout.admissions.join(TREE);
    for index in 0..512 {
        fs::write(
            admissions.join(format!("historical-admission-{index}")),
            b"expired history",
        )
        .unwrap();
        fs::write(
            store
                .layout
                .admission_credits
                .join(format!("historical-credit-{index}")),
            b"expired history",
        )
        .unwrap();
    }

    assert!(
        store
            .prepare_admission_consumption(&transaction, &[candidate], instant("12:05"),)
            .unwrap()
    );
    let committed = store
        .commit_admission_consumption(&transaction)
        .unwrap()
        .unwrap();
    assert_eq!(
        store.admission_credit(&transaction).unwrap(),
        Some(committed)
    );
    assert_eq!(fs::read_dir(admissions).unwrap().count(), 513);
    assert_eq!(
        fs::read_dir(&store.layout.admission_credits)
            .unwrap()
            .count(),
        513
    );
}

fn publish_at(store: &EvidenceStore, identity: &[u8], created: &str, expires: &str) -> AdmissionId {
    publish_candidate_admission_at(
        store,
        TREE,
        BRANCH,
        None,
        identity,
        &format!("2026-09-04T{created}:00.000Z"),
        &format!("2026-09-04T{expires}:00.000Z"),
    )
}

fn prepare(
    store: &EvidenceStore,
    identity: &[u8],
    commit_digit: &str,
    at: &str,
) -> AdmissionTransactionId {
    let transaction = AdmissionTransactionId::derive(identity);
    assert!(
        store
            .prepare_admission_consumption(&transaction, &[candidate(commit_digit)], instant(at))
            .unwrap()
    );
    transaction
}

fn candidate(digit: &str) -> AdmissionCandidate {
    AdmissionCandidate::new(BRANCH, None, digit.repeat(40), TREE).unwrap()
}

fn instant(time: &str) -> wire::Instant {
    format!("2026-09-04T{time}:00.000Z").parse().unwrap()
}

fn admission_path(store: &EvidenceStore, admission: &AdmissionId) -> PathBuf {
    store.layout.admissions.join(TREE).join(admission.as_str())
}

fn receipt_path(store: &EvidenceStore, identity: &[u8]) -> PathBuf {
    let receipt = pass_receipt(&derive_action_key(identity), identity);
    store.layout.object_path(&receipt.id().unwrap())
}

fn set_modified(path: &Path, modified: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}
