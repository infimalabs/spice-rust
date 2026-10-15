use super::*;

fn uuid(value: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{value:012x}")
        .parse()
        .expect("fixture uuid is canonical")
}

/// A running OMP member of `team`, its own flag set by `fast`.
fn member(lane: u8, team: u8, fast: bool) -> wire::LaneResidency {
    wire::LaneResidency {
        lane: uuid(lane),
        team: uuid(team),
        name: None,
        worktree: format!("/fleet/main-{lane}"),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: fast,
        occupant: Some(uuid(100 + lane)),
        lineage: Vec::new(),
        capabilities: Vec::new(),
        session: None,
        disposition: "running".to_owned(),
    }
}

/// Team 1 (lanes 1, 2) is TURBO; team 3 (lanes 3, 4) is not, though lane 4
/// sets its own flag.
fn fleet() -> wire::ResidencyHead {
    wire::ResidencyHead {
        epoch: uuid(99),
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![
            member(1, 1, false),
            member(2, 1, false),
            member(3, 3, false),
            member(4, 3, true),
        ],
        subscriptions: [(1, true), (3, false)]
            .into_iter()
            .map(|(team, fast_mode)| wire::TeamSubscription {
                team: uuid(team),
                policy: wire::intent::TeamLife::Drive,
                fast_mode,
            })
            .collect(),
        renewals: Vec::new(),
    }
}

fn owed(flips: Vec<FastFlip>) -> Vec<(CanonicalUuid, CanonicalUuid, LiveRetune)> {
    flips
        .into_iter()
        .map(|flip| (flip.lane, flip.occupant, flip.live))
        .collect()
}

#[test]
fn a_global_fast_mode_change_owes_only_the_members_it_flips() {
    let fleet = fleet();

    assert_eq!(
        owed(fast_flips(&fleet, true, &fleet, false)),
        vec![(uuid(3), uuid(103), LiveRetune::Fast { enabled: false })],
        "global off slows only the member no lane or team flag keeps fast"
    );
    assert_eq!(
        owed(fast_flips(&fleet, false, &fleet, true)),
        vec![(uuid(3), uuid(103), LiveRetune::Fast { enabled: true })],
        "global on speeds only the member not already fast"
    );
    assert!(owed(fast_flips(&fleet, true, &fleet, true)).is_empty());
}
