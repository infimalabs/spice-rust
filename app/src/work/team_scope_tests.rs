use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;

use super::tests::{board, claim, stamp, task};
use super::{WorkOwner, WorkProjection, WorkScope, WorkStemIdentity, WorkVisibility};

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture uuid")
}

fn public(stem: &str) -> WorkStemIdentity {
    WorkStemIdentity::new(WorkVisibility::Public, stem.to_owned())
}

#[test]
fn team_scope_waits_for_the_roster_then_homes_creator_and_member_claimed_tasks() {
    let team = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let member = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let other_team = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");

    let mut created = task(1, "alpha.created");
    created.creator_team = Some(team);
    let mut claimed = task(2, "beta.claimed");
    let mut member_claim = claim(&claimed, ClaimDisposition::Active);
    member_claim.body.lane = member;
    claimed.claim = Some(member_claim);
    let mut both = task(3, "gamma.both");
    both.creator_team = Some(team);
    let mut both_claim = claim(&both, ClaimDisposition::Freed);
    both_claim.body.lane = member;
    both.claim = Some(both_claim);
    let mut unrelated = task(4, "delta.unrelated");
    unrelated.creator_team = Some(other_team);

    let board = board(vec![created, claimed, both, unrelated]);
    let pending = WorkProjection::from_board_and_lanes(&board, None);
    let scope = WorkScope::team_wide(team);
    assert_eq!(pending.contains_scope(&scope), None);
    assert_eq!(pending.scope_counts(&scope).open(), 0);

    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team])),
        &BTreeMap::from([(member, team)]),
    );
    assert_eq!(projection.contains_scope(&scope), Some(true));
    assert_eq!(
        projection.contains_scope(&WorkScope::team_wide(other_team)),
        Some(false)
    );
    assert_eq!(projection.scope_counts(&scope).open(), 3);
    assert_eq!(
        projection
            .tasks()
            .iter()
            .filter(|task| projection.in_scope(task, &scope))
            .map(|task| task.row().statement.body.born.clone())
            .collect::<Vec<_>>(),
        [stamp(1), stamp(2), stamp(3)]
    );

    let narrowed = WorkScope::Team {
        team,
        stem: Some(public("beta")),
    };
    assert_eq!(projection.contains_scope(&narrowed), Some(true));
    assert_eq!(projection.scope_counts(&narrowed).open(), 1);
    let absent_stem = WorkScope::Team {
        team,
        stem: Some(public("gone")),
    };
    assert_eq!(projection.contains_scope(&absent_stem), Some(false));
}

#[test]
fn team_header_groups_stems_by_first_task_without_changing_source_order() {
    let team = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let other_team = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let member = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    let other_member = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");

    let created = |value, project| {
        let mut task = task(value, project);
        task.creator_team = Some(team);
        task
    };
    let first_ready = created(1, "alpha.first");
    let second_ready = created(2, "beta.second");
    let mut blocked = created(3, "alpha.blocked");
    blocked.statement.body.requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
    );
    let mut claimed = task(4, "gamma.claimed");
    let mut member_claim = claim(&claimed, ClaimDisposition::Active);
    member_claim.body.lane = member;
    claimed.claim = Some(member_claim);
    let mut elsewhere = created(5, "delta.elsewhere");
    let mut other_claim = claim(&elsewhere, ClaimDisposition::Active);
    other_claim.body.lane = other_member;
    elsewhere.claim = Some(other_claim);
    let mut finished = created(6, "alpha.done");
    finished.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let mut unrelated = task(7, "omega.unrelated");
    unrelated.creator_team = Some(other_team);
    let hidden_ready = created(8, ".alpha.hidden");

    let mut board = board(vec![
        first_ready,
        second_ready,
        blocked,
        claimed,
        elsewhere,
        finished,
        unrelated,
        hidden_ready,
    ]);
    board.offer = vec![stamp(2), stamp(1), stamp(8)];
    let groups = |projection: &WorkProjection<'_>, team| {
        projection
            .stem_task_groups(&WorkScope::team_wide(team))
            .into_iter()
            .map(|group| {
                (
                    group.stem().to_string(),
                    group
                        .tasks()
                        .iter()
                        .map(|task| task.row().statement.body.project.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };

    let pending = WorkProjection::from_board_and_lanes(&board, None);
    assert!(
        groups(&pending, team).is_empty(),
        "a team's header waits for the roster",
    );

    let loaded = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team, other_team])),
        &BTreeMap::from([(member, team), (other_member, other_team)]),
    );
    let grouped = groups(&loaded, team);
    assert_eq!(
        grouped,
        [
            ("gamma.*".to_owned(), vec!["gamma.claimed".to_owned()]),
            ("beta.*".to_owned(), vec!["beta.second".to_owned()]),
            (
                "alpha.*".to_owned(),
                vec!["alpha.first".to_owned(), "alpha.blocked".to_owned()],
            ),
            (".alpha.*".to_owned(), vec![".alpha.hidden".to_owned()]),
        ],
        "groups follow their first task and gather later stem matches in source order",
    );
    assert_eq!(
        grouped.iter().map(|(_, tasks)| tasks.len()).sum::<usize>(),
        loaded.scope_counts(&WorkScope::team_wide(team)).open(),
        "the header holds exactly the open tasks of the team's Work scope",
    );
    assert_eq!(
        groups(&loaded, other_team),
        [
            ("delta.*".to_owned(), vec!["delta.elsewhere".to_owned()]),
            ("omega.*".to_owned(), vec!["omega.unrelated".to_owned()]),
        ],
        "a task created by one team and actively claimed by another shows only on the claimant's",
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
        &BTreeMap::from([(member, team), (other_member, other_team)]),
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

    let mut design = task(1, "rooms.design");
    design.creator_team = Some(creator);
    design.statement.body.intent = Some(wire::TaskIntent::Design);

    let mut direct_output = task(2, "plan.candidate");
    direct_output.creator_team = Some(creator);
    direct_output.statement.body.intent = Some(wire::TaskIntent::Plan);
    direct_output.statement.body.preparation = Some(preparation(design_born.clone()));
    let mut active_claim = claim(&direct_output, ClaimDisposition::Active);
    active_claim.body.lane = member;
    direct_output.claim = Some(active_claim);

    let mut plan = task(3, "plan.followup");
    plan.creator_team = Some(creator);
    plan.statement.body.intent = Some(wire::TaskIntent::Plan);
    plan.statement.body.preparation = Some(preparation(design_born.clone()));

    let mut execution = task(4, "app.execute");
    execution.creator_team = Some(executor);
    execution.statement.body.preparation = Some(preparation(plan_born));

    let mut rehomed = task(5, "app.rehomed");
    rehomed.creator_team = Some(dissolved);
    let mut past_claim = claim(&rehomed, ClaimDisposition::Freed);
    past_claim.body.lane = member;
    rehomed.claim = Some(past_claim);

    let board = board(vec![design, direct_output, plan, execution, rehomed]);
    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([creator, executor])),
        &BTreeMap::from([(member, executor)]),
    );
    let borns = |scope: &WorkScope| {
        projection
            .scope_tasks(scope)
            .into_iter()
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(
        borns(&WorkScope::planning()),
        BTreeSet::from([stamp(1), stamp(2), stamp(3)]),
        "a Design owns itself and every direct output, including one a team currently claims",
    );
    assert_eq!(
        borns(&WorkScope::AllOpen),
        BTreeSet::from([stamp(4), stamp(5)]),
        "a Plan-controlled task stays execution work and no Design-owned row is duplicated",
    );
    assert_eq!(
        borns(&WorkScope::team_wide(executor)),
        BTreeSet::from([stamp(4), stamp(5)]),
        "a current team strip excludes Design-owned planning even while its member claims it",
    );
    assert_eq!(
        projection.tasks()[1].accountable_owner(),
        Some(&WorkOwner::Design(design_born)),
    );
    assert_eq!(
        projection.tasks()[4].accountable_owner(),
        Some(&WorkOwner::Team(executor)),
        "a live last claimant can rehome work without changing its dissolved creator provenance",
    );
    assert_eq!(projection.tasks()[4].row().creator_team, Some(dissolved));
}

