use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;
use std::path::PathBuf;

use crate::{
    ChangeSelectionPolicy, ChangeSelectionPolicyInputs, ChangeSelectionReport, select_change_gates,
};

mod collection;

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
fn successful_batch_publishes_every_receipt_and_index() {
    let fixture = Fixture::new("batch-success");
    let store = fixture.store();
    let actions = [
        derive_action_key(b"first successful batch action"),
        derive_action_key(b"second successful batch action"),
    ];
    let receipts = [
        pass_receipt(&actions[0], b"first successful batch result"),
        pass_receipt(&actions[1], b"second successful batch result"),
    ];

    let publications = store.publish_batch(&receipts).unwrap();

    assert_eq!(publications.len(), receipts.len());
    for ((action, receipt), publication) in actions.iter().zip(&receipts).zip(publications) {
        assert!(publication.object_created());
        assert!(publication.index_updated());
        assert_eq!(publication.receipt(), &receipt.id().unwrap());
        assert_eq!(store.lookup(action).unwrap(), Some(receipt.clone()));
    }
}

#[test]
fn exact_batch_lookup_reports_missing_and_refuses_mismatched_authority() {
    let fixture = Fixture::new("batch-lookup");
    let store = fixture.store();
    let present_key = derive_action_key(b"present admission action");
    let missing_key = derive_action_key(b"missing admission action");
    let receipt = pass_receipt(&present_key, b"present admission result");
    store.publish(&receipt).unwrap();
    let expected = [
        ReceiptExpectation::new(
            "present",
            present_key.clone(),
            receipt.action_spec().clone(),
            receipt.input_manifest().clone(),
        )
        .unwrap(),
        ReceiptExpectation::new(
            "missing",
            missing_key,
            receipt.action_spec().clone(),
            receipt.input_manifest().clone(),
        )
        .unwrap(),
    ];

    let observed = store.lookup_batch_exact(&expected).unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(
        observed[0].as_ref().unwrap().receipt(),
        &receipt.id().unwrap()
    );
    assert!(observed[1].is_none());

    let mismatched = [ReceiptExpectation::new(
        "present",
        present_key,
        receipt.action_spec().clone(),
        Digest::compute(Domain::GateInputManifest, b"different manifest"),
    )
    .unwrap()];
    let error = store.lookup_batch_exact(&mismatched).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::receipt-authority-mismatch");
}

