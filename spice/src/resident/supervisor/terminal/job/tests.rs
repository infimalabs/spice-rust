use std::fs;
use std::path::PathBuf;

use super::{Ending, Jobs, REQUEST, STOP, ServiceIndex, reconcile_services_beneath};

fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-service-index-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ))
}

fn reconcile(root: &std::path::Path, rebuild: bool) -> String {
    let mut diagnostic = Vec::new();
    reconcile_services_beneath(root, rebuild, |_| false, &mut diagnostic);
    String::from_utf8(diagnostic).expect("diagnostic text")
}

/// An unowned service is asked to stop on every commit until its ending is
/// observed; only then does it leave the index.
#[test]
fn an_unowned_service_is_stopped_until_its_ending_retires_it() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let jobs = Jobs::beneath(&root, lane);
    let service = jobs.record_for_tests("serve", false, None);
    let index = ServiceIndex::beneath(&root);

    reconcile(&root, false);
    let stop = service.directory.join(STOP);
    let stopped = stop.exists();
    let indexed_while_running = index.entries().expect("index reads");
    service.end_for_tests(Ending::Stopped);
    reconcile(&root, false);
    let indexed_after_ending = index.entries().expect("index reads");
    let _ = fs::remove_dir_all(&root);

    assert!(stopped, "an unowned service was not asked to stop");
    assert_eq!(indexed_while_running, vec![(lane, service.id().to_owned())]);
    assert!(
        indexed_after_ending.is_empty(),
        "an ended service stayed indexed"
    );
}

/// Commit-time reconciliation reads only the index: retained finite history
/// and records an earlier resident never indexed are untouched until the
/// first reconciliation rebuilds the index, which then stops an unreadable
/// record fail-closed.
#[test]
fn only_a_rebuild_reads_job_history_and_it_fails_closed() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let jobs = Jobs::beneath(&root, lane);
    for _ in 0..8 {
        let finite = jobs.record_for_tests("cargo test", false, Some(600_000));
        finite.end_for_tests(Ending::Exited { code: 0 });
        assert!(finite.claim_heard().expect("delivery claims"));
    }
    let running = jobs.record_for_tests("cargo build", false, Some(600_000));
    let unindexed = jobs.record_for_tests("serve", false, None);
    let index = ServiceIndex::beneath(&root);
    index.remove(lane, unindexed.id()).expect("entry removes");
    fs::remove_file(unindexed.directory.join(REQUEST)).expect("request removes");

    reconcile(&root, false);
    let stopped_without_rebuild = unindexed.directory.join(STOP).exists();
    let diagnostic = reconcile(&root, true);
    let stopped_after_rebuild = unindexed.directory.join(STOP).exists();
    let finite_stopped = running.directory.join(STOP).exists();
    let indexed = index.entries().expect("index reads");
    let _ = fs::remove_dir_all(&root);

    assert!(
        !stopped_without_rebuild,
        "commit-time reconciliation read unindexed history"
    );
    assert!(
        stopped_after_rebuild,
        "a rebuild did not stop an unreadable record: {diagnostic}"
    );
    assert!(
        !finite_stopped,
        "service reconciliation stopped a finite job"
    );
    assert_eq!(indexed, vec![(lane, unindexed.id().to_owned())]);
}

/// An entry whose record no longer exists names no process and is retired
/// without stopping anything.
#[test]
fn an_entry_without_a_record_is_retired() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let index = ServiceIndex::beneath(&root);
    let id = "dW0Cb068M";
    index.insert(lane, id).expect("entry inserts");

    let diagnostic = reconcile(&root, false);
    let indexed = index.entries().expect("index reads");
    let _ = fs::remove_dir_all(&root);

    assert!(indexed.is_empty(), "a vanished record stayed indexed");
    assert!(
        diagnostic.is_empty(),
        "a vanished record was reported: {diagnostic}"
    );
}
