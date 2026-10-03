use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;

use super::tests::{board, claim, stamp, task};
use super::{WorkProjection, WorkRail, WorkScope, WorkStemIdentity, WorkVisibility};

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
fn each_open_task_rides_its_live_creator_team_rail_or_else_the_global_strip() {
    let live = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let dissolved = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");

    let mut live_alpha = task(1, "alpha.runtime");
    live_alpha.creator_team = Some(live);
    let mut live_beta = task(2, "beta.core");
    live_beta.creator_team = Some(live);
    let mut orphaned = task(3, "alpha.audit");
    orphaned.creator_team = Some(dissolved);
    let operator = task(4, "alpha.notes");
    let mut finished = task(5, "gamma.done");
    finished.creator_team = Some(live);
    finished.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let board = board(vec![live_alpha, live_beta, orphaned, operator, finished]);
    let rail = |projection: &WorkProjection<'_>, rail| {
        projection
            .rail_stems(rail)
            .into_iter()
            .map(|(stem, counts)| (stem.stem().to_owned(), counts.open()))
            .collect::<Vec<_>>()
    };

    let pending = WorkProjection::from_board_and_lanes(&board, None);
    assert_eq!(
        rail(&pending, WorkRail::Global),
        [("alpha".to_owned(), 1)],
        "team-created work waits for the roster instead of flashing onto the global strip",
    );
    assert!(rail(&pending, WorkRail::Team(live)).is_empty());

    let loaded = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([live])),
        &BTreeMap::new(),
    );
    assert_eq!(
        rail(&loaded, WorkRail::Team(live)),
        [("alpha".to_owned(), 1), ("beta".to_owned(), 1)],
    );
    assert_eq!(loaded.rail_counts(WorkRail::Team(live)).open(), 2);
    assert_eq!(
        rail(&loaded, WorkRail::Global),
        [("alpha".to_owned(), 2)],
        "a dissolved creator team returns its work to the global strip",
    );
    assert_eq!(
        loaded.rail_counts(WorkRail::Team(live)).open()
            + loaded.rail_counts(WorkRail::Global).open(),
        loaded.counts().open(),
        "every open task rides exactly one strip",
    );
}
