use super::*;

fn uuid(value: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{value:012x}")
        .parse()
        .expect("fixture uuid is canonical")
}

fn lane(lane: u8, team: u8) -> wire::LaneResidency {
    wire::LaneResidency {
        lane: uuid(lane),
        team: uuid(team),
        name: None,
        worktree: format!("/fleet/main-{lane}"),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        occupant: None,
        lineage: Vec::new(),
        capabilities: Vec::new(),
        session: None,
        disposition: "vacant".to_owned(),
    }
}

fn head(rows: &[(u8, u8)]) -> wire::ResidencyHead {
    let lanes = rows
        .iter()
        .map(|(member, team)| lane(*member, *team))
        .collect::<Vec<_>>();
    let mut teams = rows.iter().map(|(_, team)| *team).collect::<Vec<_>>();
    teams.sort_unstable();
    teams.dedup();
    wire::ResidencyHead {
        epoch: uuid(99),
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes,
        subscriptions: teams
            .into_iter()
            .map(|team| wire::TeamSubscription {
                team: uuid(team),
                policy: TeamLife::Drive,
            })
            .collect(),
        renewals: Vec::new(),
    }
}

fn grouping(head: &wire::ResidencyHead) -> Vec<(CanonicalUuid, CanonicalUuid)> {
    head.lanes.iter().map(|row| (row.lane, row.team)).collect()
}

fn subscription_ids(head: &wire::ResidencyHead) -> Vec<CanonicalUuid> {
    head.subscriptions
        .iter()
        .map(|subscription| subscription.team)
        .collect()
}

#[test]
fn a_member_moves_between_orgs_and_an_empty_source_subscription_disappears() {
    let mut state = head(&[(1, 1), (2, 1), (3, 3)]);

    let joined = move_member(&mut state, uuid(3), uuid(1)).expect("member joins team");

    assert_eq!(joined.from_team, uuid(3));
    assert_eq!(joined.source_rekeyed_to, None);
    assert_eq!(
        grouping(&state),
        vec![(uuid(1), uuid(1)), (uuid(2), uuid(1)), (uuid(3), uuid(1))]
    );
    assert_eq!(subscription_ids(&state), vec![uuid(1)]);

    move_member(&mut state, uuid(2), uuid(2)).expect("member leaves team");
    assert_eq!(
        grouping(&state),
        vec![(uuid(1), uuid(1)), (uuid(2), uuid(2)), (uuid(3), uuid(1))]
    );
    assert_eq!(subscription_ids(&state), vec![uuid(1), uuid(2)]);
}

#[test]
fn removing_the_host_rekeys_every_remaining_member_and_preserves_both_policies() {
    let mut state = head(&[(1, 1), (2, 1), (3, 1)]);

    let split = move_member(&mut state, uuid(1), uuid(1)).expect("host leaves team");

    assert_eq!(split.from_team, uuid(1));
    assert_eq!(split.source_rekeyed_to, Some(uuid(2)));
    assert_eq!(
        grouping(&state),
        vec![(uuid(1), uuid(1)), (uuid(2), uuid(2)), (uuid(3), uuid(2))]
    );
    assert_eq!(subscription_ids(&state), vec![uuid(1), uuid(2)]);
    assert!(
        state
            .subscriptions
            .iter()
            .all(|subscription| subscription.policy == TeamLife::Drive)
    );
}

#[test]
fn removing_a_host_mints_an_identity_when_every_remaining_member_name_is_taken() {
    let mut state = head(&[(1, 1), (2, 2), (3, 2)]);
    move_member(&mut state, uuid(2), uuid(1)).expect("member joins the host team");

    let split = move_member(&mut state, uuid(1), uuid(1)).expect("host leaves the team");
    let rekeyed = split
        .source_rekeyed_to
        .expect("the remaining member receives a team identity");

    assert_eq!(
        rekeyed,
        uuid(4),
        "the first derived identity absent from every lane and team wins"
    );
    assert_eq!(
        grouping(&state),
        vec![(uuid(1), uuid(1)), (uuid(2), rekeyed), (uuid(3), uuid(2))]
    );
    assert_eq!(state.subscriptions.len(), 3);
    assert!(subscription_ids(&state).contains(&rekeyed));
    assert_eq!(
        state
            .subscriptions
            .iter()
            .find(|subscription| subscription.team == rekeyed)
            .expect("the replacement team has a subscription")
            .policy,
        TeamLife::Drive
    );
}