#[test]
fn immutable_candidate_admission_round_trips_and_concurrent_publishers_converge() {
    let fixture = Fixture::new("admission-publication");
    let store = fixture.store();
    let action_key = derive_action_key(b"admitted action");
    let receipt = pass_receipt(&action_key, b"admitted result");
    store.publish(&receipt).unwrap();
    let expected = ReceiptExpectation::new(
        "formatters",
        action_key,
        receipt.action_spec().clone(),
        receipt.input_manifest().clone(),
    )
    .unwrap();
    let action = store
        .lookup_batch_exact(&[expected])
        .unwrap()
        .remove(0)
        .unwrap();
    let created_at: wire::Instant = "2026-09-04T12:00:00.000Z".parse().unwrap();
    let expires_at: wire::Instant = "2026-09-04T12:10:00.000Z".parse().unwrap();
    let bundle = AdmissionBundle::new(AdmissionBundleFields {
        candidate_tree: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        branch: "refs/heads/main".to_owned(),
        head: Some("89abcdef0123456789abcdef0123456789abcdef".to_owned()),
        selection: Digest::compute(Domain::GateSelection, b"selection"),
        policy: Digest::compute(Domain::GatePolicy, b"policy"),
        gate_set: Digest::compute(Domain::GateSet, b"gate set"),
        actions: vec![action],
        live_checks: vec![
            AdmissionLiveCheck::new(
                "staging",
                Digest::compute(Domain::GateFocusedReport, b"staging result"),
            )
            .unwrap(),
        ],
        claim: Digest::compute(Domain::GateRepositoryAuthority, b"claim"),
        created_at,
        expires_at,
    })
    .unwrap();
    let expected_id = bundle.id().unwrap();
    let bytes = bundle.encode().unwrap();
    assert_eq!(AdmissionBundle::decode(&bytes).unwrap(), bundle);

    let barrier = Arc::new(Barrier::new(4));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        let bundle = bundle.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            store.publish_admission(&bundle).unwrap()
        }));
    }
    for worker in workers {
        assert_eq!(worker.join().unwrap(), expected_id);
    }
    let path = store
        .layout
        .admissions
        .join(bundle.candidate_tree())
        .join(expected_id.as_str());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn admission_consumption_is_idempotent_across_prepare_commit_and_abort() {
    let fixture = Fixture::new("admission-consumption-lifecycle");
    let producer = fixture.store();
    let tree = "0123456789abcdef0123456789abcdef01234567";
    let commit = "1111111111111111111111111111111111111111";
    let head = "89abcdef0123456789abcdef0123456789abcdef";
    let admission =
        publish_candidate_admission(&producer, tree, "refs/heads/main", Some(head), b"lifecycle");
    let consumer = fixture.store();
    let transaction = AdmissionTransactionId::derive(b"lifecycle transaction");
    let candidate =
        AdmissionCandidate::new("refs/heads/main", Some(head.to_owned()), commit, tree).unwrap();
    let now = "2026-09-04T12:05:00.000Z".parse().unwrap();

    assert!(
        consumer
            .prepare_admission_consumption(&transaction, std::slice::from_ref(&candidate), now)
            .unwrap()
    );
    assert!(
        consumer
            .prepare_admission_consumption(&transaction, std::slice::from_ref(&candidate), now)
            .unwrap()
    );
    assert!(consumer.admission_credit(&transaction).unwrap().is_none());

    let committed = consumer
        .commit_admission_consumption(&transaction)
        .unwrap()
        .expect("pending admission promotes");
    assert_eq!(committed.commits().collect::<Vec<_>>(), [commit]);
    assert_eq!(committed.admissions().collect::<Vec<_>>(), [&admission]);
    assert_eq!(
        consumer.commit_admission_consumption(&transaction).unwrap(),
        Some(committed.clone())
    );
    assert!(!consumer.abort_admission_consumption(&transaction).unwrap());
    assert_eq!(
        consumer.admission_credit(&transaction).unwrap(),
        Some(committed)
    );
}

#[test]
fn aborted_consumption_removes_only_its_pending_transaction() {
    let fixture = Fixture::new("admission-consumption-abort");
    let store = fixture.store();
    let tree = "0123456789abcdef0123456789abcdef01234567";
    publish_candidate_admission(&store, tree, "refs/heads/main", None, b"abort");
    let transaction = AdmissionTransactionId::derive(b"aborted transaction");
    let other = AdmissionTransactionId::derive(b"other transaction");
    let candidate = AdmissionCandidate::new(
        "refs/heads/main",
        None,
        "1111111111111111111111111111111111111111",
        tree,
    )
    .unwrap();
    let now = "2026-09-04T12:05:00.000Z".parse().unwrap();
    store
        .prepare_admission_consumption(&transaction, std::slice::from_ref(&candidate), now)
        .unwrap();
    store
        .prepare_admission_consumption(&other, std::slice::from_ref(&candidate), now)
        .unwrap();

    assert!(store.abort_admission_consumption(&transaction).unwrap());
    assert!(!store.abort_admission_consumption(&transaction).unwrap());
    assert!(
        store
            .commit_admission_consumption(&transaction)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .commit_admission_consumption(&other)
            .unwrap()
            .is_some()
    );
}

#[test]
fn multi_ref_consumption_is_sorted_and_shared_across_linked_store_handles() {
    let fixture = Fixture::new("admission-consumption-multi-ref");
    let producer = fixture.store();
    let first_tree = "0123456789abcdef0123456789abcdef01234567";
    let second_tree = "2222222222222222222222222222222222222222";
    publish_candidate_admission(&producer, first_tree, "refs/heads/a", None, b"a");
    publish_candidate_admission(&producer, second_tree, "refs/heads/b", None, b"b");
    let transaction = AdmissionTransactionId::derive(b"multi-ref transaction");
    let candidates = [
        AdmissionCandidate::new(
            "refs/heads/b",
            None,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            second_tree,
        )
        .unwrap(),
        AdmissionCandidate::new(
            "refs/heads/a",
            None,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            first_tree,
        )
        .unwrap(),
    ];
    producer
        .prepare_admission_consumption(
            &transaction,
            &candidates,
            "2026-09-04T12:05:00.000Z".parse().unwrap(),
        )
        .unwrap();

    let linked_consumer = fixture.store();
    let credit = linked_consumer
        .commit_admission_consumption(&transaction)
        .unwrap()
        .unwrap();
    assert_eq!(
        credit.commits().collect::<Vec<_>>(),
        [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ]
    );
}

