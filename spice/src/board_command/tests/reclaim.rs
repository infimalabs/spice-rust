use super::*;

fn reclaim_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    expected: Vec<Witness>,
    at: Instant,
) -> Outcome<CommandAnswer> {
    let renew = ClaimRenew {
        task: held_task.clone(),
        lease_seconds: 90 * 60,
    };
    let command = fixture_command(
        epoch,
        lane.clone(),
        CommandKind::ClaimRenew,
        expected,
        &renew,
    );
    claim_reclaim_with_clock(
        bus,
        &command,
        &renew,
        &guard_projects(),
        &board::NextConfig::default(),
        || Ok(at),
    )
}

pub(super) fn renew_and_assert(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    before: &BoardTask,
    expected: Vec<Witness>,
) -> BoardTask {
    let lease = before.lease.as_ref().expect("active lease");
    let first_at = fixture_instant_after(lease.body.start, 1);
    let answer = assert_committed(reclaim_at(
        bus,
        epoch,
        lane,
        held_task,
        expected.clone(),
        first_at,
    ));

    let renewed = board(bus, epoch, lane.clone());
    let after = at(&renewed, held_task).clone();
    assert_eq!(after.statement, before.statement);
    assert_eq!(after.workflow, before.workflow);
    assert_eq!(after.claim, before.claim);
    let renewed_lease = after.lease.as_ref().expect("renewed lease");
    let CommandAnswer::Claim(answer) = answer else {
        panic!("reclaim returned a non-claim answer")
    };
    assert_eq!(answer.expiry.as_deref(), Some(&renewed_lease.body.expiry));
    assert_ne!(renewed_lease.position, lease.position);
    assert_eq!(renewed_lease.body.maximum, lease.body.maximum);
    assert!(renewed_lease.body.expiry > lease.body.expiry);
    assert!(renewed_lease.body.expiry <= renewed_lease.body.maximum);
    let mut current = expected.clone();
    for witness in &mut current {
        if let Witness::Lease { head, .. } = witness {
            *head = renewed_lease.position;
        }
    }
    assert!(matches!(
        reclaim_at(bus, epoch, lane, held_task, current, first_at),
        Outcome::Refusal(ref refusal) if refusal.reason() == "claim::lease-nonextending"
    ));
    assert!(matches!(
        reclaim_at(
            bus,
            epoch,
            lane,
            held_task,
            expected,
            fixture_instant_after(first_at, 1),
        ),
        Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"
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
            after_restart.recent_cell.as_ref(),
        )
        .expect("allocator still has unclaimed work")
        .entry()
        .handle(),
        available_task,
        "the renewed claim remains excluded from allocation after restart",
    );
    clean(root, reopened);
}
