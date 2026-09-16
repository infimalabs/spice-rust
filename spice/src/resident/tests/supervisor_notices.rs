//! The supervisor notice socket queues a burst the resident has not read yet.

use std::collections::BTreeSet;

use super::*;

/// One journal commit on the resident's loop outlasts a burst of tape records
/// on a busy lane. Every notice in that burst must still reach the resident,
/// so the queue holds far more than Darwin's seventeen-notice default.
#[test]
fn the_notice_socket_queues_a_burst_the_resident_reads_later() {
    let (root, paths) = resident_fixture("supervisor-notices");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a current-thread runtime builds");
    let entered = runtime.enter();
    let mut reports = capture::SupervisorReports::bind(&paths).expect("the notice socket binds");
    let door = supervisor::capture_path(&paths);
    let sender = std::os::unix::net::UnixDatagram::unbound().expect("an unbound sender opens");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let burst = 500_u64;

    for offset in 0..burst {
        let notice = supervisor::Notice::Record {
            lane,
            occupant,
            stream: "stdout".to_owned(),
            boundary: "complete".to_owned(),
            locus: wire::TapeLocus {
                member: 1,
                offset: offset * 256,
                length: 256,
            },
        };
        let bytes = serde_json::to_vec(&notice).expect("the notice serializes");
        sender
            .send_to(&bytes, &door)
            .unwrap_or_else(|error| panic!("notice {offset} was refused: {error}"));
    }

    let received = runtime.block_on(async {
        let mut offsets = Vec::new();
        for _ in 0..burst {
            match reports.recv().await.expect("a queued notice reads") {
                supervisor::Notice::Record { locus, .. } => offsets.push(locus.offset / 256),
                other => panic!("a record notice read back as {other:?}"),
            }
        }
        offsets
    });
    assert_eq!(received, (0..burst).collect::<Vec<_>>());
    drop(reports);
    drop(entered);
    fs::remove_dir_all(root).expect("notice fixture cleanup");
}

/// A supervisor sends its ending notice and exits right after, and a seat
/// sweep judges it by that exit, so the notice can still be in the kernel
/// queue when the sweep runs. A pull takes it without a scheduler turn, and
/// the drain task keeps reading behind it without reading it twice.
#[test]
fn a_pull_takes_a_notice_the_drain_task_has_not_been_scheduled_to_read() {
    let (root, paths) = resident_fixture("supervisor-pull");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a current-thread runtime builds");
    let entered = runtime.enter();
    let mut reports = capture::SupervisorReports::bind(&paths).expect("the notice socket binds");
    let door = supervisor::capture_path(&paths);
    let sender = std::os::unix::net::UnixDatagram::unbound().expect("an unbound sender opens");
    let lane = test_uuid(7);
    let occupant = test_uuid(8);
    let send = |notice: &supervisor::Notice| {
        let bytes = serde_json::to_vec(notice).expect("the notice serializes");
        sender.send_to(&bytes, &door).expect("the notice is queued");
    };

    assert!(reports.pull().is_empty(), "an idle door pulls nothing");
    send(&supervisor::Notice::Exited { lane, occupant });
    let pulled = reports.pull();
    assert!(
        matches!(
            pulled.as_slice(),
            [Ok(supervisor::Notice::Exited { lane: l, occupant: o })] if *l == lane && *o == occupant
        ),
        "the exit notice is pulled off the kernel queue: {pulled:?}"
    );
    assert!(reports.pull().is_empty(), "a pulled notice is gone");

    send(&supervisor::Notice::Failed {
        lane,
        occupant,
        failure: "later".to_owned(),
    });
    let next = runtime
        .block_on(reports.recv())
        .expect("the drain task reads");
    assert!(
        matches!(&next, supervisor::Notice::Failed { failure, .. } if failure == "later"),
        "the drain task reads what the pull left: {next:?}"
    );
    drop(reports);
    drop(entered);
    fs::remove_dir_all(root).expect("notice fixture cleanup");
}

/// A supervisor's ending notice can be at the door when a sweep judges that
/// supervisor by its exit. The sweep asks the door and leaves the seat to the
/// notice arm, which then reads what was staged in arrival order.
#[test]
fn a_sweep_asks_the_door_and_the_notice_arm_reads_what_it_staged_in_order() {
    let (root, paths) = resident_fixture("supervisor-door");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a current-thread runtime builds");
    let entered = runtime.enter();
    let mut reports = capture::SupervisorReports::bind(&paths).expect("the notice socket binds");
    let door = supervisor::capture_path(&paths);
    let sender = std::os::unix::net::UnixDatagram::unbound().expect("an unbound sender opens");
    let lane = test_uuid(9);
    let occupant = test_uuid(10);
    let send = |notice: &supervisor::Notice| {
        let bytes = serde_json::to_vec(notice).expect("the notice serializes");
        sender.send_to(&bytes, &door).expect("the notice is queued");
    };

    assert!(
        reports.noticed_endings().is_empty(),
        "an idle door notices nothing"
    );
    send(&supervisor::Notice::Record {
        lane,
        occupant,
        stream: "stdout".to_owned(),
        boundary: "terminal".to_owned(),
        locus: wire::TapeLocus {
            member: 1,
            offset: 0,
            length: 256,
        },
    });
    send(&supervisor::Notice::Exited { lane, occupant });
    let noticed = BTreeSet::from([(lane, occupant)]);
    assert_eq!(
        reports.noticed_endings(),
        noticed,
        "the ending at the door is noticed"
    );
    assert_eq!(
        reports.noticed_endings(),
        noticed,
        "a staged ending stays noticed until the notice arm takes it"
    );

    let first = runtime
        .block_on(reports.recv())
        .expect("the staged record reads without the drain task");
    let batch = reports.batch(Ok(first));
    assert!(
        matches!(
            batch.as_slice(),
            [
                Ok(supervisor::Notice::Record { .. }),
                Ok(supervisor::Notice::Exited { .. })
            ]
        ),
        "the notice arm reads the record before the ending, as they arrived: {batch:?}"
    );
    assert!(reports.pull().is_empty(), "nothing is staged twice");
    drop(reports);
    drop(entered);
    fs::remove_dir_all(root).expect("notice fixture cleanup");
}