#[test]
fn stale_or_mismatched_admission_refuses_before_pending_publication() {
    let fixture = Fixture::new("admission-consumption-mismatch");
    let store = fixture.store();
    let tree = "0123456789abcdef0123456789abcdef01234567";
    publish_candidate_admission(
        &store,
        tree,
        "refs/heads/main",
        Some("89abcdef0123456789abcdef0123456789abcdef"),
        b"mismatch",
    );
    let transaction = AdmissionTransactionId::derive(b"mismatched transaction");
    let candidate = AdmissionCandidate::new(
        "refs/heads/main",
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
        "1111111111111111111111111111111111111111",
        tree,
    )
    .unwrap();
    let error = store
        .prepare_admission_consumption(
            &transaction,
            &[candidate],
            "2026-09-04T12:20:00.000Z".parse().unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::admission-candidate-mismatch"
    );
    assert!(
        store
            .commit_admission_consumption(&transaction)
            .unwrap()
            .is_none()
    );
}

#[test]
fn current_admission_lookup_is_constant_over_accumulated_history() {
    let fixture = Fixture::new("admission-current-index");
    let store = fixture.store();
    let tree = "0123456789abcdef0123456789abcdef01234567";
    let older = publish_candidate_admission_at(
        &store,
        tree,
        "refs/heads/main",
        None,
        b"older",
        "2026-09-04T12:00:00.000Z",
        "2026-09-04T12:20:00.000Z",
    );
    let newer = publish_candidate_admission_at(
        &store,
        tree,
        "refs/heads/main",
        None,
        b"newer",
        "2026-09-04T12:05:00.000Z",
        "2026-09-04T12:15:00.000Z",
    );
    assert_ne!(older, newer);
    let history = store.layout.admissions.join(tree);
    for index in 0..128 {
        fs::write(
            history.join(format!("historical-{index}")),
            b"malformed history",
        )
        .unwrap();
    }
    let transaction = AdmissionTransactionId::derive(b"indexed transaction");
    let candidate = AdmissionCandidate::new(
        "refs/heads/main",
        None,
        "1111111111111111111111111111111111111111",
        tree,
    )
    .unwrap();

    store
        .prepare_admission_consumption(
            &transaction,
            &[candidate],
            "2026-09-04T12:06:00.000Z".parse().unwrap(),
        )
        .unwrap();
    let credit = store
        .commit_admission_consumption(&transaction)
        .unwrap()
        .unwrap();
    assert_eq!(credit.admissions().collect::<Vec<_>>(), [&newer]);
}

#[test]
fn interrupted_commit_replays_from_pending_without_losing_credit() {
    let fixture = Fixture::new("admission-consumption-crash");
    let store = fixture.store();
    let tree = "0123456789abcdef0123456789abcdef01234567";
    publish_candidate_admission(&store, tree, "refs/heads/main", None, b"crash");
    let transaction = AdmissionTransactionId::derive(b"crash transaction");
    store
        .prepare_admission_consumption(
            &transaction,
            &[AdmissionCandidate::new(
                "refs/heads/main",
                None,
                "1111111111111111111111111111111111111111",
                tree,
            )
            .unwrap()],
            "2026-09-04T12:05:00.000Z".parse().unwrap(),
        )
        .unwrap();
    let error = store
        .with_lock(|layout| {
            consumption::commit_with(layout, &transaction, || {
                Err(EvidenceStoreError::new(
                    "fixture::interrupted",
                    "stop after credit",
                ))
            })
        })
        .unwrap_err();
    assert_eq!(error.reason(), "fixture::interrupted");
    assert!(store.admission_credit(&transaction).unwrap().is_some());
    assert!(
        store
            .commit_admission_consumption(&transaction)
            .unwrap()
            .is_some()
    );
    assert!(!store.abort_admission_consumption(&transaction).unwrap());
}

