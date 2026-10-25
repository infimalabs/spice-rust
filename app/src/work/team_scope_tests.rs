use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;

use super::tests::{board, claim, design, stamp, task};
use super::{WorkOwner, WorkProjection, WorkScope};

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture uuid")
}

#[test]
fn team_scope_docks_only_active_member_work_without_changing_ownership() {
    let team = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let other_team = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let member = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    let other_member = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");

    let created = |value, project| {
        let mut row = task(value, project);
        row.creator_team = Some(team);
        row
    };
    let inactive = created(1, "alpha.inactive");
    let mut active = created(2, "alpha.active");
    let mut active_claim = claim(&active, ClaimDisposition::Active);
    active_claim.body.lane = member;
    active.claim = Some(active_claim);
    let mut released = created(3, "alpha.released");
    let mut released_claim = claim(&released, ClaimDisposition::Freed);
    released_claim.body.lane = member;
    released.claim = Some(released_claim);
    let mut moved = created(4, "alpha.moved");
    let mut moved_claim = claim(&moved, ClaimDisposition::Active);
    moved_claim.body.lane = other_member;
    moved.claim = Some(moved_claim);

    let board = board(vec![inactive, active, released, moved]);
    let pending = WorkProjection::from_board_and_lanes(&board, None);
    let scope = WorkScope::Team { team };
    assert_eq!(pending.contains_scope(&scope), None);
    assert_eq!(pending.scope_counts(&scope).open(), 0);

    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team, other_team])),
        BTreeMap::from([(member, team), (other_member, other_team)]),
    );
    assert_eq!(
        projection.scope_tasks(&scope),
        [stamp(2)],
        "creator provenance and released claims do not dock inactive work",
    );
    assert_eq!(projection.scope_counts(&scope).open(), 1);
    assert_eq!(
        projection.overview_for_scope(scope).tasks(),
        &[stamp(2)],
        "the team total selects exactly the tabs currently docked there",
    );
    assert_eq!(
        projection.scope_tasks(&WorkScope::Team { team: other_team }),
        [stamp(4)],
        "an active claim moves local placement to the claimant's team",
    );
    assert_eq!(
        projection
            .scope_tasks(&WorkScope::AllOpen)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([stamp(1), stamp(2), stamp(3), stamp(4)]),
        "local docking never removes execution work from its global namespace",
    );
    assert_eq!(
        projection.tasks()[0].accountable_owner(),
        Some(&WorkOwner::Team(team)),
    );
    assert_eq!(
        projection.tasks()[2].accountable_owner(),
        Some(&WorkOwner::Team(team)),
        "release changes local placement without erasing accountable ownership",
    );
}

#[test]
fn task_home_is_the_active_claimant_team_then_creator_then_last_claimant() {
    let team = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let other_team = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let member = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    let other_member = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");
    let unteamed_lane = uuid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");
    let homed = |value, creator: Option<CanonicalUuid>, claim_by| {
        let mut row = task(value, "alpha.task");
        row.creator_team = creator;
        if let Some((lane, disposition)) = claim_by {
            let mut head = claim(&row, disposition);
            head.body.lane = lane;
            row.claim = Some(head);
        }
        row
    };
    let board = board(vec![
        homed(
            1,
            Some(team),
            Some((other_member, ClaimDisposition::Active)),
        ),
        homed(
            2,
            Some(team),
            Some((unteamed_lane, ClaimDisposition::Active)),
        ),
        homed(3, Some(team), Some((other_member, ClaimDisposition::Freed))),
        homed(4, None, Some((other_member, ClaimDisposition::Lapsed))),
        homed(
            5,
            Some(uuid("ffffffff-ffff-4fff-8fff-ffffffffffff")),
            Some((member, ClaimDisposition::Freed)),
        ),
    ]);
    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team, other_team])),
        BTreeMap::from([(member, team), (other_member, other_team)]),
    );
    let homes = projection
        .tasks()
        .iter()
        .map(|task| {
            [team, other_team]
                .into_iter()
                .filter(|&candidate| task.belongs_to_team(candidate))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        homes,
        [
            vec![other_team],
            vec![],
            vec![team],
            vec![other_team],
            vec![team],
        ],
        "an active claim homes the task with the claimant's team, or no team for an \
         unteamed claimant; otherwise the roster creator team, then the last claimant's",
    );
}

