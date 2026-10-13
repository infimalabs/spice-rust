//! Working-group order, single membership, ending, and bounded group indexes.

use super::*;

#[test]
fn a_group_keeps_its_order_holds_designs_alone_and_ends_without_touching_rooms() {
    let mut journal = Journal::in_memory().expect("journal");
    let first = task(&mut journal, 1, None);
    let second = task(&mut journal, 2, None);
    let third = task(&mut journal, 3, None);
    let id = authority(7);
    let read_group = |journal: &Journal| {
        journal
            .read(|read| {
                Ok((
                    read.group_head(&id)?.map(|head| head.into_body().anchors),
                    read.anchor_group(&second)?
                        .map(|head| head.into_body().group),
                    read.group_head_page(None, fact_budget(8))?.rows.len(),
                ))
            })
            .expect("group reads")
    };

    commit(
        &mut journal,
        group(7, vec![third.clone(), first.clone(), second.clone()]),
    )
    .expect("group stored");
    assert_eq!(
        read_group(&journal),
        (
            Some(vec![third.clone(), first.clone(), second.clone()]),
            Some(id.clone()),
            1
        )
    );
    commit(&mut journal, group(7, vec![first.clone(), third.clone()])).expect("group reordered");
    assert_eq!(
        read_group(&journal),
        (Some(vec![first.clone(), third.clone()]), None, 1)
    );
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let result = commit(&mut journal, group(7, vec![first.clone(), absent]));
    refused(&journal, result);
    let result = commit(&mut journal, group(7, Vec::new()));
    refused(&journal, result);
    let result = commit(&mut journal, group(8, vec![second.clone(), first.clone()]));
    refused(&journal, result);
    assert_eq!(
        read_group(&journal),
        (Some(vec![first.clone(), third.clone()]), None, 1)
    );
    journal.verify().expect("ordered groups verify");

    commit(&mut journal, WorkingGroupRemoved { group: id.clone() }).expect("group ended");
    assert_eq!(read_group(&journal), (None, None, 0));
    let result = commit(&mut journal, WorkingGroupRemoved { group: id.clone() });
    refused(&journal, result);
    assert!(
        journal
            .read(|read| read.task_head(&first))
            .expect("room reads")
            .is_some()
    );
    commit(&mut journal, group(8, vec![first.clone(), second.clone()]))
        .expect("an ended group's designs join another group");
    commit(&mut journal, group(7, vec![third.clone()])).expect("an ended group recreates");
    journal.verify().expect("group history verifies");
}

#[test]
fn group_catalog_pages_by_bytes_without_gaps() {
    let mut journal = Journal::in_memory().expect("journal");
    let groups = [1, 2, 3].map(authority);
    for micros in [1, 2, 3] {
        let anchor = task(&mut journal, micros, None);
        commit(&mut journal, group(micros, vec![anchor])).expect("group commits");
    }

    let row_limited = journal
        .read(|read| read.group_head_page(None, fact_budget(1)))
        .expect("row-limited group page");
    assert_eq!(row_limited.rows.len(), 1);
    assert_eq!(row_limited.continuation, Some(groups[0].clone()));

    let anchor = TaskStamp::for_micros(1).expect("fixture anchor");
    let group_bytes = serde_json::to_vec(&group(1, vec![anchor]).group)
        .expect("group encodes")
        .len();
    let group_budget = FactReadBudget::new(8, group_bytes).expect("one group body");
    let mut after = None;
    for expected in &groups {
        let page = journal
            .read(|read| read.group_head_page(after.as_ref(), group_budget))
            .expect("group page");
        assert_eq!(
            page.rows
                .iter()
                .map(|head| head.body().group.clone())
                .collect::<Vec<_>>(),
            vec![expected.clone()]
        );
        after = page.continuation;
    }
    assert_eq!(after, None);
    let too_small = FactReadBudget::new(8, group_bytes - 1).expect("smaller body budget");
    let indivisible = journal
        .read(|read| read.group_head_page(None, too_small))
        .expect("first indivisible group head advances the page");
    assert_eq!(indivisible.rows[0].body().group, groups[0]);
    assert_eq!(indivisible.continuation, Some(groups[0].clone()));
}