#[test]
fn malformed_pending_consumption_is_retained_and_never_promoted() {
    let fixture = Fixture::new("admission-consumption-malformed");
    let store = fixture.store();
    let transaction = AdmissionTransactionId::derive(b"malformed transaction");
    let pending = store.layout.pending_admission_path(&transaction);
    fs::write(&pending, b"not canonical").unwrap();

    let error = store
        .commit_admission_consumption(&transaction)
        .unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::admission-consumption-invalid"
    );
    assert_eq!(fs::read(pending).unwrap(), b"not canonical");
    assert!(store.admission_credit(&transaction).unwrap().is_none());
}

#[test]
fn later_batch_failure_restores_every_index_and_new_object() {
    let fixture = Fixture::new("batch-failure");
    let store = fixture.store();
    let first_action = derive_action_key(b"first batch action");
    let second_action = derive_action_key(b"second batch action");
    let first_candidates = [
        pass_receipt(&first_action, b"first prior result"),
        pass_receipt(&first_action, b"first pending result"),
    ];
    let mut ordered = first_candidates
        .into_iter()
        .map(|receipt| (receipt.id().unwrap(), receipt))
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let pending_first = ordered.remove(0).1;
    let prior_first = ordered.remove(0).1;
    store.publish(&prior_first).unwrap();
    let prior_index = store_index(&store, &first_action);
    let pending_second = pass_receipt(&second_action, b"second pending result");
    let receipts = [pending_first.clone(), pending_second.clone()];

    let error = store
        .publish_batch_with(&receipts, |boundary| {
            if boundary == PublicationBoundary::Index(0) {
                Err(EvidenceStoreError::new(
                    "injected",
                    "cancel after first receipt",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();

    assert_eq!(error.reason(), "injected");
    assert_eq!(store_index(&store, &first_action), prior_index);
    assert_eq!(store.lookup(&first_action).unwrap(), Some(prior_first));
    assert!(store.lookup(&second_action).unwrap().is_none());
    for receipt in receipts {
        assert!(!store.layout.object_path(&receipt.id().unwrap()).exists());
    }
}

#[test]
fn abandoned_batch_recovers_at_every_precommit_publication_boundary() {
    let boundaries = [
        PublicationBoundary::Prepared,
        PublicationBoundary::Object(0),
        PublicationBoundary::Index(0),
        PublicationBoundary::Object(1),
        PublicationBoundary::Index(1),
    ];
    for target in boundaries {
        let fixture = Fixture::new(&format!("batch-interruption-{target:?}"));
        let store = fixture.store();
        let actions = [
            derive_action_key(b"interrupted first action"),
            derive_action_key(b"interrupted second action"),
        ];
        let receipts = [
            pass_receipt(&actions[0], b"interrupted first result"),
            pass_receipt(&actions[1], b"interrupted second result"),
        ];

        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store
                .publish_batch_with(&receipts, |observed| {
                    assert_ne!(observed, target, "injected publication interruption");
                    Ok(())
                })
                .unwrap();
        }));
        assert!(interrupted.is_err(), "boundary {target:?} was not reached");

        let recovered = fixture.store();
        for (action, receipt) in actions.iter().zip(&receipts) {
            assert!(recovered.lookup(action).unwrap().is_none());
            assert!(
                !recovered
                    .layout
                    .object_path(&receipt.id().unwrap())
                    .exists()
            );
        }
        assert_eq!(
            read_publication(&recovered.layout).unwrap(),
            Some(PublicationJournal::idle())
        );
    }
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
fn leased_publication_requires_the_exact_live_action_token() {
    let fixture = Fixture::new("leased-publication");
    let store = fixture.store();
    let action = derive_action_key(b"published action");
    let other = derive_action_key(b"other action");
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let lease = acquired(
        store
            .acquire_lease(&action, "worker-a", start, Duration::from_secs(10))
            .unwrap(),
    );
    let receipt = pass_receipt(&action, b"passed");
    let other_receipt = pass_receipt(&other, b"other");

    assert_eq!(
        store
            .publish_batch_leased([(&other_receipt, &lease)], start)
            .unwrap_err()
            .reason(),
        "gate-evidence::lease-action-mismatch"
    );
    assert_eq!(
        store
            .publish_batch_leased([(&receipt, &lease)], start + Duration::from_secs(10))
            .unwrap_err()
            .reason(),
        "gate-evidence::lease-expired"
    );
    let publication = store
        .publish_batch_leased([(&receipt, &lease)], start + Duration::from_secs(9))
        .unwrap();
    assert_eq!(publication.len(), 1);
    assert!(store.lookup(&action).unwrap().is_some());
}

#[test]
fn lease_failure_is_shared_until_one_expired_takeover() {
    let fixture = Fixture::new("lease-failure");
    let store = fixture.store();
    let action = derive_action_key(b"failed action");
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
    let lease = acquired(
        store
            .acquire_lease(&action, "worker-a", start, Duration::from_secs(10))
            .unwrap(),
    );
    store
        .fail_lease(
            &lease,
            "gate::fixture-failed",
            "fixture execution failed",
            false,
        )
        .unwrap();
    assert_eq!(
        store
            .publish_batch_leased([(&pass_receipt(&action, b"late"), &lease)], start)
            .unwrap_err()
            .reason(),
        "gate-evidence::lease-terminal"
    );

    for owner in ["worker-b", "worker-c"] {
        assert!(matches!(
            store.acquire_lease(
                &action,
                owner,
                start + Duration::from_secs(9),
                Duration::from_secs(10),
            ).unwrap(),
            LeaseAcquisition::Failed { reason, detail, fault: false, .. }
                if reason == "gate::fixture-failed" && detail == "fixture execution failed"
        ));
    }
    assert_eq!(
        store
            .renew_lease(
                &lease,
                start + Duration::from_secs(1),
                Duration::from_secs(10)
            )
            .unwrap_err()
            .reason(),
        "gate-evidence::lease-terminal"
    );
    let LeaseAcquisition::Acquired { recovered, .. } = store
        .acquire_lease(
            &action,
            "worker-b",
            start + Duration::from_secs(10),
            Duration::from_secs(10),
        )
        .unwrap()
    else {
        panic!("expired failure must admit one takeover");
    };
    assert!(recovered);
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

#[test]
fn shared_fingerprint_manifest_round_trips_and_authority_drift_misses() {
    let fixture = Fixture::new("fingerprint-round-trip");
    let producer = fixture.store();
    let consumer = fixture.store();
    let authority = fingerprint_authority(b"environment-a");
    let action = derive_action_key(b"fingerprinted action");
    let receipt = pass_receipt(&action, b"fingerprinted result");
    producer.publish(&receipt).unwrap();
    let expected = ReceiptExpectation::new(
        "cargo:typecheck:fixture::lib:fixture",
        action.clone(),
        receipt.action_spec().clone(),
        receipt.input_manifest().clone(),
    )
    .unwrap();
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: vec![expected],
    })
    .unwrap();

    let published = producer.publish_action_fingerprints(&manifest).unwrap();
    assert_eq!(published, authority.key().unwrap());
    let object = producer
        .layout
        .fingerprint_objects
        .join(manifest.id().unwrap().as_str());
    assert_eq!(fs::read(&object).unwrap(), manifest.encode().unwrap());
    assert_ne!(
        fs::read(producer.layout.fingerprints.join(published.as_str())).unwrap(),
        manifest.encode().unwrap(),
    );
    assert_eq!(
        consumer.lookup_action_fingerprints(&authority).unwrap(),
        Some(manifest.clone())
    );
    let resolution = consumer
        .lookup_fingerprint_receipts(&authority)
        .unwrap()
        .unwrap();
    let expected_bytes = [
        producer.layout.fingerprints.join(published.as_str()),
        object,
        producer.layout.index_path(&action),
        producer.layout.object_path(&receipt.id().unwrap()),
    ]
    .into_iter()
    .map(|path| fs::metadata(path).unwrap().len())
    .sum::<u64>();
    assert_eq!(resolution.files_read(), 4);
    assert_eq!(resolution.bytes_read(), expected_bytes);
    assert_eq!(
        consumer
            .lookup_batch_exact(manifest.actions())
            .unwrap()
            .into_iter()
            .flatten()
            .count(),
        1
    );
    assert_eq!(
        consumer
            .lookup_action_fingerprints(&fingerprint_authority(b"environment-b"))
            .unwrap(),
        None
    );
}

#[test]
fn corrupt_fingerprint_index_refuses_without_another_lookup_path() {
    let fixture = Fixture::new("fingerprint-corrupt");
    let store = fixture.store();
    let authority = fingerprint_authority(b"environment");
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: Vec::new(),
    })
    .unwrap();
    let key = store.publish_action_fingerprints(&manifest).unwrap();
    fs::write(
        store.layout.fingerprints.join(key.as_str()),
        b"not canonical",
    )
    .unwrap();

    let error = store.lookup_action_fingerprints(&authority).unwrap_err();
    assert_eq!(error.reason(), "gate-evidence::fingerprint-index-invalid");
}