#[test]
fn accountable_owner_partitions_design_planning_from_team_execution() {
    let creator = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let executor = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let member = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    let dissolved = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");
    let design_born = stamp(1);
    let plan_born = stamp(3);
    let preparation = |controller: wire::TaskStamp| wire::TaskPreparation {
        parent: controller.clone(),
        controller,
        construction: stamp(90),
    };

    let mut design = design(1, "rooms.design");
    design.creator_team = Some(creator);
    let mut design_claim = claim(&design, ClaimDisposition::Active);
    design_claim.body.lane = member;
    design.claim = Some(design_claim);

    let mut direct_output = task(2, "plan.candidate");
    direct_output.creator_team = Some(creator);
    direct_output.statement.body.intent = Some(wire::TaskIntent::Plan);
    direct_output.statement.body.preparation = Some(preparation(design_born.clone()));
    direct_output.statement.body.dependencies = vec![design_born.clone()];
    let mut active_claim = claim(&direct_output, ClaimDisposition::Active);
    active_claim.body.lane = member;
    direct_output.claim = Some(active_claim);

    let mut plan = task(3, "plan.followup");
    plan.creator_team = Some(creator);
    plan.statement.body.intent = Some(wire::TaskIntent::Plan);
    plan.statement.body.preparation = Some(preparation(design_born.clone()));
    plan.statement.body.dependencies = vec![design_born.clone()];

    // A Plan-created task inherits its Plan's Design as selection controller,
    // as task creation binds it, while its direct parent is the Plan.
    let mut execution = task(4, "app.execute");
    execution.creator_team = Some(executor);
    execution.statement.body.preparation = Some(wire::TaskPreparation {
        parent: plan_born,
        controller: design_born.clone(),
        construction: stamp(4),
    });
    execution.statement.body.dependencies = vec![stamp(3)];

    let mut rehomed = task(5, "app.rehomed");
    rehomed.creator_team = Some(dissolved);
    let mut past_claim = claim(&rehomed, ClaimDisposition::Freed);
    past_claim.body.lane = member;
    rehomed.claim = Some(past_claim);

    // Intake before the open design phase is ordinary claimed work.
    let mut intake = task(6, "rooms.intake");
    intake.creator_team = Some(creator);
    intake.statement.body.intent = Some(wire::TaskIntent::Design);
    intake.workflow.body.flow = ["plan", "design"]
        .map(|phase| phase.parse().expect("fixture phase"))
        .to_vec();
    intake.workflow.body.phase = "plan".parse().expect("fixture phase");

    let board = board(vec![
        design,
        direct_output,
        plan,
        execution,
        rehomed,
        intake,
    ]);
    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([creator, executor])),
        BTreeMap::from([(member, executor)]),
    );
    let borns = |scope: &WorkScope| {
        projection
            .scope_tasks(scope)
            .into_iter()
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(
        borns(&WorkScope::planning()),
        BTreeSet::from([stamp(1), stamp(2), stamp(3), stamp(4)]),
        "a Design owns its complete dependent subtree, including work a team created",
    );
    assert_eq!(
        borns(&WorkScope::AllOpen),
        BTreeSet::from([stamp(5), stamp(6)]),
        "Design intake and unrelated work remain execution-side without duplicating the subtree",
    );
    let team = WorkScope::Team { team: executor };
    let strip = projection
        .scope_strip(&team)
        .into_iter()
        .map(|task| task.row().statement.body.born.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        strip,
        projection.scope_tasks(&team),
        "a team total selects exactly the tabs its strip shows",
    );
    assert_eq!(
        borns(&team),
        BTreeSet::from([stamp(1), stamp(2)]),
        "a team holds the Design work its member claims, and accountable ownership alone docks nothing",
    );
    assert_eq!(projection.scope_counts(&team).open(), strip.len());
    assert_eq!(
        projection.tasks()[1].accountable_owner(),
        Some(&WorkOwner::Design),
    );
    assert_eq!(
        projection.tasks()[4].accountable_owner(),
        Some(&WorkOwner::Team(executor)),
        "a live last claimant can rehome work without changing its dissolved creator provenance",
    );
    assert_eq!(projection.tasks()[4].row().creator_team, Some(dissolved));
}

#[test]
fn a_shared_subtree_stays_on_the_planning_rail_until_its_last_design_edge_is_removed() {
    let first = design(1, "rooms.first");
    let second = design(2, "rooms.second");
    let mut shared = task(3, "rooms.shared");
    shared.statement.body.intent = Some(wire::TaskIntent::Plan);
    shared.statement.body.dependencies = vec![stamp(1), stamp(2)];
    let mut child = task(4, "app.child");
    child.statement.body.dependencies = vec![stamp(3), stamp(6)];
    let mut cycle = task(5, "app.cycle");
    cycle.statement.body.dependencies = vec![stamp(4)];
    let mut cycle_back = task(6, "app.cycle");
    cycle_back.statement.body.dependencies = vec![stamp(5)];

    let both_edges = board(vec![
        first.clone(),
        second.clone(),
        shared.clone(),
        child.clone(),
        cycle.clone(),
        cycle_back.clone(),
    ]);
    let projection = WorkProjection::from_board(&both_edges);
    assert_eq!(
        projection
            .scope_tasks(&WorkScope::planning())
            .into_iter()
            .collect::<BTreeSet<_>>(),
        [1, 2, 3, 4, 5, 6].map(stamp).into_iter().collect(),
        "the Design rail contains the complete subtree and terminates across its cycle",
    );

    shared.statement.body.dependencies = vec![stamp(2)];
    let one_edge = board(vec![
        first.clone(),
        second.clone(),
        shared.clone(),
        child.clone(),
        cycle.clone(),
        cycle_back.clone(),
    ]);
    let projection = WorkProjection::from_board(&one_edge);
    assert!(
        projection.tasks()[3..]
            .iter()
            .all(|task| task.belongs_to_design()),
        "selection by one Design leaves the other Design's whole subtree docked",
    );

    shared.statement.body.dependencies.clear();
    let no_edges = board(vec![first, second, shared, child, cycle, cycle_back]);
    let projection = WorkProjection::from_board(&no_edges);
    assert!(
        projection.tasks()[2..]
            .iter()
            .all(|task| !task.belongs_to_design()),
        "removing the final Design edge returns the whole subtree to execution",
    );
}

