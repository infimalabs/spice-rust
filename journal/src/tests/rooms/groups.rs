//! Working-group order, removal, and bounded group indexes.

use super::*;

#[test]
fn group_order_is_the_stored_order_and_removal_leaves_rooms_intact() {
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
                    read.anchor_group_page(&second, None, fact_budget(8))?.rows,
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
            vec![id.clone()],
            1
        )
    );
    commit(&mut journal, group(7, vec![first.clone(), third.clone()])).expect("group reordered");
    assert_eq!(
        read_group(&journal),
        (Some(vec![first.clone(), third.clone()]), Vec::new(), 1)
    );
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let result = commit(&mut journal, group(7, vec![first.clone(), absent]));
    refused(&journal, result);
    journal.verify().expect("ordered groups verify");

    commit(&mut journal, WorkingGroupRemoved { group: id.clone() }).expect("group removed");
    assert_eq!(read_group(&journal), (None, Vec::new(), 0));
    let result = commit(&mut journal, WorkingGroupRemoved { group: id.clone() });
    refused(&journal, result);
    assert!(
        journal
            .read(|read| read.task_head(&first))
            .expect("room reads")
            .is_some()
    );
    commit(&mut journal, group(7, vec![second.clone()])).expect("removed group recreates");
    journal.verify().expect("group history verifies");
}

#[test]
fn group_indexes_page_by_bytes_and_fanout_without_gaps() {
    let mut journal = Journal::in_memory().expect("journal");
    let anchor = task(&mut journal, 1, None);
    let groups = [1, 2, 3].map(authority);
    for micros in [1, 2, 3] {
        commit(&mut journal, group(micros, vec![anchor.clone()])).expect("group commits");
    }

    let row_limited = journal
        .read(|read| read.group_head_page(None, fact_budget(1)))
        .expect("row-limited group page");
    assert_eq!(row_limited.rows.len(), 1);
    assert_eq!(row_limited.continuation, Some(groups[0].clone()));
    let membership_limited = journal
        .read(|read| read.anchor_group_page(&anchor, None, fact_budget(1)))
        .expect("row-limited membership page");
    assert_eq!(membership_limited.rows, vec![groups[0].clone()]);
    assert_eq!(membership_limited.continuation, Some(groups[0].clone()));

    let group_bytes = serde_json::to_vec(&group(1, vec![anchor.clone()]).group)
        .expect("group encodes")
        .len();
    let group_budget = FactReadBudget::new(8, group_bytes).expect("one group body");
    let first = journal
        .read(|read| read.group_head_page(None, group_budget))
        .expect("first group page");
    assert_eq!(
        first
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[0].clone()]
    );
    assert_eq!(first.continuation, Some(groups[0].clone()));
    let second = journal
        .read(|read| read.group_head_page(first.continuation.as_ref(), group_budget))
        .expect("second group page");
    assert_eq!(
        second
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[1].clone()]
    );
    assert_eq!(second.continuation, Some(groups[1].clone()));
    let third = journal
        .read(|read| read.group_head_page(second.continuation.as_ref(), group_budget))
        .expect("third group page");
    assert_eq!(
        third
            .rows
            .iter()
            .map(|head| head.body().group.clone())
            .collect::<Vec<_>>(),
        vec![groups[2].clone()]
    );
    assert_eq!(third.continuation, None);
    let too_small = FactReadBudget::new(8, group_bytes - 1).expect("smaller body budget");
    let indivisible = journal
        .read(|read| read.group_head_page(None, too_small))
        .expect("first indivisible group head advances the page");
    assert_eq!(indivisible.rows[0].body().group, groups[0]);
    assert_eq!(indivisible.continuation, Some(groups[0].clone()));

    let identity_bytes = groups[0].to_string().len();
    let anchor_budget = FactReadBudget::new(8, identity_bytes).expect("one identity");
    let first = journal
        .read(|read| read.anchor_group_page(&anchor, None, anchor_budget))
        .expect("first membership page");
    assert_eq!(first.rows, vec![groups[0].clone()]);
    assert_eq!(first.continuation, Some(groups[0].clone()));
    let second = journal
        .read(|read| read.anchor_group_page(&anchor, first.continuation.as_ref(), anchor_budget))
        .expect("second membership page");
    assert_eq!(second.rows, vec![groups[1].clone()]);
    assert_eq!(second.continuation, Some(groups[1].clone()));
    let third = journal
        .read(|read| read.anchor_group_page(&anchor, second.continuation.as_ref(), anchor_budget))
        .expect("third membership page");
    assert_eq!(third.rows, vec![groups[2].clone()]);
    assert_eq!(third.continuation, None);
    let too_small = FactReadBudget::new(8, identity_bytes - 1).expect("smaller identity budget");
    let indivisible = journal
        .read(|read| read.anchor_group_page(&anchor, None, too_small))
        .expect("first indivisible membership advances the page");
    assert_eq!(indivisible.rows, vec![groups[0].clone()]);
    assert_eq!(indivisible.continuation, Some(groups[0].clone()));
}