#[test]
fn version_two_authority_key_is_stable_and_direct_manifest_refuses() {
    let fixture = Fixture::new("fingerprint-v2-refusal");
    let store = fixture.store();
    let authority = fingerprint_authority(b"environment");
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: Vec::new(),
    })
    .unwrap();
    let key = authority.key().unwrap();
    let authority_bytes = wire::encode(&authority).unwrap();
    assert!(authority_bytes.starts_with(br#"{"version":2,"candidate_tree":"#));
    assert_eq!(
        key.as_str(),
        "781323747a963a75ede73ff3f03812c4c7ef622593455be550aed2babd036d91"
    );

    let current = String::from_utf8(manifest.encode().unwrap()).unwrap();
    let version_two = current.replacen("\"version\":3", "\"version\":2", 1);
    fs::write(store.layout.fingerprints.join(key.as_str()), version_two).unwrap();
    let error = store.lookup_action_fingerprints(&authority).unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::fingerprint-version-unsupported"
    );
    assert!(error.detail().contains("spice gate prove --missing"));
}

#[test]
fn interrupted_fingerprint_publication_leaves_only_an_idempotent_object() {
    let fixture = Fixture::new("fingerprint-interrupted");
    let store = fixture.store();
    let authority = fingerprint_authority(b"environment");
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: Vec::new(),
    })
    .unwrap();

    let error = store
        .with_lock(|layout| {
            fingerprint::publish_with(layout, &manifest, || {
                Err(EvidenceStoreError::new(
                    "gate-evidence::fingerprint-test-interrupted",
                    "stop after object publication",
                ))
            })
        })
        .unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::fingerprint-test-interrupted"
    );
    assert!(
        fs::read_dir(&store.layout.fingerprints)
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(
        fs::read_dir(&store.layout.fingerprint_objects)
            .unwrap()
            .count(),
        1,
    );

    store.publish_action_fingerprints(&manifest).unwrap();
    assert_eq!(
        store.lookup_action_fingerprints(&authority).unwrap(),
        Some(manifest),
    );
}

