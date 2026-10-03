use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;

use super::tests::{board, claim, stamp, task};
use super::{WorkProjection, WorkScope, WorkStemIdentity, WorkVisibility};

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture uuid")
}

fn public(stem: &str) -> WorkStemIdentity {
    WorkStemIdentity::new(WorkVisibility::Public, stem.to_owned())
}

#[test]
fn team_scope_waits_for_the_roster_then_unions_creator_and_member_last_claim_once() {
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
fn team_header_tasks_are_its_open_scope_in_flight_first_then_offer_order() {
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

    let mut board = board(vec![
        first_ready,
        second_ready,
        blocked,
        claimed,
        elsewhere,
        finished,
        unrelated,
    ]);
    board.offer = vec![stamp(2), stamp(1)];
    let projects = |projection: &WorkProjection<'_>, team| {
        projection
            .team_tasks(team)
            .into_iter()
            .map(|task| task.row().statement.body.project.clone())
            .collect::<Vec<_>>()
    };

    let pending = WorkProjection::from_board_and_lanes(&board, None);
    assert!(
        projects(&pending, team).is_empty(),
        "a team's header waits for the roster",
    );

    let loaded = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team, other_team])),
        &BTreeMap::from([(member, team), (other_member, other_team)]),
    );
    let order = projects(&loaded, team);
    assert_eq!(
        order,
        [
            "gamma.claimed",
            "delta.elsewhere",
            "beta.second",
            "alpha.first",
            "alpha.blocked",
        ],
        "in-flight work leads, ready work follows the offer, and closed work is absent",
    );
    assert_eq!(
        order.len(),
        loaded.scope_counts(&WorkScope::team_wide(team)).open(),
        "the header holds exactly the open tasks its total opens",
    );
    assert_eq!(
        projects(&loaded, other_team),
        ["delta.elsewhere", "omega.unrelated"],
        "a task created by one team and claimed by another shows on both",
    );
}
