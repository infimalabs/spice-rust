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
/// titled `title` for it.
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

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free)
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

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free)
        .expect("one generation")
        .expect("a free lane");

    assert_eq!(
        (chosen.lane, chosen.seated, chosen.score),
        (seated, true, 0.0)
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

    let chosen = choose(&mut profiles, &frequency, Some(&design), &free)
        .expect("one generation")
        .expect("a free lane");

    assert_eq!((chosen.lane, chosen.seated), (matching, false));
    assert!(chosen.score > 0.0, "its best retained session scores");
    assert_eq!(
        choose(
            &mut profiles,
            &frequency,
            Some(&design),
            &FreeLanes::default()
        ),
        Ok(None),
        "no free lane leaves the Design for allocation"
    );
}