#[test]
fn missing_or_changed_fingerprint_object_refuses_by_digest() {
    let fixture = Fixture::new("fingerprint-object-drift");
    let store = fixture.store();
    let authority = fingerprint_authority(b"environment");
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: Vec::new(),
    })
    .unwrap();
    store.publish_action_fingerprints(&manifest).unwrap();
    let object = store
        .layout
        .fingerprint_objects
        .join(manifest.id().unwrap().as_str());

    fs::remove_file(&object).unwrap();
    let missing = store.lookup_action_fingerprints(&authority).unwrap_err();
    assert_eq!(
        missing.reason(),
        "gate-evidence::fingerprint-object-missing"
    );

    fs::write(&object, b"changed fingerprint bytes").unwrap();
    let changed = store.lookup_action_fingerprints(&authority).unwrap_err();
    assert_eq!(
        changed.reason(),
        "gate-evidence::fingerprint-digest-mismatch"
    );
}

#[test]
fn fingerprint_index_cannot_substitute_another_valid_manifest_object() {
    let fixture = Fixture::new("fingerprint-object-swap");
    let store = fixture.store();
    let first_authority = fingerprint_authority(b"environment-a");
    let second_authority = fingerprint_authority(b"environment-b");
    let manifest = |authority: &ActionFingerprintAuthority| {
        ActionFingerprintManifest::new(ActionFingerprintManifestFields {
            authority: authority.clone(),
            selection: fingerprint_selection(authority),
            cargo_tests: false,
            actions: Vec::new(),
        })
        .unwrap()
    };
    let first = manifest(&first_authority);
    let second = manifest(&second_authority);
    store.publish_action_fingerprints(&first).unwrap();
    store.publish_action_fingerprints(&second).unwrap();
    let first_index = store
        .layout
        .fingerprints
        .join(first_authority.key().unwrap().as_str());
    fs::write(
        first_index,
        fingerprint::ActionFingerprintIndex::new(first_authority.clone(), second.id().unwrap())
            .encode()
            .unwrap(),
    )
    .unwrap();

    let error = store
        .lookup_action_fingerprints(&first_authority)
        .unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::fingerprint-authority-mismatch"
    );
}

