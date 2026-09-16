//! The supervisor notice socket queues a burst the resident has not read yet.

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
