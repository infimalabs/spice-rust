use std::collections::BTreeSet;

use super::*;

pub(super) fn configure_subscription(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    policy: wire::intent::TeamLife,
) {
    let snapshot = bus.task_snapshot().expect("subscription head reads");
    let observed = snapshot.residency().expect("subscription head exists");
    let mut next = observed.body().clone();
    let subscription = next
        .subscriptions
        .iter_mut()
        .find(|subscription| subscription.team == seat.team)
        .expect("fixture team subscription exists");
    let from = subscription.policy;
    subscription.policy = policy;
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("fixture resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::ResidentRecognition);
    let configured = write
        .fact(
            wire::body::TeamConfigured {
                team: seat.team,
                member: None,
                from_team: None,
                source_rekeyed_to: None,
                from,
                life: policy,
                from_fast_mode: false,
                fast_mode: false,
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

pub(super) fn move_fixture_lane_to_team(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    team: CanonicalUuid,
) {
    let snapshot = bus
        .task_snapshot()
        .expect("residency head reads before move");
    let observed = snapshot
        .residency()
        .expect("residency head exists before move");
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == seat.lane)
        .expect("fixture lane remains seated");
    let from_team = row.team;
    row.team = team;
    next.topology_revision += 1;
    next.subscriptions
        .retain(|subscription| subscription.team != from_team);
    next.subscriptions.push(wire::TeamSubscription {
        team,
        policy: wire::intent::TeamLife::Drive,
        fast_mode: false,
    });

    let origin = format!("resident:{epoch}")
        .parse()
        .expect("fixture resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::ResidentRecognition);
    let configured = write
        .fact(
            wire::body::TeamConfigured {
                team,
                member: Some(seat.lane),
                from_team: Some(from_team),
                source_rekeyed_to: None,
                from: wire::intent::TeamLife::Drive,
                life: wire::intent::TeamLife::Drive,
                from_fast_mode: false,
                fast_mode: false,
                sources: Vec::new(),
            },
            FactCause::Root,
        )
        .expect("fixture team move fact builds");
    write.residency_head(configured, move |_| next);
    match bus.commit(write).expect("team move reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("team move did not commit: {outcome:?}"),
    }
}

pub(super) fn selectable_handles(answer: &BoardAnswer, actor: &Actor) -> BTreeSet<String> {
    let entries = actor_entries(answer).expect("scoped board entries validate");
    let subscription = allocation_subscription(&answer.residency.body, actor)
        .expect("scoped board has one team subscription");
    let environment = answer.environment();
    let satisfied = board::satisfied_tasks(&entries);
    entries
        .iter()
        .filter(|entry| {
            board::selectable_entry(&satisfied, entry, actor, &subscription, &environment)
        })
        .map(|entry| entry.handle().to_string())
        .collect()
}

fn assert_scope(
    answer: &BoardAnswer,
    actor: &Actor,
    policy: wire::intent::TeamLife,
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
    same_team: TaskHandle,
    cross_team: TaskHandle,
    operator: TaskHandle,
}

fn seed_scoped_tasks(
    bus: &Bus,
    epoch: CanonicalUuid,
    same_lane: &Actor,
    cross_lane: &Actor,
) -> ScopedTasks {
    let same_team = add_project(
        bus,
        epoch,
        same_lane.clone(),
        "SCOPE-manual1",
        "Same-team work",
        "task.manual",
        Some("todo"),
    );
    let cross_team = add_project(
        bus,
        epoch,
        cross_lane.clone(),
        "SCOPE-learn01",
        "Cross-team work",
        "task.learned",
        Some("todo"),
    );
    let operator = add_project(
        bus,
        epoch,
        Actor::Operator,
        "SCOPE-open001",
        "Operator work",
        "task.unrelated",
        Some("todo"),
    );
    ScopedTasks {
        same_team,
        cross_team,
        operator,
    }
}

#[test]
fn team_lifetimes_use_immutable_creator_team_and_survive_restart() {
    let (root, bus, epoch, seats) = fixture("creator-team-scopes", 2);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let peer = Actor::Lane(seats[1].lane);
    authorize_lane_creation(&bus, epoch, seat);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Steer);
    let ScopedTasks {
        same_team,
        cross_team,
        operator,
    } = seed_scoped_tasks(&bus, epoch, &lane, &peer);

    let steer = board(&bus, epoch, lane.clone());
    assert_scope(&steer, &lane, wire::intent::TeamLife::Steer, &[]);

    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Drive);
    let drive = board(&bus, epoch, lane.clone());
    assert_scope(&drive, &lane, wire::intent::TeamLife::Drive, &[&same_team]);

    let next_rule: Name = "next".parse().expect("allocator rule");
    let stale_next = Command {
        v: WireVersion::CURRENT,
        id: uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed)),
        epoch,
        kind: CommandKind::ClaimWin,
        expected: claim_expected(&drive, &same_team, seat),
        body: serde_json::to_value(&ClaimWin {
            rule: next_rule.clone(),
            recovery: None,
        })
        .expect("stale allocator command encodes"),
    }
    .admit(&Accepted {
        wire: WireVersion::CURRENT,
        actor: lane.clone(),
        epoch,
    })
    .expect("stale allocator command admits");
    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Drain);
    let stale = dispatch(&bus, &stale_next).expect("board owns stale allocator command");
    assert!(
        matches!(
            stale,
            Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"
        ),
        "a command prepared under Drive crossed into Drain: {stale:?}",
    );
    let drain = board(&bus, epoch, lane.clone());
    assert_scope(
        &drain,
        &lane,
        wire::intent::TeamLife::Drain,
        &[&cross_team, &operator, &same_team],
    );

    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Drive);

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
        wire::intent::TeamLife::Drive,
        &[&same_team],
    );
    clean(&root, reopened);
}

