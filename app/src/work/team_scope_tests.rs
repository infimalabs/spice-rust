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
fn each_open_task_rides_its_claimant_or_creator_team_rail_or_else_the_global_strip() {
    let creator_team = uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    let claimant_team = uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    let claimant = uuid("cccccccc-cccc-4ccc-8ccc-cccccccccccc");
    let unteamed_claimant = uuid("dddddddd-dddd-4ddd-8ddd-dddddddddddd");
    let dissolved = uuid("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee");

    let mut created_alpha = task(1, "alpha.runtime");
    created_alpha.creator_team = Some(creator_team);
    let mut created_beta = task(2, "beta.core");
    created_beta.creator_team = Some(creator_team);
    let mut claimed = task(3, "claim.runtime");
    let mut active_claim = claim(&claimed, ClaimDisposition::Active);
    active_claim.body.lane = claimant;
    claimed.claim = Some(active_claim);
    let mut moved = task(4, "move.runtime");
    moved.creator_team = Some(creator_team);
    let mut moved_claim = claim(&moved, ClaimDisposition::Active);
    moved_claim.body.lane = claimant;
    moved.claim = Some(moved_claim);
    let mut orphaned = task(5, "orphan.audit");
    orphaned.creator_team = Some(dissolved);
    let operator = task(6, "operator.notes");
    let mut solo = task(7, "solo.runtime");
    let mut solo_claim = claim(&solo, ClaimDisposition::Active);
    solo_claim.body.lane = unteamed_claimant;
    solo.claim = Some(solo_claim);
    let mut finished = task(8, "gamma.done");
    finished.creator_team = Some(creator_team);
    finished.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let board = board(vec![
        created_alpha,
        created_beta,
        claimed,
        moved,
        orphaned,
        operator,
        solo,
        finished,
    ]);
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
        [("operator".to_owned(), 1)],
        "team-associated work waits for the roster instead of flashing globally",
    );
    assert!(rail(&pending, WorkRail::Team(creator_team)).is_empty());
    assert!(rail(&pending, WorkRail::Team(claimant_team)).is_empty());

    let loaded = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([creator_team, claimant_team])),
        &BTreeMap::from([(claimant, claimant_team)]),
    );
    assert_eq!(
        rail(&loaded, WorkRail::Team(creator_team)),
        [("alpha".to_owned(), 1), ("beta".to_owned(), 1)],
    );
    assert_eq!(
        rail(&loaded, WorkRail::Team(claimant_team)),
        [("claim".to_owned(), 1), ("move".to_owned(), 1)],
        "a current claimant team takes precedence over the creator team",
    );
    assert_eq!(
        rail(&loaded, WorkRail::Global),
        [
            ("operator".to_owned(), 1),
            ("orphan".to_owned(), 1),
            ("solo".to_owned(), 1),
        ],
        "dissolved and unteamed ownership belongs to the global strip",
    );
    assert_eq!(
        loaded.rail_counts(WorkRail::Team(creator_team)).open()
            + loaded.rail_counts(WorkRail::Team(claimant_team)).open()
            + loaded.rail_counts(WorkRail::Global).open(),
        loaded.counts().open(),
        "every open task rides exactly one strip",
    );
}
