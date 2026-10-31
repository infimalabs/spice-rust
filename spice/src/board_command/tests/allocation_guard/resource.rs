use super::super::fixture::{authored_add, issue_add};
use super::*;

fn resource_task(bus: &Bus, epoch: CanonicalUuid, name: &str, title: &str) -> TaskHandle {
    let mut add = authored_add(
        title,
        "board.test",
        Some("todo,review"),
        &["the fixture work is verified"],
    );
    add.requirements = wire::EnvironmentRequirements::new(vec![
        wire::EnvironmentRequirement::resource_pool(
            "emulator".parse().expect("fixture resource name"),
            2,
        )
        .expect("fixture shard count"),
        wire::EnvironmentRequirement::resource_pool(
            "windows-guest".parse().expect("fixture resource name"),
            1,
        )
        .expect("fixture shard count"),
    ])
    .expect("fixture resource declarations are canonical");
    issue_add(bus, epoch, Actor::Operator, name, add)
}

fn resource_surfaces(root: &std::path::Path) -> (resources::ResourceClaims, sill::LockTopology) {
    let config: sill::LockConfig = toml::from_str(
        "busy = 71\ntaken = 72\nfull = 73\n[emulator]\ndirectory = \"resource/emulator\"\nshards = 2\n[windows-guest]\ndirectory = \"resource/windows-guest\"\nshards = 1\n",
    )
    .expect("fixture resource configuration");
    let root = root
        .canonicalize()
        .expect("fixture authority root canonicalizes");
    (
        resources::ResourceClaims::new(sill::LockTopology::new(&root, &config)),
        sill::LockTopology::new(&root, &config),
    )
}

fn next_with_resources(
    bus: &Bus,
    actor: Actor,
    resources: &resources::ResourceClaims,
) -> BoardAnswer {
    let observed = bus.task_snapshot().expect("fixture snapshot reads");
    read_guard::next_work::read_next_work_with_resources(
        bus,
        &observed,
        actor,
        &board::NextConfig::default(),
        None,
        Some(resources),
    )
    .expect("resource-aware allocation reads")
}

fn candidate(answer: &BoardAnswer) -> Option<TaskHandle> {
    match answer.next.as_ref() {
        Some(NextWorkState::Candidate { task }) => Some(TaskHandle::for_filing(
            &task.statement.body.project,
            &task.statement.body.born,
            task.statement.body.filing,
        )),
        _ => None,
    }
}

fn delete_task(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let visible = board(bus, epoch, Actor::Operator);
    let heads = at(&visible, task);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDelete,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskDelete {
            task: task.clone(),
            reason: "resource sequence advanced".to_owned(),
            force_claimed: false,
        },
    ));
}

fn assert_declared_resources(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let listed = board(bus, epoch, Actor::Operator);
    assert_eq!(
        at(&listed, task)
            .statement
            .body
            .resources
            .iter()
            .map(|(name, shards)| (name.as_str(), shards))
            .collect::<Vec<_>>(),
        [("emulator", 2), ("windows-guest", 1)]
    );
}

fn assert_capacity_gates(
    bus: &Bus,
    actor: &Actor,
    resources: &resources::ResourceClaims,
    external: &sill::LockTopology,
) {
    let external_guard = match external
        .try_acquire("windows-guest", None)
        .expect("fixture resource acquisition")
    {
        sill::LockAttempt::Acquired(shard) => shard,
        sill::LockAttempt::Contended { .. } => panic!("fresh fixture resource is free"),
    };
    let blocked = next_with_resources(bus, actor.clone(), resources);
    assert_eq!(candidate(&blocked), None);
    assert_eq!(
        blocked
            .resource_pools
            .get(&"windows-guest".parse().expect("fixture resource name")),
        Some(&0),
    );
    drop(external_guard);
    let emulator_guard = match external
        .try_acquire("emulator", None)
        .expect("fixture resource acquisition")
    {
        sill::LockAttempt::Acquired(shard) => shard,
        sill::LockAttempt::Contended { .. } => panic!("fresh fixture resource is free"),
    };
    let below_count = next_with_resources(bus, actor.clone(), resources);
    assert_eq!(candidate(&below_count), None);
    assert_eq!(
        below_count
            .resource_pools
            .get(&"emulator".parse().expect("fixture resource name")),
        Some(&1),
    );
    drop(emulator_guard);
}