#[test]
fn drive_does_not_move_existing_work_with_its_creator_lane() {
    let (root, bus, epoch, seats) = fixture("creator-team-move", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Drive);
    let task = add_project(
        &bus,
        epoch,
        lane.clone(),
        "SCOPE-stable1",
        "Creator-team ownership stays fixed",
        "task.stable",
        Some("todo"),
    );

    let before = board(&bus, epoch, lane.clone());
    assert_scope(&before, &lane, wire::intent::TeamLife::Drive, &[&task]);
    let row = before
        .tasks
        .iter()
        .find(|row| row.statement.body.handle == task)
        .expect("created task remains on the board");
    assert_eq!(row.creator_team, Some(seat.team));

    let moved_team = uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed));
    move_fixture_lane_to_team(&bus, epoch, seat, moved_team);
    let after = board(&bus, epoch, lane.clone());
    assert_scope(&after, &lane, wire::intent::TeamLife::Drive, &[]);
    let row = after
        .tasks
        .iter()
        .find(|row| row.statement.body.handle == task)
        .expect("old task remains readable after its creator moves");
    assert_eq!(
        row.creator_team,
        Some(seat.team),
        "creator-team ownership is the TaskAdded-time team, not current membership",
    );
    clean(&root, bus);
}

/// An agent seated alone under its own identity is not on a team.
///
/// Its configured lifetime stays durable, but allocation honours it only for
/// a team: alone, the lane selects nothing, exactly as Steer would, while its
/// seat keeps running. Joining a team resumes the configured lifetime.
#[test]
fn an_agent_without_a_team_allocates_as_steer_until_it_joins_one() {
    let (root, bus, epoch, seats) = fixture("unteamed-allocation", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Drive);
    let task = add_project(
        &bus,
        epoch,
        lane.clone(),
        "SCOPE-alone01",
        "Work the team created",
        "task.alone",
        Some("todo"),
    );
    let teamed = board(&bus, epoch, lane.clone());
    assert_scope(&teamed, &lane, wire::intent::TeamLife::Drive, &[&task]);

    move_fixture_lane_to_team(&bus, epoch, seat, seat.lane);
    let alone = board(&bus, epoch, lane.clone());
    assert_eq!(
        alone
            .residency
            .body
            .subscriptions
            .iter()
            .find(|subscription| subscription.team == seat.lane)
            .map(|subscription| subscription.policy),
        Some(wire::intent::TeamLife::Drive),
        "the configured lifetime stays durable while the agent is alone",
    );
    assert_scope(&alone, &lane, wire::intent::TeamLife::Steer, &[]);
    assert_eq!(
        allocated(&bus, epoch, &lane),
        None,
        "an agent without a team is handed no work",
    );

    move_fixture_lane_to_team(&bus, epoch, seat, seat.team);
    let rejoined = board(&bus, epoch, lane.clone());
    assert_scope(&rejoined, &lane, wire::intent::TeamLife::Drive, &[&task]);
    assert_eq!(
        allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
        Some(task.to_string()),
        "joining a team restores the configured lifetime",
    );
    clean(&root, bus);
}

/// Reading remains independent from Steer's empty automatic allocation.
///
/// Public stems decide what a lane can read. The allocation lifetime decides
/// what `task next` can hand it. Steer therefore leaves visible work readable
/// while selecting none, so explicit task capture and correction remain
/// possible without turning project names into allocation predicates.
#[test]
fn steer_keeps_public_stem_reading_while_automatic_allocation_is_empty() {
    let (_root, bus, epoch, seats) = fixture("subscription-read-gap", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    configure_subscription(&bus, epoch, seat, wire::intent::TeamLife::Steer);
    let elsewhere = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "SCOPE-gap0001",
        "Visible while automatic allocation is empty",
        "rim.elsewhere",
        Some("todo"),
    );

    let answer = board(&bus, epoch, lane.clone());
    assert!(
        selectable_handles(&answer, &lane).is_empty(),
        "Steer does not automatically select visible work",
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
