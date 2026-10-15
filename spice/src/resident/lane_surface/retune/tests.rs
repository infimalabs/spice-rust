use std::collections::BTreeSet;

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

fn levels(global: bool, groups: &[u8]) -> FastLevels {
    FastLevels::new(global, groups.iter().map(|lane| uuid(*lane)).collect())
}

fn lanes(lanes: &[u8]) -> BTreeSet<CanonicalUuid> {
    lanes.iter().map(|lane| uuid(*lane)).collect()
}

fn fast(lane: u8, enabled: bool) -> (CanonicalUuid, CanonicalUuid, LiveRetune) {
    (uuid(lane), uuid(100 + lane), LiveRetune::Fast { enabled })
}

#[test]
fn a_global_fast_mode_change_owes_only_the_members_it_flips() {
    let fleet = fleet();

    assert_eq!(
        owed(fast_flips(&fleet, &fleet, &levels(true, &[]), false)),
        vec![fast(3, false)],
        "global off slows only the member no lane or team flag keeps fast"
    );
    assert_eq!(
        owed(fast_flips(&fleet, &fleet, &levels(false, &[]), true)),
        vec![fast(3, true)],
        "global on speeds only the member not already fast"
    );
    assert!(owed(fast_flips(&fleet, &fleet, &levels(true, &[]), true)).is_empty());
    assert!(
        owed(fast_flips(&fleet, &fleet, &levels(true, &[3]), false)).is_empty(),
        "global off leaves fast a member its working group runs fast"
    );
}

#[test]
fn group_fast_mode_runs_only_its_claimants_and_adds_to_every_other_level() {
    let fleet = fleet();
    let row = |lane| {
        fleet
            .lanes
            .iter()
            .find(|row| row.lane == uuid(lane))
            .expect("fleet member")
    };
    let effective = |levels: &FastLevels| [1, 2, 3, 4].map(|lane| levels.lane(&fleet, row(lane)));
    assert_eq!(effective(&levels(false, &[])), [true, true, false, true]);
    assert_eq!(effective(&levels(false, &[3])), [true, true, true, true]);
    assert_eq!(effective(&levels(true, &[])), [true; 4]);
    assert_eq!(
        effective(&levels(false, &[1, 4])),
        [true, true, false, true],
        "a group adds fast mode only to its claimants and switches no level off"
    );
}

#[test]
fn a_claim_beginning_and_ending_owes_its_running_lane_one_live_change() {
    let fleet = fleet();
    assert_eq!(
        owed(group_flips(&fleet, &lanes(&[]), &levels(false, &[3]))),
        vec![fast(3, true)]
    );
    assert_eq!(
        owed(group_flips(&fleet, &lanes(&[3]), &levels(false, &[]))),
        vec![fast(3, false)]
    );
    assert!(owed(group_flips(&fleet, &lanes(&[3]), &levels(false, &[3]))).is_empty());
}

#[test]
fn a_group_change_owes_nothing_while_another_level_holds_fast_mode() {
    let fleet = fleet();
    for (global, lane, held) in [(false, 1, "team"), (false, 4, "own"), (true, 3, "global")] {
        assert!(
            owed(group_flips(&fleet, &lanes(&[]), &levels(global, &[lane]))).is_empty()
                && owed(group_flips(&fleet, &lanes(&[lane]), &levels(global, &[]))).is_empty(),
            "the {held} flag holds lane {lane} fast"
        );
    }
}

#[test]
fn team_turbo_off_owes_nothing_to_a_lane_its_group_runs_fast() {
    let before = fleet();
    let mut after = before.clone();
    after.subscriptions[0].fast_mode = false;
    assert_eq!(
        owed(fast_flips(&before, &after, &levels(false, &[1]), false)),
        vec![fast(2, false)]
    );
}