#[test]
fn opportunistic_design_owns_its_complete_plan_subtree() {
    let root_born = stamp(1);
    let preparation = |parent: wire::TaskStamp, construction| wire::TaskPreparation {
        parent,
        controller: root_born.clone(),
        construction: stamp(construction),
    };
    let mut root = design(1, "spice.oops");
    root.statement.body.intent = Some(wire::TaskIntent::OpportunisticDesign);
    let mut output = task(2, "spice.oops");
    output.statement.body.intent = Some(wire::TaskIntent::Plan);
    output.statement.body.preparation = Some(preparation(root_born.clone(), 2));
    output.statement.body.dependencies = vec![root_born.clone()];
    let mut execution = task(3, "spice.app");
    execution.statement.body.preparation = Some(preparation(stamp(2), 3));
    execution.statement.body.dependencies = vec![stamp(2)];

    let board = board(vec![root, output, execution]);
    let projection =
        WorkProjection::from_board_with_team_memberships(&board, None, BTreeMap::new());
    let borns = |scope: &WorkScope| {
        projection
            .scope_tasks(scope)
            .into_iter()
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(
        borns(&WorkScope::planning()),
        BTreeSet::from([stamp(1), stamp(2), stamp(3)]),
        "the Design rail contains the opportunistic Design's complete dependency subtree",
    );
    let execution_project = WorkScope::Project("spice.app".to_owned());
    for scope in [WorkScope::AllOpen, execution_project] {
        assert!(
            borns(&scope).is_empty(),
            "{scope:?} does not duplicate Design-gated execution work",
        );
    }
    assert_eq!(
        projection.tasks()[1].accountable_owner(),
        Some(&WorkOwner::Design),
    );
}

#[test]
fn group_scope_docks_only_active_work_associated_with_its_designs() {
    let design_born = stamp(1);
    let second_lane = uuid("22222222-2222-4222-8222-222222222222");
    let group = "dVxJ7j0vj"
        .parse::<wire::rooms::RoomAuthorityStamp>()
        .expect("fixture group");
    let preparation = || wire::TaskPreparation {
        parent: design_born.clone(),
        controller: design_born.clone(),
        construction: design_born.clone(),
    };
    let activate = |row: &mut wire::intent::BoardTask| {
        row.claim = Some(claim(row, ClaimDisposition::Active));
    };

    let mut active_design = design(1, "rooms.design");
    activate(&mut active_design);
    let mut active_output = task(2, "rooms.active");
    active_output.statement.body.preparation = Some(preparation());
    activate(&mut active_output);
    active_output
        .claim
        .as_mut()
        .expect("active output claim")
        .body
        .lane = second_lane;
    let mut second_active_output = task(5, "rooms.active");
    second_active_output.statement.body.dependencies = vec![design_born.clone()];
    activate(&mut second_active_output);
    let mut inactive_output = task(3, "rooms.inactive");
    inactive_output.statement.body.preparation = Some(preparation());
    inactive_output.statement.body.dependencies = vec![design_born.clone()];
    let mut unrelated = task(4, "rooms.unrelated");
    activate(&mut unrelated);
    let other_design = design(6, "rooms.other-design");
    let mut other_selected_output = task(7, "rooms.other-selected");
    other_selected_output.statement.body.preparation = Some(wire::TaskPreparation {
        parent: stamp(6),
        controller: stamp(6),
        construction: stamp(7),
    });
    activate(&mut other_selected_output);

    let board = board(vec![
        active_design,
        active_output,
        inactive_output,
        unrelated,
        second_active_output,
        other_design,
        other_selected_output,
    ]);
    let projection = WorkProjection::from_board(&board);
    let scope = WorkScope::Group {
        group,
        designs: vec![design_born],
    };
    let (group_counts, workers) = projection.group_worker_strip(&scope);
    assert_eq!(
        projection
            .scope_tasks(&scope)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([stamp(1), stamp(2), stamp(5)]),
        "selected and currently gated work enter only their controller's group, and inactive work does not dock a tab",
    );
    assert_eq!(group_counts.open(), 3);
    assert_eq!(
        workers
            .into_iter()
            .map(|(lane, task)| (lane, task.row().statement.body.born.clone()))
            .collect::<Vec<_>>(),
        [
            (uuid("11111111-1111-4111-8111-111111111111"), stamp(1),),
            (second_lane, stamp(2)),
        ],
        "every active task contributes to the total while the group rail presents each working lane once in active-task strip order",
    );
    assert_eq!(
        projection
            .scope_tasks(&WorkScope::planning())
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([stamp(1), stamp(3), stamp(5), stamp(6)]),
        "only current dependency reachability remains in the global planning namespace",
    );
}
