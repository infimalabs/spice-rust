//! A partition keeps one group per member, ends a group with its last member,
//! grows no group past the limit, and admits only faithful arrangements.

use super::{
    Destination, Group, GroupingError, Kind, MEMBER_LIMIT, Moved, Partition, Placement,
    RevisionError, advance_revision,
};

/// The one member no fixture kind admits.
const OUTSIDER: u8 = u8::MAX;

/// A kind whose ungrouped member stands alone in a group named for itself,
/// as a team's lone lane does.
struct Hosted;

impl Kind for Hosted {
    type Group = u8;
    type Member = u8;

    /// Every member but the outsider is admitted.
    fn admits(&self, member: u8) -> bool {
        member != OUTSIDER
    }

    fn ungrouped(&self, member: u8) -> Option<u8> {
        Some(member)
    }

    fn rekey(&self, partition: &Partition<u8, u8>, group: u8, _: u8) -> u8 {
        (group..)
            .find(|candidate| !partition.holds(*candidate) && *candidate != group)
            .expect("a free identity")
    }
}

/// A kind whose ungrouped member is absent, as an ungrouped design is.
struct Absent;

impl Kind for Absent {
    type Group = u8;
    type Member = u8;

    fn admits(&self, _: u8) -> bool {
        true
    }

    fn ungrouped(&self, _: u8) -> Option<u8> {
        None
    }

    fn rekey(&self, _: &Partition<u8, u8>, group: u8, _: u8) -> u8 {
        group
    }
}

fn partition(placements: &[(u8, u8)]) -> Partition<u8, u8> {
    Partition::new(
        placements
            .iter()
            .map(|&(member, group)| Placement { member, group }),
    )
    .expect("fixture partition")
}

fn pairs(partition: &Partition<u8, u8>) -> Vec<(u8, u8)> {
    partition
        .placements()
        .iter()
        .map(|placement| (placement.member, placement.group))
        .collect()
}

fn group(group: u8, members: &[u8]) -> Group<u8, u8> {
    Group {
        group,
        members: members.to_vec(),
    }
}

#[test]
fn a_member_stands_in_one_group_at_most() {
    assert_eq!(
        Partition::new([
            Placement {
                member: 1,
                group: 1
            },
            Placement {
                member: 1,
                group: 2
            },
        ]),
        Err(GroupingError::Repeated(1))
    );
}

#[test]
fn a_joining_member_lands_last_and_its_emptied_group_ends() {
    let current = partition(&[(1, 1), (2, 1), (3, 3), (4, 4)]);
    let (next, moved) = current
        .apply_move(&Hosted, 3, Destination::Group(1))
        .expect("member joins");
    assert_eq!(pairs(&next), vec![(1, 1), (2, 1), (3, 1), (4, 4)]);
    assert_eq!(
        moved,
        Moved {
            from: Some(3),
            to: Some(1),
            rekeyed: None
        }
    );
    assert!(!next.holds(3), "the group ends with its last member");
    assert_eq!(next.groups(), vec![group(1, &[1, 2, 3]), group(4, &[4])]);
}

#[test]
fn a_member_founds_a_group_in_place_and_a_host_leaving_rekeys_the_rest() {
    let current = partition(&[(1, 1), (2, 1), (3, 1)]);
    let (founded, _) = current
        .apply_move(&Hosted, 2, Destination::Group(9))
        .expect("member founds a group");
    assert_eq!(pairs(&founded), vec![(1, 1), (2, 9), (3, 1)]);

    let (split, moved) = current
        .apply_move(&Hosted, 1, Destination::Ungrouped)
        .expect("host leaves");
    assert_eq!(pairs(&split), vec![(1, 1), (2, 2), (3, 2)]);
    assert_eq!(moved.rekeyed, Some(2));
    assert_eq!(split.lead(2), Some(2), "member zero leads the remainder");
}

#[test]
fn a_move_that_changes_nothing_or_names_no_admitted_member_is_refused() {
    let current = partition(&[(1, 1), (2, 1), (3, 3)]);
    for (member, destination) in [
        (2, Destination::Group(1)),
        (3, Destination::Group(3)),
        (3, Destination::Ungrouped),
    ] {
        assert_eq!(
            current.apply_move(&Hosted, member, destination),
            Err(GroupingError::Unchanged)
        );
    }
    assert_eq!(
        current.apply_move(&Hosted, OUTSIDER, Destination::Group(1)),
        Err(GroupingError::NotAdmitted(OUTSIDER))
    );
    let absent = partition(&[(1, 1)]);
    assert_eq!(
        absent.apply_move(&Absent, 2, Destination::Ungrouped),
        Err(GroupingError::Unchanged)
    );
    let (left, _) = absent
        .apply_move(&Absent, 1, Destination::Ungrouped)
        .expect("last member leaves");
    assert!(left.groups().is_empty(), "the group ends on its last leave");
}

