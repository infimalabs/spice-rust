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
        fast_mode: false,
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
                formed_at: wire::head::teamed(
                    uuid(team),
                    rows.iter()
                        .filter(|(_, candidate)| *candidate == team)
                        .map(|(member, _)| uuid(*member)),
                )
                .then(|| {
                    "2030-04-12T18:44:09.271Z"
                        .parse()
                        .expect("fixture formation instant")
                }),
                policy: TeamLife::Drive,
                fast_mode: false,
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
fn membership_admission_stops_at_six_without_changing_a_refused_head() {
    let mut state = head(&[(1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 6), (7, 7)]);
    move_member(&mut state, uuid(6), uuid(1)).expect("sixth member joins");
    assert_eq!(
        state.team_members(uuid(1)).collect::<Vec<_>>(),
        (1..=6).map(uuid).collect::<Vec<_>>(),
    );
    let full = state.clone();
    let Err(Outcome::Refusal(refusal)) = move_member(&mut state, uuid(7), uuid(1)) else {
        panic!("seventh member must be refused");
    };
    assert_eq!(refusal.reason(), "team::member-limit");
    assert_eq!(state, full);

    let reorder = wire::intent::LaneArrange {
        topology_revision: state.topology_revision,
        lane: uuid(7),
        before: uuid(1),
    };
    let Err(Outcome::Refusal(refusal)) = rearrange(&state, &reorder) else {
        panic!("moving before a member cannot bypass capacity");
    };
    assert_eq!(refusal.reason(), "team::member-limit");

    move_member(&mut state, uuid(6), uuid(6)).expect("member leaves a full team");
    move_member(&mut state, uuid(7), uuid(1)).expect("vacated capacity admits another member");
    assert_eq!(
        state.team_members(uuid(1)).collect::<Vec<_>>(),
        vec![uuid(1), uuid(2), uuid(3), uuid(4), uuid(5), uuid(7)],
    );
}

#[test]
fn complete_arrangements_cannot_grow_a_team_past_six() {
    let state = head(&[(1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 6), (7, 7)]);
    let request = |count: u8| TeamArrange {
        topology_revision: state.topology_revision,
        teams: std::iter::once(wire::intent::TeamArrangement {
            team: uuid(1),
            lanes: (1..=count).map(uuid).collect(),
        })
        .chain(
            ((count + 1)..=7).map(|member| wire::intent::TeamArrangement {
                team: uuid(member),
                lanes: vec![uuid(member)],
            }),
        )
        .collect(),
    };
    admit_arrangement(&state, &request(6)).expect("six-member arrangement admitted");
    let Err(Outcome::Refusal(refusal)) = admit_arrangement(&state, &request(7)) else {
        panic!("seven-member arrangement must be refused");
    };
    assert_eq!(refusal.reason(), "team::member-limit");
}

#[test]
fn legacy_oversized_teams_can_be_restated_reordered_and_reduced_but_not_grown() {
    let mut state = head(&[
        (1, 1),
        (2, 1),
        (3, 1),
        (4, 1),
        (5, 1),
        (6, 1),
        (7, 1),
        (8, 8),
    ]);
    let restated = TeamArrange {
        topology_revision: state.topology_revision,
        teams: vec![
            wire::intent::TeamArrangement {
                team: uuid(1),
                lanes: (1..=7).rev().map(uuid).collect(),
            },
            wire::intent::TeamArrangement {
                team: uuid(8),
                lanes: vec![uuid(8)],
            },
        ],
    };
    admit_arrangement(&state, &restated).expect("legacy team can be restated");
    let reorder = wire::intent::LaneArrange {
        topology_revision: state.topology_revision,
        lane: uuid(7),
        before: uuid(1),
    };
    let reordered = rearrange(&state, &reorder).expect("legacy team can reorder");
    assert_eq!(reordered.next.lanes[0].lane, uuid(7));
    let full = state.clone();
    let Err(Outcome::Refusal(refusal)) = move_member(&mut state, uuid(8), uuid(1)) else {
        panic!("legacy oversized teams cannot grow");
    };
    assert_eq!(refusal.reason(), "team::member-limit");
    assert_eq!(state, full);
    move_member(&mut state, uuid(7), uuid(7)).expect("legacy team can shed a member");
    assert_eq!(
        state.team_members(uuid(1)).collect::<Vec<_>>(),
        (1..=6).map(uuid).collect::<Vec<_>>(),
    );
}

#[test]
fn a_member_joining_an_existing_team_lands_after_its_current_members() {
    let mut state = head(&[(1, 1), (2, 1), (3, 1), (4, 4)]);
    move_member(&mut state, uuid(1), uuid(1)).expect("host leaves team");
    assert_eq!(
        grouping(&state),
        vec![
            (uuid(1), uuid(1)),
            (uuid(2), uuid(2)),
            (uuid(3), uuid(2)),
            (uuid(4), uuid(4)),
        ]
    );

    move_member(&mut state, uuid(1), uuid(2)).expect("former host rejoins");

    assert_eq!(
        grouping(&state),
        vec![
            (uuid(2), uuid(2)),
            (uuid(3), uuid(2)),
            (uuid(1), uuid(2)),
            (uuid(4), uuid(4)),
        ],
        "the rejoining member is last, and team order is unchanged"
    );
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

#[test]
fn formation_instants_follow_the_teamed_predicate_through_singleton_teams() {
    let instant = |text: &str| -> wire::Instant { text.parse().expect("fixture instant") };
    let (first, later, reformed) = (
        instant("2031-01-01T00:00:01.000Z"),
        instant("2031-01-01T00:00:02.000Z"),
        instant("2031-01-01T00:00:03.000Z"),
    );
    let formed_at = |head: &wire::ResidencyHead, team: u8| {
        head.subscriptions
            .iter()
            .find(|subscription| subscription.team == uuid(team))
            .expect("team subscription")
            .formed_at
    };
    let step = |observed: &wire::ResidencyHead, rows: &[(u8, u8)], at| {
        let mut next = head(rows);
        for subscription in &mut next.subscriptions {
            subscription.formed_at = None;
        }
        reconcile_formations(observed, &mut next, at);
        next
    };

    let lone = head(&[(1, 1), (2, 2)]);
    assert_eq!(
        formed_at(&lone, 1),
        None,
        "a self-identified seat is unteamed"
    );
    let split = step(&lone, &[(1, 7), (2, 2)], first);
    assert_eq!(
        formed_at(&split, 7),
        Some(first),
        "one member under another identity forms a team at that commit"
    );
    let grown = step(&split, &[(2, 7), (1, 7)], later);
    assert_eq!(
        formed_at(&grown, 7),
        Some(first),
        "growth and order keep it"
    );
    let shrunk = step(&grown, &[(2, 7), (1, 1)], later);
    assert_eq!(
        formed_at(&shrunk, 7),
        Some(first),
        "one remaining member under another identity keeps the team formed"
    );

    let pair = head(&[(1, 1), (2, 1)]);
    let unteamed = step(&pair, &[(1, 1), (2, 2)], later);
    assert_eq!(
        formed_at(&unteamed, 1),
        None,
        "the self-identified seat clears it"
    );
    let reteamed = step(&unteamed, &[(1, 1), (3, 1)], reformed);
    assert_eq!(
        formed_at(&reteamed, 1),
        Some(reformed),
        "reforming mints a new instant"
    );
}
