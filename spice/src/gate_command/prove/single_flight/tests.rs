use std::fs;
use std::thread;
use std::time::{Duration, SystemTime};

use gate::{
    EvidenceProducerRecord, LeaseAcquisition, PassReceipt, PassReceiptFields, derive_action_key,
};
use wire::{Digest, Domain};

use super::{Claim, LeaseGuard, claim};

struct Fixture {
    root: std::path::PathBuf,
    store: gate::EvidenceStore,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-proof-flight-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        fs::create_dir(&root).expect("create single-flight fixture");
        let store = gate::EvidenceStore::open(&root).expect("open single-flight evidence store");
        Self { root, store }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove single-flight fixture");
    }
}

#[test]
fn waiter_consumes_the_producers_receipt() {
    let fixture = Fixture::new("success");
    let action = derive_action_key(b"shared success");
    let LeaseAcquisition::Acquired { lease, .. } = fixture
        .store
        .acquire_lease(
            &action,
            "producer",
            SystemTime::now(),
            Duration::from_secs(5),
        )
        .unwrap()
    else {
        panic!("producer acquires action");
    };
    let waiting_store = fixture.store.clone();
    let waiting_action = action.clone();
    let waiter = thread::spawn(move || claim(&waiting_store, "waiter", "shared", &waiting_action));
    thread::sleep(Duration::from_millis(50));
    let receipt = receipt(&action);
    fixture
        .store
        .publish_batch_leased([(&receipt, &lease)], SystemTime::now())
        .unwrap();
    fixture.store.release_lease(&lease).unwrap();

    let Ok(Claim::Present(report)) = waiter.join().expect("waiter thread completes") else {
        panic!("waiter consumes published receipt");
    };
    assert_eq!(report.action_key, action);
}

#[test]
fn waiter_receives_the_producers_terminal_failure() {
    let fixture = Fixture::new("failure");
    let action = derive_action_key(b"shared failure");
    let LeaseAcquisition::Acquired { lease, .. } = fixture
        .store
        .acquire_lease(
            &action,
            "producer",
            SystemTime::now(),
            Duration::from_secs(5),
        )
        .unwrap()
    else {
        panic!("producer acquires action");
    };
    let waiting_store = fixture.store.clone();
    let waiting_action = action.clone();
    let waiter = thread::spawn(move || claim(&waiting_store, "waiter", "shared", &waiting_action));
    thread::sleep(Duration::from_millis(50));
    fixture
        .store
        .fail_lease(&lease, "gate::shared-failure", "producer failed", true)
        .unwrap();

    let Err(error) = waiter.join().expect("waiter thread completes") else {
        panic!("waiter receives failure");
    };
    assert_eq!(error.reason(), "gate::shared-failure");
    assert_eq!(error.detail(), "producer failed");
    assert!(error.is_fault());
}

#[test]
fn renewal_retains_one_long_running_producer() {
    let fixture = Fixture::new("renewal");
    let action = derive_action_key(b"renewed action");
    let LeaseAcquisition::Acquired { lease, .. } = fixture
        .store
        .acquire_lease(
            &action,
            "producer",
            SystemTime::now(),
            Duration::from_millis(500),
        )
        .unwrap()
    else {
        panic!("producer acquires action");
    };
    let guard = LeaseGuard::new_with_timing(
        fixture.store.clone(),
        lease,
        Duration::from_secs(2),
        Duration::from_millis(50),
    );
    thread::sleep(Duration::from_millis(600));
    assert!(matches!(
        fixture.store.acquire_lease(
            &action,
            "contender",
            SystemTime::now(),
            Duration::from_millis(20),
        ).unwrap(),
        LeaseAcquisition::Busy { owner, .. } if owner == "producer"
    ));
    drop(guard);
}

#[test]
fn expired_producer_admits_one_waiter_takeover() {
    let fixture = Fixture::new("takeover");
    let action = derive_action_key(b"expired action");
    let LeaseAcquisition::Acquired { lease: _lease, .. } = fixture
        .store
        .acquire_lease(
            &action,
            "departed",
            SystemTime::now(),
            Duration::from_millis(20),
        )
        .unwrap()
    else {
        panic!("departed producer acquires action");
    };
    let Ok(Claim::Lease(guard)) = claim(&fixture.store, "waiter", "shared", &action) else {
        panic!("expired producer admits waiter takeover");
    };
    assert_eq!(guard.current().owner(), "waiter");
}

fn receipt(action: &gate::ActionKey) -> PassReceipt {
    PassReceipt::new(PassReceiptFields {
        action_key: action.clone(),
        action_spec: Digest::compute(Domain::GatePolicy, b"spec"),
        input_manifest: Digest::compute(Domain::GateInventory, b"inputs"),
        typed_result: Digest::compute(Domain::GateMeasurement, b"passed"),
        started_at: "2026-09-05T20:00:00.000Z".parse().unwrap(),
        finished_at: "2026-09-05T20:00:01.000Z".parse().unwrap(),
        duration_millis: 1_000,
        artifacts: Vec::new(),
        producer: EvidenceProducerRecord::new(None, None, None, None, None),
    })
    .unwrap()
}