#[test]
fn growth_stops_at_the_limit_but_an_oversized_group_restates_and_shrinks() {
    let full = (1..=MEMBER_LIMIT as u8).map(|member| (member, 1));
    let current = partition(&full.chain([(7, 7), (8, 8)]).collect::<Vec<_>>());
    let refused = Err(GroupingError::MemberLimit {
        members: MEMBER_LIMIT + 1,
    });
    assert_eq!(
        current.apply_move(&Hosted, 7, Destination::Group(1)),
        refused
    );
    assert_eq!(current.place_before(7, 1), refused);
    assert_eq!(
        current.arrange(&[group(1, &[1, 2, 3, 4, 5, 6, 7]), group(8, &[8])]),
        Err(GroupingError::MemberLimit {
            members: MEMBER_LIMIT + 1
        })
    );

    let legacy = partition(&(1..=8).map(|member| (member, 1)).collect::<Vec<_>>());
    let restated = legacy
        .arrange(&[group(1, &[8, 7, 6, 5, 4, 3, 2, 1])])
        .expect("an oversized group restates");
    assert_eq!(restated.partition.lead(1), Some(8));
    legacy
        .place_before(8, 1)
        .expect("an oversized group reorders");
    legacy
        .arrange(&[group(1, &[1, 2, 3, 4, 5, 6, 7]), group(8, &[8])])
        .expect("an oversized group sheds a member into a fresh split");
}

#[test]
fn placing_before_a_member_takes_its_group_and_position() {
    let current = partition(&[(1, 1), (2, 1), (3, 3)]);
    let (next, moved) = current.place_before(3, 2).expect("member placed");
    assert_eq!(pairs(&next), vec![(1, 1), (3, 1), (2, 1)]);
    assert_eq!((moved.from, moved.to), (Some(3), Some(1)));
    assert_eq!(current.place_before(2, 2), Err(GroupingError::PlaceSelf(2)));
    assert_eq!(current.place_before(9, 2), Err(GroupingError::Absent(9)));
    assert_eq!(
        current.place_before(2, 9),
        Err(GroupingError::TargetAbsent(9))
    );
}

#[test]
fn an_arrangement_restates_every_member_exactly_once() {
    let current = partition(&[(1, 1), (2, 1), (3, 3)]);
    let arranged = current
        .arrange(&[group(3, &[3]), group(1, &[2]), group(5, &[1])])
        .expect("reorder and split admitted");
    assert_eq!(pairs(&arranged.partition), vec![(3, 3), (2, 1), (1, 5)]);
    assert_eq!(arranged.splits, vec![(5, 1)]);

    for (proposed, refusal) in [
        (vec![], GroupingError::ArrangeEmpty),
        (
            vec![group(1, &[1, 2]), group(1, &[3])],
            GroupingError::DuplicateGroup(1),
        ),
        (
            vec![group(1, &[1, 2, 2]), group(3, &[3])],
            GroupingError::DuplicateMember(2),
        ),
        (
            vec![group(1, &[1, 2])],
            GroupingError::Incomplete {
                expected: 3,
                missing: 3,
            },
        ),
        (
            vec![group(1, &[1, 2]), group(3, &[3, 9])],
            GroupingError::UnknownMember(9),
        ),
        (
            vec![group(1, &[1, 2]), group(3, &[]), group(4, &[3])],
            GroupingError::VacantGroup(3),
        ),
        (
            vec![group(1, &[1, 2]), group(4, &[3])],
            GroupingError::UnknownGroup(4),
        ),
    ] {
        assert_eq!(current.arrange(&proposed), Err(refusal), "{proposed:?}");
    }
}

#[test]
fn a_stale_revision_is_refused_and_a_current_one_advances_once() {
    assert_eq!(advance_revision(4, 4), Ok(5));
    assert_eq!(
        advance_revision(4, 3),
        Err(RevisionError::Moved { current: 4 })
    );
    assert_eq!(
        advance_revision(u64::MAX, u64::MAX),
        Err(RevisionError::Exhausted)
    );
}
