use board::similarity::TermCounts;
use wire::body::LaneSessionDeclared;
use wire::{Seq, TaskStamp};

use super::*;
use crate::rooms::evidence::Evidence;
use crate::rooms::profile::EvidenceKey;

fn uuid(last: &str) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last}")
        .parse()
        .expect("fixture uuid")
}

/// Declare `identity` for `occupant` in `lane` and retain one completed task
/// titled `title` for it, keyed by `key`; key [`NUDGED`] is the nudged Design.
fn retain(
    profiles: &mut SessionProfiles,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    identity: &str,
    key: u64,
    title: &str,
) {
    profiles.declare(&LaneSessionDeclared {
        lane,
        occupant,
        worktree: "/worktree".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("model", "effort"),
        epoch: uuid("03"),
        stream: "session".to_owned(),
        source: uuid("04"),
        identity: identity.to_owned(),
        member: 1,
        offset: 2,
        length: 3,
    });
    profiles.insert(
        lane,
        occupant,
        Evidence::task(
            EvidenceKey::Task {
                task: TaskStamp::for_micros(i64::try_from(key).expect("key")).expect("stamp"),
                statement: Seq::new(key * 2 + 1).expect("seq"),
                completion: Seq::new(key * 2 + 2).expect("seq"),
                support: wire::rooms::DesignSupport::Implemented,
            },
            title,
            None,
            &[],
            "checked",
        ),
    );
}

const DESIGN: &str = "room catch-up delivers unheard operator posts";

/// Key of the nudged Design's own task cards.
const NUDGED: u64 = 9;

fn nudged() -> TaskStamp {
    TaskStamp::for_micros(i64::try_from(NUDGED).expect("key")).expect("stamp")
}

fn frequency() -> FrequencyProfile {
    FrequencyProfile::of(
        [
            DESIGN,
            "room catch-up budget",
            "app theme colors",
            "gate proof cache",
        ]
        .map(|text| TermCounts::from_texts([text])),
    )
}

fn design(frequency: &FrequencyProfile) -> Prepared {
    frequency
        .prepare(&TermCounts::from_texts([DESIGN]))
        .expect("the design weighs terms")
}

#[test]
fn the_best_matching_free_seated_lane_receives_the_nudge_and_ties_go_to_the_lowest_lane() {
    let (low, high, tied) = (uuid("01"), uuid("02"), uuid("05"));
    let mut profiles = SessionProfiles::default();
    retain(&mut profiles, low, uuid("11"), "a", 1, "app theme colors");
    retain(
        &mut profiles,
        high,
        uuid("12"),
        "b",
        2,
        "room catch-up delivers posts",
    );
    retain(
        &mut profiles,
        tied,
        uuid("15"),
        "e",
        5,
        "room catch-up delivers posts",
    );
    let frequency = frequency();
    let design = design(&frequency);
    let free = FreeLanes {
        seated: [(low, uuid("11")), (high, uuid("12")), (tied, uuid("15"))].into(),
        vacant: BTreeSet::new(),
    };

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free, &nudged())
        .expect("one generation")
        .expect("a free lane");

    assert_eq!(chosen.lane, high, "equal best scores go to the lower lane");
    assert!(chosen.seated);
    assert!(chosen.score > 0.0);
}

#[test]
fn a_free_seated_lane_outranks_a_better_matching_vacant_lane() {
    let (seated, vacant) = (uuid("02"), uuid("01"));
    let mut profiles = SessionProfiles::default();
    retain(
        &mut profiles,
        seated,
        uuid("12"),
        "b",
        2,
        "gate proof cache",
    );
    retain(&mut profiles, vacant, uuid("11"), "a", 1, DESIGN);
    let frequency = frequency();
    let design = design(&frequency);
    let free = FreeLanes {
        seated: [(seated, uuid("12"))].into(),
        vacant: [vacant].into(),
    };

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free, &nudged())
        .expect("one generation")
        .expect("a free lane");

    assert_eq!(
        (chosen.lane, chosen.seated, chosen.score, chosen.last_resort),
        (seated, true, 0.0, false)
    );
}