#[test]
fn a_group_scope_holds_exactly_the_open_tasks_associated_with_its_designs() {
    let design = stamp(1);
    let group = "dVxJ7j0vj"
        .parse::<wire::rooms::RoomAuthorityStamp>()
        .expect("fixture group");
    let prepared = |value, project, relation: fn(&mut wire::TaskPreparation, wire::TaskStamp)| {
        let mut task = task(value, project);
        let mut preparation = wire::TaskPreparation {
            parent: stamp(90),
            controller: stamp(91),
            construction: stamp(92),
        };
        relation(&mut preparation, design.clone());
        task.statement.body.preparation = Some(preparation);
        task
    };
    let mut settled = prepared(5, "rooms.settled", |preparation, design| {
        preparation.parent = design;
    });
    settled.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let board = {
        let mut board = board(vec![
            task(1, "rooms.design"),
            prepared(2, "alpha.child", |preparation, design| {
                preparation.parent = design;
            }),
            prepared(3, "rooms.controlled", |preparation, design| {
                preparation.controller = design;
            }),
            prepared(4, "alpha.constructed", |preparation, design| {
                preparation.construction = design;
            }),
            settled,
            task(6, "rooms.other"),
        ]);
        board.offer = vec![stamp(4), stamp(1), stamp(2), stamp(3), stamp(6)];
        board
    };
    let projection = WorkProjection::from_board(&board);
    let scope = |designs: &[wire::TaskStamp], stem: Option<WorkStemIdentity>| WorkScope::Group {
        group: group.clone(),
        designs: designs.to_vec(),
        stem,
    };
    let whole = scope(std::slice::from_ref(&design), None);
    let strip = projection
        .stem_task_groups(&whole)
        .into_iter()
        .map(|group| {
            (
                group.stem().to_string(),
                group
                    .tasks()
                    .iter()
                    .map(|task| task.row().statement.body.born.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        strip,
        [
            ("alpha.*".to_owned(), vec![stamp(4), stamp(2)]),
            ("rooms.*".to_owned(), vec![stamp(1), stamp(3)]),
        ],
        "the Design and every open task it parents, controls, or constructs cluster by \
         stem in offer order; settled and unassociated work stays out",
    );
    assert_eq!(
        projection.overview_for_scope(whole.clone()).tasks(),
        &[stamp(4), stamp(1), stamp(2), stamp(3)],
        "the total opens exactly the strip's tasks",
    );
    assert_eq!(projection.scope_counts(&whole).open(), 4);
    let alpha = WorkStemIdentity::new(WorkVisibility::Public, "alpha".to_owned());
    assert_eq!(
        projection
            .overview_for_scope(scope(std::slice::from_ref(&design), Some(alpha)))
            .tasks(),
        &[stamp(4), stamp(2)],
        "a stem tab opens only its stem within the group",
    );
    assert!(
        projection.scope_tasks(&scope(&[], None)).is_empty(),
        "a group without designs holds no work",
    );
}
