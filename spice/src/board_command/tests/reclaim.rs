use super::*;

pub(super) fn renew_and_assert(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    before: &BoardTask,
    expected: Vec<Witness>,
) -> BoardTask {
    let lease = before.lease.as_ref().expect("active lease");
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimRenew,
        expected.clone(),
        &ClaimRenew {
            task: held_task.clone(),
            lease_seconds: 30 * 60,
        },
    ));

    let renewed = board(bus, epoch, lane.clone());
    let after = at(&renewed, held_task).clone();
    assert_eq!(after.statement, before.statement);
    assert_eq!(after.workflow, before.workflow);
    assert_eq!(after.claim, before.claim);
    let renewed_lease = after.lease.as_ref().expect("renewed lease");
    assert_ne!(renewed_lease.position, lease.position);
    assert_eq!(renewed_lease.body.maximum, lease.body.maximum);
    assert!(renewed_lease.body.expiry > lease.body.expiry);
    assert!(renewed_lease.body.expiry <= renewed_lease.body.maximum);
    assert!(matches!(
        issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimRenew,
            expected,
            &ClaimRenew {
                task: held_task.clone(),
                lease_seconds: 30 * 60,
            },
        ),
        Outcome::Refusal(Refusal { ref reason, .. }) if reason == "board::witness-moved"
    ));
    assert!(
        bus.fact_snapshot()
            .expect("reclaim facts read")
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed)
    );
    after
}

pub(super) fn assert_reclaim_survives_restart(
    root: &Path,
    bus: Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    available_task: &TaskHandle,
    after: &BoardTask,
) {
    bus.verify().expect("reclaimed authority verifies");
    bus.shutdown().expect("reclaim fixture bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("reclaim journal reopens");
    let reopened = Bus::start(reopened).expect("reclaim bus restarts");
    let after_restart = board(&reopened, epoch, lane.clone());
    assert_eq!(at(&after_restart, held_task), after);
    let entries = actor_entries(&after_restart).expect("restarted board validates");
    assert_eq!(
        board::select(
            &entries,
            lane,
            allocation_subscription(&after_restart.residency.body, lane)
                .expect("restarted lane subscription"),
            after_restart.allocation_band,
        )
        .expect("allocator still has unclaimed work")
        .entry()
        .handle(),
        available_task,
        "the renewed claim remains excluded from allocation after restart",
    );
    clean(root, reopened);
}