#[test]
fn with_no_free_seated_lane_the_vacant_lane_whose_retained_session_matches_best_is_woken() {
    let (stale, matching) = (uuid("01"), uuid("02"));
    let mut profiles = SessionProfiles::default();
    retain(&mut profiles, stale, uuid("11"), "a", 1, "app theme colors");
    retain(
        &mut profiles,
        matching,
        uuid("12"),
        "b",
        2,
        "gate proof cache",
    );
    retain(&mut profiles, matching, uuid("13"), "c", 3, DESIGN);
    let frequency = frequency();
    let design = design(&frequency);
    let free = FreeLanes {
        seated: BTreeMap::new(),
        vacant: [stale, matching].into(),
    };

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free, &nudged())
        .expect("one generation")
        .expect("a free lane");

    assert_eq!((chosen.lane, chosen.seated), (matching, false));
    assert!(chosen.score > 0.0, "its best retained session scores");
    assert_eq!(
        choose(
            &mut profiles,
            &frequency,
            Some(&design),
            &FreeLanes::default(),
            &nudged()
        ),
        Ok(None),
        "no free lane leaves the Design for allocation"
    );
}

#[test]
fn the_designs_own_cards_do_not_score_and_a_vacant_stranger_outranks_a_seated_lane_that_worked_it()
{
    let (worked, stranger) = (uuid("01"), uuid("02"));
    let mut profiles = SessionProfiles::default();
    retain(&mut profiles, worked, uuid("11"), "a", NUDGED, DESIGN);
    retain(
        &mut profiles,
        stranger,
        uuid("12"),
        "b",
        2,
        "gate proof cache",
    );
    let frequency = frequency();
    let design = design(&frequency);

    let alone = choose(
        &mut profiles,
        &frequency,
        Some(&design),
        &FreeLanes {
            seated: [(worked, uuid("11"))].into(),
            vacant: BTreeSet::new(),
        },
        &nudged(),
    )
    .expect("one generation")
    .expect("a free lane");
    assert_eq!(
        (alone.lane, alone.score, alone.last_resort),
        (worked, 0.0, true),
        "the Design's own card neither scores nor hides that the lane worked it"
    );

    let chosen = choose(
        &mut profiles,
        &frequency,
        Some(&design),
        &FreeLanes {
            seated: [(worked, uuid("11"))].into(),
            vacant: [stranger].into(),
        },
        &nudged(),
    )
    .expect("one generation")
    .expect("a free lane");
    assert_eq!(
        (chosen.lane, chosen.seated, chosen.score, chosen.last_resort),
        (stranger, false, 0.0, false),
        "a vacant lane that never worked the Design outranks a seated one that did"
    );
}

#[test]
fn when_every_free_lane_worked_the_design_the_last_resort_is_seated_then_best_scoring_then_lowest()
{
    let (quiet, matching, tied, vacant) = (uuid("01"), uuid("02"), uuid("05"), uuid("03"));
    let mut profiles = SessionProfiles::default();
    for (lane, occupant, identity) in [
        (quiet, uuid("11"), "a"),
        (matching, uuid("12"), "b"),
        (tied, uuid("15"), "e"),
        (vacant, uuid("13"), "c"),
    ] {
        retain(&mut profiles, lane, occupant, identity, NUDGED, DESIGN);
    }
    retain(&mut profiles, quiet, uuid("11"), "a", 1, "app theme colors");
    retain(
        &mut profiles,
        matching,
        uuid("12"),
        "b",
        2,
        "room catch-up delivers posts",
    );
    retain(
        &mut profiles,
        tied,
        uuid("15"),
        "e",
        5,
        "room catch-up delivers posts",
    );
    retain(&mut profiles, vacant, uuid("13"), "c", 3, DESIGN);
    let frequency = frequency();
    let design = design(&frequency);
    let free = FreeLanes {
        seated: [
            (quiet, uuid("11")),
            (matching, uuid("12")),
            (tied, uuid("15")),
        ]
        .into(),
        vacant: [vacant].into(),
    };

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free, &nudged())
        .expect("one generation")
        .expect("a free lane");

    assert_eq!(
        (chosen.lane, chosen.seated, chosen.last_resort),
        (matching, true, true),
        "seated beats the better-scoring vacant lane, score beats quiet, and the tie keeps the lower lane"
    );
    assert!(chosen.score > 0.0);
}
