//! Working-group fast mode joins the levels a residency head holds
//! additively, and a running OMP member owes a live change only when its
//! effective fast mode flips.

use std::collections::BTreeSet;

use super::*;

const TEAM: u8 = 1;
const CLAIMANT: u8 = 2;
const PEER: u8 = 3;

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn seat(lane: u8) -> wire::LaneResidency {
    let occupant = uuid(lane + 50);
    wire::LaneResidency {
        lane: uuid(lane),
        team: uuid(TEAM),
        name: None,
        worktree: format!("/work/{lane}"),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: Vec::new(),
        session: None,
        disposition: "occupied".to_owned(),
    }
}

fn head(team_fast: bool) -> wire::ResidencyHead {
    wire::ResidencyHead {
        epoch: uuid(90),
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![seat(CLAIMANT), seat(PEER)],
        subscriptions: vec![wire::TeamSubscription {
            team: uuid(TEAM),
            policy: wire::intent::TeamLife::Drive,
            fast_mode: team_fast,
        }],
        renewals: Vec::new(),
    }
}

fn row(head: &wire::ResidencyHead, lane: u8) -> &wire::LaneResidency {
    head.lanes
        .iter()
        .find(|row| row.lane == uuid(lane))
        .expect("fixture seat")
}

fn claimants(lanes: &[u8]) -> BTreeSet<CanonicalUuid> {
    lanes.iter().map(|lane| uuid(*lane)).collect()
}

/// Each owed change as its lane and the fast mode it asks for.
fn owed(flips: &[FastFlip]) -> Vec<(CanonicalUuid, bool)> {
    flips
        .iter()
        .map(|flip| match flip.live {
            LiveRetune::Fast { enabled } => (flip.lane, enabled),
            _ => panic!("fast-mode flip asks for another retune"),
        })
        .collect()
}

#[test]
fn group_fast_mode_runs_only_its_claimants_and_adds_to_every_other_level() {
    let slow = head(false);
    let grouped = FastLevels::new(false, claimants(&[CLAIMANT]));
    assert!(grouped.lane(&slow, row(&slow, CLAIMANT)));
    assert!(!grouped.lane(&slow, row(&slow, PEER)));

    let ungrouped = FastLevels::new(false, BTreeSet::new());
    assert!(!ungrouped.lane(&slow, row(&slow, CLAIMANT)));
    assert!(FastLevels::new(true, BTreeSet::new()).lane(&slow, row(&slow, PEER)));
    let team = head(true);
    assert!(ungrouped.lane(&team, row(&team, PEER)));
    let mut own = head(false);
    own.lanes[1].fast_mode = true;
    assert!(ungrouped.lane(&own, row(&own, PEER)));

    // Every other level off leaves the group's claimant fast, and the
    // group's flag off leaves fast whatever another level holds.
    assert!(grouped.lane(&team, row(&team, CLAIMANT)));
    assert!(grouped.lane(&team, row(&team, PEER)));
}

#[test]
fn a_claim_beginning_and_ending_owes_its_running_lane_one_live_change() {
    let slow = head(false);
    let begun = FastLevels::new(false, claimants(&[CLAIMANT]));
    assert_eq!(
        owed(&group_flips(&slow, &BTreeSet::new(), &begun)),
        [(uuid(CLAIMANT), true)]
    );
    let ended = FastLevels::new(false, BTreeSet::new());
    assert_eq!(
        owed(&group_flips(&slow, &claimants(&[CLAIMANT]), &ended)),
        [(uuid(CLAIMANT), false)]
    );
    assert!(group_flips(&slow, &claimants(&[CLAIMANT]), &begun).is_empty());
}

#[test]
fn a_group_change_owes_nothing_while_another_level_holds_fast_mode() {
    let unchanged = |head: &wire::ResidencyHead, global: bool| {
        let begun = FastLevels::new(global, claimants(&[CLAIMANT]));
        let ended = FastLevels::new(global, BTreeSet::new());
        group_flips(head, &BTreeSet::new(), &begun).is_empty()
            && group_flips(head, &claimants(&[CLAIMANT]), &ended).is_empty()
    };
    assert!(unchanged(&head(true), false), "team TURBO holds it");
    assert!(unchanged(&head(false), true), "global TURBO holds it");
    let mut own = head(false);
    own.lanes[0].fast_mode = true;
    assert!(unchanged(&own, false), "the lane's own flag holds it");
}

#[test]
fn team_turbo_off_owes_nothing_to_a_lane_its_group_runs_fast() {
    let (before, after) = (head(true), head(false));
    let grouped = FastLevels::new(false, claimants(&[CLAIMANT]));
    assert_eq!(
        owed(&fast_flips(&before, &after, &grouped)),
        [(uuid(PEER), false)]
    );
}
