//! The working-group partition: one revision, standing groups of existing
//! designs, and nothing a fact does not restate exactly.

use super::*;

#[test]
fn the_partition_advances_one_revision_per_fact_and_ends_empty_groups() {
    let mut journal = Journal::in_memory().expect("journal");
    let first = task(&mut journal, 1, None);
    let second = task(&mut journal, 2, None);
    let third = task(&mut journal, 3, None);
    let standing = |journal: &Journal| {
        let current = partition(journal);
        (
            current.revision,
            current
                .groups
                .into_iter()
                .map(|group| (group.group, group.anchors))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(standing(&journal), (0, Vec::new()));

    let founded = moved(&journal, &first, Some(7));
    commit(&mut journal, founded).expect("a design founds a group");
    let joined = moved(&journal, &second, Some(7));
    commit(&mut journal, joined).expect("a design joins a group");
    let other = moved(&journal, &third, Some(8));
    commit(&mut journal, other).expect("a second group follows the first");
    assert_eq!(
        standing(&journal),
        (
            3,
            vec![
                (authority(7), vec![first.clone(), second.clone()]),
                (authority(8), vec![third.clone()]),
            ]
        )
    );

    let stale = arranged(&journal, vec![(8, vec![third.clone()])]);
    let restated = arranged(
        &journal,
        vec![
            (8, vec![third.clone()]),
            (7, vec![second.clone(), first.clone()]),
        ],
    );
    commit(&mut journal, restated).expect("an arrangement restates order");
    let result = commit(&mut journal, stale);
    refused(&journal, result);

    let mut skipped = arranged(&journal, vec![(8, vec![third.clone()])]);
    skipped.partition.revision += 1;
    let result = commit(&mut journal, skipped);
    refused(&journal, result);
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let invented = arranged(&journal, vec![(8, vec![third.clone(), absent])]);
    let result = commit(&mut journal, invented);
    refused(&journal, result);
    let mut misattributed = moved(&journal, &first, Some(8));
    misattributed.from = Some(authority(8));
    let result = commit(&mut journal, misattributed);
    refused(&journal, result);
    let crowd = (10..=16)
        .map(|offset| task(&mut journal, offset, None))
        .collect::<Vec<_>>();
    let overfull = arranged(&journal, vec![(9, crowd)]);
    let result = commit(&mut journal, overfull);
    refused(&journal, result);
    assert_eq!(partition(&journal).revision, 4);

    let left = moved(&journal, &third, None);
    commit(&mut journal, left).expect("the last design leaves its group");
    assert_eq!(
        standing(&journal),
        (5, vec![(authority(7), vec![second.clone(), first.clone()])])
    );
    journal.verify().expect("partition history verifies");
}
