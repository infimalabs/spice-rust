use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;
use std::path::PathBuf;

#[test]
fn immutable_receipts_and_atomic_indexes_round_trip_canonically() {
    let fixture = Fixture::new("round-trip");
    let store = fixture.store();
    let action = derive_action_key(b"canonical action");
    let receipt = pass_receipt(&action, b"trusted result");

    let first = store.publish(&receipt).unwrap();
    assert!(first.object_created());
    assert!(first.index_updated());
    assert_eq!(first.receipt(), first.selected());
    assert_eq!(store.lookup(&action).unwrap(), Some(receipt.clone()));

    let second = store.publish(&receipt).unwrap();
    assert!(!second.object_created());
    assert!(!second.index_updated());
    assert_eq!(second.selected(), first.selected());
    let bytes = receipt.encode().unwrap();
    assert_eq!(PassReceipt::decode(&bytes).unwrap(), receipt);
    assert_eq!(
        store.root(),
        fixture.path().join(".spice-rs/gate-evidence/v1")
    );
    assert_eq!(
        store
            .layout
            .object_path(first.receipt())
            .file_name()
            .unwrap(),
        first.receipt().as_str()
    );
    assert_eq!(
        store.layout.index_path(&action).file_name().unwrap(),
        action.as_str()
    );
}

