use super::*;

fn uuid(value: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{value:012x}")
        .parse()
        .expect("fixture uuid is canonical")
}

fn lane(lane: u8, lineage: u8) -> wire::LaneResidency {
    wire::LaneResidency {
        lane: uuid(lane),
        lineage_id: uuid(lineage),
        name: None,
        worktree: format!("/fleet/main-{lane}"),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
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
        .map(|(member, lineage)| lane(*member, *lineage))
        .collect::<Vec<_>>();
    let mut lineages = rows.iter().map(|(_, lineage)| *lineage).collect::<Vec<_>>();
    lineages.sort_unstable();
    lineages.dedup();
    wire::ResidencyHead {
        epoch: uuid(99),
        state: wire::ResidentState::Seated,
        lanes,
        subscriptions: lineages
            .into_iter()
            .map(|lineage| wire::LineageSubscription {
                lineage_id: uuid(lineage),
                policy: LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: vec![format!("team.{lineage}")],
            })
            .collect(),
    }
}

fn grouping(head: &wire::ResidencyHead) -> Vec<(CanonicalUuid, CanonicalUuid)> {
    head.lanes
        .iter()
        .map(|row| (row.lane, row.lineage_id))
        .collect()
}

fn subscription_ids(head: &wire::ResidencyHead) -> Vec<CanonicalUuid> {
    head.subscriptions
        .iter()
        .map(|subscription| subscription.lineage_id)
        .collect()
}

#[test]
fn a_member_moves_between_teams_and_an_empty_source_subscription_disappears() {
    let mut state = head(&[(1, 1), (2, 1), (3, 3)]);

    let joined = move_member(&mut state, uuid(3), uuid(1)).expect("member joins team");

    assert_eq!(joined.from_lineage, uuid(3));
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

    assert_eq!(split.from_lineage, uuid(1));
    assert_eq!(split.source_rekeyed_to, Some(uuid(2)));
    assert_eq!(
        grouping(&state),
        vec![(uuid(1), uuid(1)), (uuid(2), uuid(2)), (uuid(3), uuid(2))]
    );
    assert_eq!(subscription_ids(&state), vec![uuid(1), uuid(2)]);
    assert_eq!(state.subscriptions[0].auto_projects, ["team.1"]);
    assert_eq!(state.subscriptions[1].auto_projects, ["team.1"]);
}

#[test]
fn removing_a_host_mints_an_identity_when_every_remaining_member_name_is_taken() {
    let mut state = head(&[(1, 1), (2, 2), (3, 2)]);
    move_member(&mut state, uuid(2), uuid(1)).expect("member joins the host team");

    let split = move_member(&mut state, uuid(1), uuid(1)).expect("host leaves the team");
    let rekeyed = split
        .source_rekeyed_to
        .expect("the remaining member receives a lineage identity");

    assert_eq!(
        rekeyed,
        uuid(4),
        "the first derived identity absent from every lane and lineage wins"
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
            .find(|subscription| subscription.lineage_id == rekeyed)
            .expect("the replacement lineage has a subscription")
            .auto_projects,
        ["team.1"]
    );
}
