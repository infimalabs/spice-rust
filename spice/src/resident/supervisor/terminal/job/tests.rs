use std::fs;
use std::path::PathBuf;

use super::keeper::event_lane;
use super::reconcile::reconcile_services_beneath;
use super::service::ServiceClaim;
use super::{Ending, Job, Jobs, REQUEST, REQUEST_BYTES, Request, STOP, ServiceIndex, read_json};

fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-service-index-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple(),
    ))
}

fn reconcile(root: &std::path::Path, rebuild: bool) -> String {
    reconcile_with(root, rebuild, |_| false, |_, _| false)
}

fn reconcile_with(
    root: &std::path::Path,
    rebuild: bool,
    task_owned: impl Fn(&ServiceClaim) -> bool,
    turn_owned: impl Fn(wire::CanonicalUuid, wire::CanonicalUuid) -> bool,
) -> String {
    let mut diagnostic = Vec::new();
    reconcile_services_beneath(root, rebuild, task_owned, turn_owned, &mut diagnostic);
    String::from_utf8(diagnostic).expect("diagnostic text")
}

#[test]
fn quiescence_waits_for_ending_delivery_not_just_process_completion() {
    let root = scratch();
    let jobs = Jobs::beneath(&root, crate::resident::new_uuid());
    let command = jobs.record_for_tests("build", false, Some(600_000));
    assert!(!jobs.quiescent().expect("running catalog"));
    command.end_for_tests(Ending::Exited { code: 0 });
    assert!(!jobs.quiescent().expect("unheard ending catalog"));
    assert!(command.claim_heard().expect("ending delivery"));
    assert!(jobs.quiescent().expect("delivered ending catalog"));
    fs::remove_dir_all(root).expect("fixture cleanup");
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

/// A service never keeps a lane idle: an event nobody hears wakes the lane
/// of a deadline-bounded command, and a service's events wake nobody.
#[test]
fn only_a_deadline_bounded_commands_unheard_events_wake_its_lane() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let jobs = Jobs::beneath(&root, lane);
    let request = |job: &Job| -> Request {
        read_json(&job.log_path().with_file_name(REQUEST), REQUEST_BYTES).expect("request")
    };
    let watch = jobs.record_for_tests("tail -f build.log", true, Some(60_000));
    let service = jobs.record_for_tests("app-demo", true, None);

    let woken = (event_lane(&request(&watch)), event_lane(&request(&service)));

    let _ = fs::remove_dir_all(&root);
    assert_eq!(woken, (Some(lane), None));
}

#[test]
fn job_records_preserve_ambient_task_presence_and_absence() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let jobs = Jobs::beneath(&root, lane);
    let task = wire::TaskStamp::for_micros(10).expect("fixture task");
    let associated = jobs.record_bound_for_tests(
        "cargo test",
        false,
        Some(600_000),
        Some(task.clone()),
        None,
        None,
    );
    let unassociated = jobs.record_for_tests("cargo check", false, Some(600_000));

    let associated = associated.listing().expect("associated job lists");
    let unassociated = unassociated.listing().expect("unassociated job lists");
    let _ = fs::remove_dir_all(&root);

    assert_eq!(associated.task.as_ref(), Some(&task));
    assert_eq!(unassociated.task, None);
}

#[test]
fn task_service_lifetime_follows_its_exact_claim_owner() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let occupant = crate::resident::new_uuid();
    let task = wire::TaskStamp::for_micros(20).expect("fixture task");
    let claim = ServiceClaim::for_tests(task.clone(), lane, occupant);
    let service = Jobs::beneath(&root, lane).record_bound_for_tests(
        "serve",
        false,
        None,
        Some(task.clone()),
        Some(claim),
        None,
    );

    reconcile_with(&root, false, |_| true, |_, _| false);
    let retained = !service.directory.join(STOP).exists();
    reconcile_with(&root, false, |_| false, |_, _| true);
    let stopped = service.directory.join(STOP).exists();
    let recorded_task = service.task().cloned();
    let _ = fs::remove_dir_all(&root);

    assert!(retained, "an active task service was stopped");
    assert!(stopped, "a task service survived its claim owner");
    assert_eq!(recorded_task, Some(task));
}

#[test]
fn turn_reconciliation_stops_only_that_turns_unanchored_services() {
    let root = scratch();
    let lane = crate::resident::new_uuid();
    let turn = crate::resident::new_uuid();
    let other_turn = crate::resident::new_uuid();
    let task = wire::TaskStamp::for_micros(30).expect("fixture task");
    let jobs = Jobs::beneath(&root, lane);
    let ending = jobs.record_bound_for_tests("serve one", false, None, None, None, Some(turn));
    let other = jobs.record_bound_for_tests("serve two", false, None, None, None, Some(other_turn));
    let anchored = jobs.record_bound_for_tests(
        "serve task",
        false,
        None,
        Some(task.clone()),
        Some(ServiceClaim::for_tests(task, lane, turn)),
        None,
    );

    reconcile_with(
        &root,
        false,
        |_| true,
        |active_lane, owner| active_lane == lane && owner == other_turn,
    );
    let ending_stopped = ending.directory.join(STOP).exists();
    let other_stopped = other.directory.join(STOP).exists();
    let anchored_stopped = anchored.directory.join(STOP).exists();
    let _ = fs::remove_dir_all(&root);

    assert!(ending_stopped, "the ended turn's service kept running");
    assert!(!other_stopped, "another turn's service was stopped");
    assert!(
        !anchored_stopped,
        "a task-owned service was stopped at turn end"
    );
}