#[test]
fn fingerprint_manifest_rejects_action_count_and_canonical_byte_excess() {
    const CURRENT_WORKSPACE_ACTIONS: usize = 202;
    let authority = fingerprint_authority(b"environment");
    let current = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: (0..CURRENT_WORKSPACE_ACTIONS)
            .map(fingerprint_expectation)
            .collect(),
    })
    .expect("current full-workspace action inventory retains explicit headroom");
    assert_eq!(current.actions().len(), CURRENT_WORKSPACE_ACTIONS);
    assert!(current.actions().len() < ActionFingerprintManifest::MAX_ACTIONS);

    let actions = (0..=ActionFingerprintManifest::MAX_ACTIONS)
        .map(fingerprint_expectation)
        .collect();
    let count_error = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions,
    })
    .unwrap_err();
    assert_eq!(
        count_error.reason(),
        "gate-evidence::fingerprint-action-count-invalid"
    );

    let oversized = ReceiptExpectation::new(
        "x".repeat(ActionFingerprintManifest::MAX_BYTES),
        derive_action_key(b"oversized fingerprint action"),
        Digest::compute(Domain::GatePolicy, b"oversized action spec"),
        Digest::compute(Domain::GateInputManifest, b"oversized input manifest"),
    )
    .unwrap();
    let size_error = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: vec![oversized],
    })
    .unwrap_err();
    assert_eq!(
        size_error.reason(),
        "gate-evidence::fingerprint-size-invalid"
    );
}