fn claim_after_all_or_none_race(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    resources: &resources::ResourceClaims,
    external: &sill::LockTopology,
    lane: CanonicalUuid,
) {
    let race_guard = match external
        .try_acquire("windows-guest", None)
        .expect("race fixture resource acquisition")
    {
        sill::LockAttempt::Acquired(shard) => shard,
        sill::LockAttempt::Contended { .. } => panic!("candidate resource was still free"),
    };
    let raced = claim_win(
        bus,
        command,
        &next_rule(),
        None,
        &board::NextConfig::default(),
        None,
        Some(resources),
    );
    assert!(
        matches!(&raced, Outcome::Refusal(_)),
        "claim must refuse after complete availability moves: {raced:?}"
    );
    let after_race = external.report().expect("partial reservation reports");
    let emulator = after_race
        .pools
        .iter()
        .find(|pool| pool.name == "emulator")
        .expect("emulator resource reports");
    assert!(
        emulator
            .entries
            .iter()
            .all(|entry| entry.availability == sill::LockAvailability::Available),
        "failed all-or-none reservation must release earlier pools"
    );
    drop(race_guard);
    claim_resource_task(bus, command, resources);
    let report = external.report().expect("claimed resources report");
    let windows = report
        .pools
        .iter()
        .find(|pool| pool.name == "windows-guest")
        .expect("windows resource reports");
    let owner = windows.entries[0]
        .owner
        .as_ref()
        .expect("claim-held shard names its owner");
    let context: wire::ResourceClaimOwner = serde_json::from_str(
        owner
            .context
            .as_deref()
            .expect("claim owner context is present"),
    )
    .expect("claim owner context is typed");
    assert_eq!(context.lane, lane);
}

fn claim_resource_task(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    resources: &resources::ResourceClaims,
) {
    assert!(matches!(
        claim_win(
            bus,
            command,
            &next_rule(),
            None,
            &board::NextConfig::default(),
            None,
            Some(resources),
        ),
        Outcome::Reply(_)
    ));
}

fn restored_claims(
    root: &std::path::Path,
    bus: &Bus,
    external: &sill::LockTopology,
) -> resources::ResourceClaims {
    let (resources, _) = resource_surfaces(root);
    resources
        .restore(bus)
        .expect("resident restart restores claim resource");
    let restored = external.report().expect("restored resources report");
    assert!(restored.pools.iter().all(|pool| {
        pool.entries
            .iter()
            .all(|entry| entry.availability == sill::LockAvailability::Held)
    }));
    resources
}

#[test]
fn one_shard_serializes_allocation_and_releases_after_claim_free() {
    let (root, bus, epoch, seats) = fixture("resource-allocation", 2);
    for seat in &seats {
        authorize_lane_creation(&bus, epoch, seat);
    }
    let first = resource_task(&bus, epoch, "RESOURCE-1", "first resource task");
    let second = resource_task(&bus, epoch, "RESOURCE-2", "second resource task");
    let third = resource_task(&bus, epoch, "RESOURCE-3", "third resource task");
    assert_declared_resources(&bus, epoch, &first);
    let (resources, external) = resource_surfaces(&root);
    let first_actor = Actor::Lane(seats[0].lane);
    let second_actor = Actor::Lane(seats[1].lane);
    assert_capacity_gates(&bus, &first_actor, &resources, &external);
    let available = next_with_resources(&bus, first_actor.clone(), &resources);
    assert_eq!(candidate(&available), Some(first.clone()));
    assert_eq!(
        available
            .resource_pools
            .get(&"windows-guest".parse().expect("fixture resource name")),
        Some(&1),
    );
    let first_board = board(&bus, epoch, first_actor.clone());
    let first_command = claim_next_command(epoch, &first_actor, &first_board, &first, &seats[0]);
    claim_after_all_or_none_race(&bus, &first_command, &resources, &external, seats[0].lane);
    drop(resources);
    let resources = restored_claims(&root, &bus, &external);
    assert_eq!(
        candidate(&next_with_resources(&bus, second_actor.clone(), &resources)),
        None,
    );
    super::super::refresh::free_active(&bus, epoch, &first_actor, &seats[0], &first);
    delete_task(&bus, epoch, &first);
    assert_eq!(
        candidate(&next_with_resources(&bus, second_actor.clone(), &resources)),
        Some(second.clone()),
    );
    let second_board = board(&bus, epoch, second_actor.clone());
    let second_command =
        claim_next_command(epoch, &second_actor, &second_board, &second, &seats[1]);
    claim_resource_task(&bus, &second_command, &resources);
    assert_eq!(
        candidate(&next_with_resources(&bus, first_actor.clone(), &resources)),
        None,
    );
    super::super::refresh::free_active(&bus, epoch, &second_actor, &seats[1], &second);
    delete_task(&bus, epoch, &second);
    assert_eq!(
        candidate(&next_with_resources(&bus, first_actor, &resources)),
        Some(third),
    );
    clean(&root, bus);
}
