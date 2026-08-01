use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use bus::timer::{TimerKind, TimerWorker};
use journal::crossing::{CanonicalUuid, LeaseHead};

#[test]
fn blocking_worker_emits_deadlines_in_order_and_skips_cancellation() {
    let (worker, events) = TimerWorker::start().expect("timer worker starts");
    let first = worker
        .arm(
            "first",
            TimerKind::TaskWake,
            instant("2030-04-12T18:44:10.000Z"),
            Duration::from_millis(20),
        )
        .expect("first timer arms");
    let lease = lease_head();
    let lease_id = worker
        .arm_lease("BUS-a1", &lease, Duration::from_millis(80))
        .expect("lease timer arms");
    let canceled = worker
        .arm(
            "canceled",
            TimerKind::SteerLapse,
            instant("2030-04-12T18:44:12.000Z"),
            Duration::from_millis(150),
        )
        .expect("cancelable timer arms");
    assert!(worker.cancel(canceled).expect("timer cancellation answers"));
    let late = worker
        .arm(
            "late",
            TimerKind::LaneResume,
            instant("2030-04-12T18:44:13.000Z"),
            Duration::from_millis(200),
        )
        .expect("late timer arms");

    let first_event = events
        .recv_timeout(Duration::from_secs(1))
        .expect("first actual deadline");
    assert_eq!(first_event.id(), first);
    assert_eq!(first_event.owner(), "first");
    assert_eq!(first_event.kind(), TimerKind::TaskWake);

    let lease_event = events
        .recv_timeout(Duration::from_secs(1))
        .expect("lease actual deadline");
    assert_eq!(lease_event.id(), lease_id);
    assert_eq!(lease_event.owner(), "BUS-a1");
    assert_eq!(lease_event.kind(), TimerKind::ClaimLapse);
    assert_eq!(lease_event.deadline(), lease.expiry);

    let late_event = events
        .recv_timeout(Duration::from_secs(1))
        .expect("late actual deadline");
    assert_eq!(late_event.id(), late);
    assert_eq!(late_event.owner(), "late");
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));

    let pending = worker
        .arm(
            "shutdown",
            TimerKind::LaneHold,
            instant("2030-04-12T18:45:00.000Z"),
            Duration::from_secs(60),
        )
        .expect("pending timer arms");
    assert!(pending.get() > late.get());
    assert_eq!(worker.shutdown().expect("worker shuts down"), 1);
    assert!(matches!(
        events.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Disconnected)
    ));
}

fn lease_head() -> LeaseHead {
    LeaseHead {
        start: instant("2030-04-12T18:44:00.000Z"),
        renewal: instant("2030-04-12T18:44:01.000Z"),
        maximum: instant("2030-04-12T18:45:00.000Z"),
        expiry: instant("2030-04-12T18:44:11.000Z"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
    }
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical uuid")
}

fn instant(value: &str) -> journal::crossing::Instant {
    value.parse().expect("canonical instant")
}