#[test]
fn fingerprint_receipt_resolution_refuses_before_returning_a_partial_vector() {
    let fixture = Fixture::new("fingerprint-read-budget");
    let store = fixture.store();
    let authority = fingerprint_authority(b"environment");
    let actions = [
        derive_action_key(b"budget action zero"),
        derive_action_key(b"budget action one"),
    ];
    let receipts = [
        pass_receipt(&actions[0], b"budget result zero"),
        pass_receipt(&actions[1], b"budget result one"),
    ];
    store.publish_batch(&receipts).unwrap();
    let expectations = actions
        .iter()
        .zip(&receipts)
        .enumerate()
        .map(|(index, (action, receipt))| {
            ReceiptExpectation::new(
                format!("action-{index:03}"),
                action.clone(),
                receipt.action_spec().clone(),
                receipt.input_manifest().clone(),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    let manifest = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
        authority: authority.clone(),
        selection: fingerprint_selection(&authority),
        cargo_tests: false,
        actions: expectations,
    })
    .unwrap();
    store.publish_action_fingerprints(&manifest).unwrap();
    let first_read_bytes = [
        store
            .layout
            .fingerprints
            .join(authority.key().unwrap().as_str()),
        store
            .layout
            .fingerprint_objects
            .join(manifest.id().unwrap().as_str()),
        store.layout.index_path(&actions[0]),
        store.layout.object_path(&receipts[0].id().unwrap()),
    ]
    .into_iter()
    .map(|path| fs::metadata(path).unwrap().len())
    .sum::<u64>();

    let error = store
        .with_lock(|layout| {
            let mut budget = ReadBudget::new(first_read_bytes);
            let manifest = fingerprint::lookup_bounded(layout, &authority, &mut budget)?
                .expect("bounded fixture has a fingerprint");
            lookup_batch_exact_bounded(layout, manifest.actions(), &mut budget)
        })
        .unwrap_err();
    assert_eq!(
        error.reason(),
        "gate-evidence::fingerprint-read-budget-exceeded"
    );
}

fn fingerprint_authority(environment: &[u8]) -> ActionFingerprintAuthority {
    ActionFingerprintAuthority::new(ActionFingerprintAuthorityFields {
        candidate_tree: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        baseline: Some("89abcdef0123456789abcdef0123456789abcdef".to_owned()),
        source: Digest::compute(Domain::GateTree, b"source"),
        policy: Digest::compute(Domain::GatePolicy, b"policy"),
        gate_set: Digest::compute(Domain::GateSet, b"gate set"),
        tools: Digest::compute(Domain::GateTool, b"tools"),
        environment: Digest::compute(Domain::GateConfiguration, environment),
        cargo_source: Some(Digest::compute(Domain::GateInventory, b"cargo source")),
    })
    .unwrap()
}

fn fingerprint_selection(authority: &ActionFingerprintAuthority) -> ChangeSelectionReport {
    let tracked = vec!["spice-rust.toml".to_owned()];
    let policy_authorities = vec!["spice-rust.toml".to_owned()];
    let suffixes = vec![".rs".to_owned()];
    let test_paths = vec!["tests".to_owned()];
    let policy = ChangeSelectionPolicy::new(ChangeSelectionPolicyInputs {
        tracked_sources: &tracked,
        policy_authorities: &policy_authorities,
        configured: true,
        repository_documents: Some(&[]),
        file_shape_suffixes: Some(&suffixes),
        complexity_suffixes: Some(&suffixes),
        magic_suffixes: Some(&suffixes),
        package_roots: &[],
        declared_rust_sources: &[],
        generated_paths: &[],
        test_paths: &test_paths,
    })
    .expect("compile fingerprint fixture policy");
    select_change_gates(authority.source().clone(), &tracked, &policy)
        .expect("select fingerprint fixture gates")
}

fn fingerprint_expectation(index: usize) -> ReceiptExpectation {
    ReceiptExpectation::new(
        format!("action-{index:03}"),
        derive_action_key(format!("fingerprint action {index}").as_bytes()),
        Digest::compute(
            Domain::GatePolicy,
            format!("action spec {index}").as_bytes(),
        ),
        Digest::compute(
            Domain::GateInputManifest,
            format!("input manifest {index}").as_bytes(),
        ),
    )
    .unwrap()
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

fn publish_candidate_admission(
    store: &EvidenceStore,
    tree: &str,
    branch: &str,
    head: Option<&str>,
    identity: &[u8],
) -> AdmissionId {
    publish_candidate_admission_at(
        store,
        tree,
        branch,
        head,
        identity,
        "2026-09-04T12:00:00.000Z",
        "2026-09-04T12:10:00.000Z",
    )
}

fn publish_candidate_admission_at(
    store: &EvidenceStore,
    tree: &str,
    branch: &str,
    head: Option<&str>,
    identity: &[u8],
    created_at: &str,
    expires_at: &str,
) -> AdmissionId {
    let action_key = derive_action_key(identity);
    let receipt = pass_receipt(&action_key, identity);
    store.publish(&receipt).unwrap();
    let expectation = ReceiptExpectation::new(
        "formatters",
        action_key,
        receipt.action_spec().clone(),
        receipt.input_manifest().clone(),
    )
    .unwrap();
    let action = store
        .lookup_batch_exact(&[expectation])
        .unwrap()
        .remove(0)
        .unwrap();
    let bundle = AdmissionBundle::new(AdmissionBundleFields {
        candidate_tree: tree.to_owned(),
        branch: branch.to_owned(),
        head: head.map(str::to_owned),
        selection: Digest::compute(Domain::GateSelection, identity),
        policy: Digest::compute(Domain::GatePolicy, b"policy"),
        gate_set: Digest::compute(Domain::GateSet, b"gate set"),
        actions: vec![action],
        live_checks: Vec::new(),
        claim: Digest::compute(Domain::GateRepositoryAuthority, identity),
        created_at: created_at.parse().unwrap(),
        expires_at: expires_at.parse().unwrap(),
    })
    .unwrap();
    store.publish_admission(&bundle).unwrap()
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
