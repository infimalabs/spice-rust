use std::collections::BTreeSet;

use super::*;

pub(super) fn configure_subscription(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    policy: wire::intent::LineageLife,
    manual_projects: &[&str],
) {
    let snapshot = bus.task_snapshot().expect("subscription head reads");
    let observed = snapshot.residency().expect("subscription head exists");
    let mut next = observed.body().clone();
    let subscription = next
        .subscriptions
        .iter_mut()
        .find(|subscription| subscription.lineage_id == seat.lane)
        .expect("fixture lineage subscription exists");
    let from = subscription.policy;
    let from_manual_projects = subscription.manual_projects.clone();
    subscription.policy = policy;
    subscription.manual_projects = manual_projects
        .iter()
        .map(|project| (*project).to_owned())
        .collect();
    subscription.manual_projects.sort();
    let configured_manual_projects = subscription.manual_projects.clone();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("fixture resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::ResidentRecognition);
    let configured = write
        .fact(
            wire::body::LineageConfigured {
                lineage_id: seat.lane,
                member: None,
                from_lineage: None,
                source_rekeyed_to: None,
                from,
                life: policy,
                from_manual_projects,
                manual_projects: configured_manual_projects,
                sources: Vec::new(),
            },
            FactCause::Root,
        )
        .expect("fixture subscription fact builds");
    write.residency_head(configured, move |_| next);
    match bus
        .commit(write)
        .expect("subscription change reaches writer")
    {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("subscription change did not commit: {outcome:?}"),
    }
}

pub(super) fn selectable_handles(answer: &BoardAnswer, actor: &Actor) -> BTreeSet<String> {
    let entries = actor_entries(answer).expect("scoped board entries validate");
    let subscription = allocation_subscription(&answer.residency.body, actor)
        .expect("scoped board has one lineage subscription");
    entries
        .iter()
        .filter(|entry| board::selectable_entry(&entries, entry, actor, subscription))
        .map(|entry| entry.handle().to_string())
        .collect()
}

fn assert_scope(
    answer: &BoardAnswer,
    actor: &Actor,
    policy: wire::intent::LineageLife,
    expected: &[&TaskHandle],
) {
    assert_eq!(
        selectable_handles(answer, actor),
        expected.iter().map(|handle| handle.to_string()).collect(),
    );
    assert_eq!(answer.residency.body.lanes[0].life, "drive");
    assert_eq!(answer.residency.body.lanes[0].disposition, "occupied");
    let subscription = allocation_subscription(&answer.residency.body, actor)
        .expect("scoped board has one durable lineage subscription");
    assert_eq!(subscription.policy, policy);
}

struct ScopedTasks {
    manual: TaskHandle,
    learned: TaskHandle,
    unrelated: TaskHandle,
}

fn seed_scoped_tasks(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, lane: &Actor) -> ScopedTasks {
    let manual = add_project(
        bus,
        epoch,
        Actor::Operator,
        "SCOPE-manual1",
        "Operator-pinned work",
        "task.manual",
        Some("todo"),
    );
    let learned = add_project(
        bus,
        epoch,
        lane.clone(),
        "SCOPE-learn01",
        "Lineage-created work",
        "task.learned",
        Some("todo"),
    );
    let unrelated = add_project(
        bus,
        epoch,
        Actor::Operator,
        "SCOPE-open001",
        "Unrelated assignable work",
        "task.unrelated",
        Some("todo"),
    );
    let unassignable = add_project(
        bus,
        epoch,
        Actor::Operator,
        "SCOPE-held001",
        "Already-held work",
        "task.unassignable",
        Some("todo"),
    );
    capture_for(bus, epoch, seat, &unassignable);
    ScopedTasks {
        manual,
        learned,
        unrelated,
    }
}

#[test]
fn resident_subscription_scopes_learn_projects_and_survive_restart_without_changing_lane_life() {
    let (root, bus, epoch, seats) = fixture("subscription-scopes", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    configure_subscription(
        &bus,
        epoch,
        seat,
        wire::intent::LineageLife::Steer,
        &["task.manual"],
    );
    let ScopedTasks {
        manual,
        learned,
        unrelated,
    } = seed_scoped_tasks(&bus, epoch, seat, &lane);

    let steer = board(&bus, epoch, lane.clone());
    assert_scope(&steer, &lane, wire::intent::LineageLife::Steer, &[&manual]);
    let steer_subscription = allocation_subscription(&steer.residency.body, &lane)
        .expect("steer subscription remains durable");
    assert_eq!(
        steer_subscription.auto_projects,
        ["task.learned", "task.unassignable"],
    );

    let next_rule: Name = "next".parse().expect("allocator rule");
    let stale_next = Command {
        v: WireVersion::CURRENT,
        id: uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed)),
        epoch,
        kind: CommandKind::ClaimWin,
        expected: claim_expected(&steer, &manual, seat),
        body: serde_json::to_value(&ClaimWin {
            rule: next_rule.clone(),
        })
        .expect("stale allocator command encodes"),
    }
    .admit(&Accepted {
        wire: WireVersion::CURRENT,
        actor: lane.clone(),
        epoch,
    })
    .expect("stale allocator command admits");
    configure_subscription(
        &bus,
        epoch,
        seat,
        wire::intent::LineageLife::Drive,
        &["task.manual"],
    );
    let stale = dispatch(&bus, &stale_next).expect("board owns stale allocator command");
    assert!(
        matches!(
            stale,
            Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"
        ),
        "a command prepared under Steer crossed into Drive: {stale:?}",
    );
    let drive = board(&bus, epoch, lane.clone());
    assert_scope(
        &drive,
        &lane,
        wire::intent::LineageLife::Drive,
        &[&manual, &learned],
    );

    configure_subscription(
        &bus,
        epoch,
        seat,
        wire::intent::LineageLife::Drain,
        &["task.manual"],
    );
    let drain = board(&bus, epoch, lane.clone());
    assert_scope(
        &drain,
        &lane,
        wire::intent::LineageLife::Drain,
        &[&learned, &manual, &unrelated],
    );

    bus.verify()
        .expect("scoped authority verifies before restart");
    bus.shutdown().expect("scoped fixture bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("scoped journal reopens");
    let reopened = Bus::start(reopened).expect("scoped bus restarts");
    let restarted = board(&reopened, epoch, lane.clone());
    assert_scope(
        &restarted,
        &lane,
        wire::intent::LineageLife::Drain,
        &[&learned, &manual, &unrelated],
    );
    let subscription = allocation_subscription(&restarted.residency.body, &lane)
        .expect("drain subscription survives restart");
    assert_eq!(subscription.manual_projects, ["task.manual"]);
    assert_eq!(
        subscription.auto_projects,
        ["task.learned", "task.unassignable"],
    );
    clean(&root, reopened);
}

/// Reading stays wider than allocation, and the gap is the invariant.
///
/// A row filed outside every project this lane's subscription admits is still
/// readable by it, because allocation matches the subscription exactly while
/// reading matches a public stem by prefix. The gap is what keeps a misfiled
/// task fixable: the lane that can no longer be handed the row can still see
/// it, and so can still move it back. Without the gap the board would have an
/// absorbing state.
///
/// No correction runs here. The edit suite observes this same pair, but only
/// as the cost of one `task modify` call, so it would stop guarding the rule
/// the moment that command changed shape. This drives both surfaces for a
/// task that was simply filed elsewhere to begin with, so that
/// narrowing reading to the subscription, or widening allocation to the stems,
/// reds the invariant on its own terms.
#[test]
fn reading_stays_wider_than_allocation_for_a_row_outside_the_subscription() {
    let (_root, bus, epoch, seats) = fixture("subscription-read-gap", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    configure_subscription(
        &bus,
        epoch,
        seat,
        wire::intent::LineageLife::Steer,
        &["task.manual"],
    );
    let elsewhere = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "SCOPE-gap0001",
        "Filed outside this lane's subscription",
        "rim.elsewhere",
        Some("todo"),
    );

    let answer = board(&bus, epoch, lane.clone());
    assert!(
        selectable_handles(&answer, &lane).is_empty(),
        "allocation is subscription scoped: rim.elsewhere is not task.manual",
    );
    assert_eq!(
        allocated(&bus, epoch, &lane),
        None,
        "so the allocator has nothing to hand this lane",
    );

    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    assert!(
        listed.contains(&format!("{elsewhere} [todo] rim.elsewhere")),
        "reading is stem scoped and stays wider, so the row this lane cannot be handed is still visible to fix: {listed}",
    );
}
