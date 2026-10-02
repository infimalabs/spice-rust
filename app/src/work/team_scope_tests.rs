use std::collections::{BTreeMap, BTreeSet};

use wire::CanonicalUuid;
use wire::head::ClaimDisposition;

use super::tests::{board, claim, stamp, task};
use super::{WorkProjection, WorkScope};

#[test]
fn team_scope_unions_creator_and_current_member_last_claim_without_duplicates() {
    let team = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        .parse::<CanonicalUuid>()
        .expect("fixture team");
    let member = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
        .parse::<CanonicalUuid>()
        .expect("fixture member");
    let other_team = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
        .parse::<CanonicalUuid>()
        .expect("fixture other team");

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
    let projection = WorkProjection::from_board_with_team_memberships(
        &board,
        BTreeSet::from([team]),
        BTreeMap::from([(member, team)]),
    );
    let scope = WorkScope::Team(team);
    assert!(projection.contains_scope(&scope));
    assert!(!projection.contains_scope(&WorkScope::Team(other_team)));
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
}