#[test]
fn concurrent_linked_publishers_converge_on_one_deterministic_index() {
    let fixture = Fixture::new("publish-race");
    let action = derive_action_key(b"shared action");
    let receipts = [b"result-a".as_slice(), b"result-b".as_slice()]
        .map(|result| pass_receipt(&action, result));
    let expected = receipts
        .iter()
        .map(PassReceipt::id)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .min()
        .unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = Vec::new();
    for index in 0..8 {
        let store = fixture.store();
        let receipt = receipts[index % receipts.len()].clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            store.publish(&receipt).unwrap()
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    let selected = store_index(&fixture.store(), &action).receipt;
    assert_eq!(selected, expected);
    assert_eq!(
        fixture
            .store()
            .lookup(&action)
            .unwrap()
            .unwrap()
            .id()
            .unwrap(),
        expected
    );
}

#[test]
fn interruption_after_object_publication_never_exposes_a_partial_index() {
    let fixture = Fixture::new("interrupted-publication");
    let store = fixture.store();
    let action = derive_action_key(b"interrupted action");
    let receipt = pass_receipt(&action, b"complete object");
    let error = store
        .publish_with(&receipt, || {
            Err(EvidenceStoreError::new("injected", "stop before index"))
        })
        .unwrap_err();
    assert_eq!(error.reason(), "injected");
    assert!(store.lookup(&action).unwrap().is_none());
    assert!(store.layout.object_path(&receipt.id().unwrap()).is_file());
}

#[test]
fn corruption_and_unsupported_schema_are_retained_and_refused() {
    let fixture = Fixture::new("corruption");
    let store = fixture.store();
    let action = derive_action_key(b"corrupt action");
    let receipt = pass_receipt(&action, b"valid");
    store.publish(&receipt).unwrap();
    let index = store.layout.index_path(&action);
    fs::write(&index, b"not json").unwrap();
    let before = fs::read(&index).unwrap();
    let error = store.lookup(&action).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::index-invalid");
    assert_eq!(fs::read(&index).unwrap(), before);

    let marker = store.root().join("store.json");
    fs::write(&marker, br#"{"version":999}"#).unwrap();
    let before = fs::read(&marker).unwrap();
    let error = EvidenceStore::open(fixture.path()).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::store-version-unsupported");
    assert_eq!(fs::read(marker).unwrap(), before);
}

#[test]
fn mismatched_and_unsupported_receipts_are_retained_and_refused() {
    let fixture = Fixture::new("invalid-receipts");
    let store = fixture.store();
    let action = derive_action_key(b"expected action");
    let other_action = derive_action_key(b"other action");
    let other_receipt = pass_receipt(&other_action, b"other result");
    store.publish(&other_receipt).unwrap();

    let index_path = store.layout.index_path(&action);
    publish_json(
        &index_path,
        &ActionIndex::new(other_action, other_receipt.id().unwrap()),
    )
    .unwrap();
    let before = fs::read(&index_path).unwrap();
    let error = store.lookup(&action).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::index-action-mismatch");
    assert_eq!(fs::read(&index_path).unwrap(), before);

    let mut unsupported = pass_receipt(&action, b"future result");
    unsupported.version = 999;
    let unsupported_bytes = wire::encode(&unsupported).unwrap();
    let unsupported_id = ReceiptId(Digest::compute(Domain::GatePassReceipt, &unsupported_bytes));
    let object_path = store.layout.object_path(&unsupported_id);
    fs::write(&object_path, unsupported_bytes).unwrap();
    publish_json(
        &index_path,
        &ActionIndex::new(action.clone(), unsupported_id),
    )
    .unwrap();
    let object_before = fs::read(&object_path).unwrap();
    let error = store.lookup(&action).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::receipt-version-unsupported");
    assert_eq!(fs::read(&object_path).unwrap(), object_before);

    let mut nonpassing = pass_receipt(&action, b"failed result");
    nonpassing.passed = false;
    let error = PassReceipt::decode(&wire::encode(&nonpassing).unwrap()).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::receipt-not-passing");

    let mut unordered = pass_receipt(&action, b"unordered artifacts");
    unordered.artifacts = [
        Digest::compute(Domain::GateMeasurement, b"artifact-a"),
        Digest::compute(Domain::GateMeasurement, b"artifact-b"),
    ]
    .into_iter()
    .rev()
    .collect();
    unordered.artifacts.sort();
    unordered.artifacts.reverse();
    let error = PassReceipt::decode(&wire::encode(&unordered).unwrap()).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::receipt-artifacts-invalid");
}

#[test]
fn stale_lease_takeover_is_exact_and_old_tokens_cannot_release_it() {
    let fixture = Fixture::new("leases");
    let store = fixture.store();
    let action = derive_action_key(b"leased action");
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let first = acquired(
        store
            .acquire_lease(&action, "worker-a", start, Duration::from_secs(10))
            .unwrap(),
    );
    assert!(
        matches!(store.acquire_lease(&action, "worker-b", start + Duration::from_secs(9), Duration::from_secs(10)).unwrap(), LeaseAcquisition::Busy { owner, .. } if owner == "worker-a")
    );

    let LeaseAcquisition::Acquired {
        lease: second,
        recovered,
    } = store
        .acquire_lease(
            &action,
            "worker-b",
            start + Duration::from_secs(10),
            Duration::from_secs(20),
        )
        .unwrap()
    else {
        panic!("expired lease must be recovered");
    };
    assert!(recovered);
    assert_ne!(first.token(), second.token());
    assert_eq!(
        store.release_lease(&first).unwrap_err().reason(),
        "gate-evidence::lease-token-mismatch"
    );
    let renewed = store
        .renew_lease(
            &second,
            start + Duration::from_secs(11),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(renewed.token(), second.token());
    assert!(renewed.expires_at_millis() > second.expires_at_millis());
    store.release_lease(&renewed).unwrap();
    assert!(!store.layout.lease_path(&action).exists());
}

#[test]
fn garbage_collection_is_bounded_and_retains_ambiguous_bytes() {
    let fixture = Fixture::new("collection");
    let store = fixture.store();
    let action = derive_action_key(b"collect action");
    let receipts =
        [b"one".as_slice(), b"two".as_slice()].map(|result| pass_receipt(&action, result));
    for receipt in &receipts {
        store.publish(receipt).unwrap();
    }
    let selected = store_index(&store, &action).receipt;
    let unreferenced = receipts
        .iter()
        .map(PassReceipt::id)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .find(|id| id != &selected)
        .unwrap();

    let now = SystemTime::now() + Duration::from_secs(1);
    let report = store.collect(now, Duration::ZERO, 1).unwrap();
    assert_eq!(report.removed_objects(), 1);
    assert_eq!(report.removed_leases(), 0);
    assert!(!store.layout.object_path(&unreferenced).exists());
    assert!(store.layout.object_path(&selected).exists());

    let corrupt = ReceiptId(Digest::compute(Domain::GatePassReceipt, b"corrupt"));
    let path = store.layout.object_path(&corrupt);
    fs::write(&path, b"corrupt").unwrap();
    let leased_action = derive_action_key(b"collect expired lease");
    let lease = acquired(
        store
            .acquire_lease(
                &leased_action,
                "retired-worker",
                SystemTime::UNIX_EPOCH,
                Duration::from_secs(1),
            )
            .unwrap(),
    );
    let lease_path = store.layout.lease_path(lease.action());
    let report = store.collect(now, Duration::ZERO, 1).unwrap();
    assert_eq!(report.removed_objects(), 0);
    assert_eq!(report.removed_leases(), 1);
    assert!(path.exists());
    assert!(!lease_path.exists());
    assert!(
        report
            .retained()
            .iter()
            .any(|item| item.contains(corrupt.as_str()))
    );
}

fn pass_receipt(action: &ActionKey, result: &[u8]) -> PassReceipt {
    PassReceipt::new(PassReceiptFields {
        action_key: action.clone(),
        action_spec: Digest::compute(Domain::GatePolicy, b"action spec"),
        input_manifest: Digest::compute(Domain::GateInventory, b"input manifest"),
        typed_result: Digest::compute(Domain::GateMeasurement, result),
        started_at: "2026-09-04T12:00:00.000Z".parse().unwrap(),
        finished_at: "2026-09-04T12:00:00.100Z".parse().unwrap(),
        duration_millis: 100,
        artifacts: Vec::new(),
        producer: ProducerRecord::new(
            None,
            None,
            None,
            Some(Actor::Spice),
            Some(Digest::compute(
                Domain::GateRepositoryAuthority,
                b"appliance generation",
            )),
        ),
    })
    .unwrap()
}

fn acquired(outcome: LeaseAcquisition) -> EvidenceLease {
    let LeaseAcquisition::Acquired {
        lease,
        recovered: false,
    } = outcome
    else {
        panic!("fresh lease must be acquired");
    };
    lease
}

fn store_index(store: &EvidenceStore, action: &ActionKey) -> ActionIndex {
    store
        .with_lock(|layout| read_index(layout, action).map(Option::unwrap))
        .unwrap()
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-evidence-{name}-{}-{}",
            std::process::id(),
            NEXT_LEASE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn path(&self) -> &Path {
        &self.root
    }
    fn store(&self) -> EvidenceStore {
        EvidenceStore::open(&self.root).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
