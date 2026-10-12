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

#[test]
fn links_must_name_existing_authority() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let room = preparation.controller.clone();
    let future = cursor(&journal).expect("nonempty").get() + 1;
    let transcript = RoomSource::Transcript(TranscriptHunk {
        head: transcript_source_head(),
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
        },
        text: TextRange { start: 0, end: 1 },
    });
    for link in [journal_source(0), journal_source(future), transcript] {
        let result = commit(
            &mut journal,
            publication(&room, 1, RoomPublicationContent::Linked { link }),
        );
        refused(&journal, result);
    }
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    let result = commit(
        &mut journal,
        publication(
            &absent,
            1,
            RoomPublicationContent::Authored {
                text: "nowhere".to_owned(),
            },
        ),
    );
    refused(&journal, result);
    let result = commit(&mut journal, preparation.coverage(1, 1));
    refused(&journal, result);
    let result = commit(
        &mut journal,
        preparation.retained(1, vec![preparation.spare.clone()]),
    );
    refused(&journal, result);
    commit(
        &mut journal,
        publication(
            &room,
            1,
            RoomPublicationContent::Linked {
                link: RoomSource::Artifact {
                    digest: digest("a"),
                },
            },
        ),
    )
    .expect("artifact bytes stay with their digest");
    journal.verify().expect("linked authority verifies");
}

#[test]
fn transcript_links_stay_within_the_accounted_generation_and_extent() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let head = transcript_source_head();
    ingest(
        &mut journal,
        head.clone(),
        TranscriptPosition {
            member: 1,
            offset: 0,
        },
        None,
    );
    let linked =
        |head: TranscriptSourceHead, locus: wire::MessageLocus| RoomPublicationContent::Linked {
            link: RoomSource::Transcript(TranscriptHunk {
                head,
                locus,
                text: TextRange { start: 0, end: 1 },
            }),
        };
    let locus = |offset, length| wire::MessageLocus {
        member: 1,
        offset,
        length,
        event: 0,
    };

    commit(
        &mut journal,
        publication(&room, 20, linked(head.clone(), locus(4_095, 1))),
    )
    .expect("the accounted final byte is linkable");
    let mut earlier = head.clone();
    earlier.next.offset = 2_048;
    commit(
        &mut journal,
        publication(&room, 21, linked(earlier, locus(2_047, 1))),
    )
    .expect("an earlier boundary within the current generation is linkable");

    let mut future = head.clone();
    future.next.offset += 1;
    let mut wrong_generation = head.clone();
    wrong_generation.source = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    let mut wrong_stream = head.clone();
    wrong_stream.stream = "sidecar".to_owned();
    for (micros, source) in [
        (22, linked(future, locus(0, 1))),
        (23, linked(head.clone(), locus(4_095, 2))),
        (24, linked(head.clone(), locus(u64::MAX, 2))),
        (25, linked(wrong_generation, locus(0, 1))),
        (26, linked(wrong_stream, locus(0, 1))),
        (27, linked(head.clone(), locus(0, 0))),
    ] {
        let result = commit(&mut journal, publication(&room, micros, source));
        refused(&journal, result);
    }

    let mut advanced = head.clone();
    advanced.next.offset = 8_192;
    ingest(&mut journal, advanced, head.next, None);
    commit(
        &mut journal,
        publication(&room, 28, linked(head.clone(), locus(4_000, 96))),
    )
    .expect("a later head preserves links within the accounted generation");

    let mut next_member = head.clone();
    next_member.next = TranscriptPosition {
        member: 2,
        offset: 0,
    };
    ingest(
        &mut journal,
        next_member.clone(),
        TranscriptPosition {
            member: 1,
            offset: 8_192,
        },
        None,
    );
    let result = commit(
        &mut journal,
        publication(&room, 280, linked(next_member, locus(u64::MAX, 2))),
    );
    refused(&journal, result);

    let mut replacement = head.clone();
    replacement.source = uuid("018f82d4-6f59-7d06-92e3-bce913116398");
    replacement.next.offset = 128;
    ingest(
        &mut journal,
        replacement,
        TranscriptPosition {
            member: 1,
            offset: 0,
        },
        Some(head.source),
    );
    let result = commit(
        &mut journal,
        publication(&room, 29, linked(head, locus(0, 1))),
    );
    refused(&journal, result);

    let expected = projection(&journal);
    journal.verify().expect("accounted links verify");
    journal.reindex().expect("accounted links rebuild");
    assert_eq!(projection(&journal), expected);
}

/// A packet's room record links only its exact decoded-event bytes inside the
/// accounted transcript, and pages beside operator publications in its room.
#[test]
fn messaged_packets_record_exact_transcript_hunks_beside_room_publications() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let head = transcript_source_head();
    ingest(
        &mut journal,
        head.clone(),
        TranscriptPosition {
            member: 1,
            offset: 0,
        },
        None,
    );
    let messaged = |journal: &mut Journal, room: &TaskStamp, micros, event, start, end| {
        let mut write = JournalWrite::new(
            Actor::Lane(head.lane),
            "task:rooms".parse().expect("room origin"),
            instant(),
            Producer::AckScanner,
        );
        write
            .fact(
                RoomMessaged {
                    room: room.clone(),
                    publication: authority(micros),
                    hunk: TranscriptHunk {
                        head: head.clone(),
                        locus: wire::MessageLocus {
                            member: 1,
                            offset: 0,
                            length: 64,
                            event,
                        },
                        text: TextRange { start, end },
                    },
                    author: head.lane,
                    directed: Vec::new(),
                },
                FactCause::Root,
            )
            .expect("messaged fact builds");
        journal.apply(write)
    };

    commit(
        &mut journal,
        publication(
            &room,
            30,
            RoomPublicationContent::Authored {
                text: "operator".to_owned(),
            },
        ),
    )
    .expect("operator publication");
    messaged(&mut journal, &room, 31, 2, 4, 9).expect("the packet hunk records");
    let absent = TaskStamp::for_micros(99_999).expect("absent stamp");
    for result in [
        messaged(&mut journal, &room, 31, 2, 4, 9),
        messaged(&mut journal, &room, 32, 2, 9, 9),
        messaged(&mut journal, &room, 33, u32::MAX, 0, 1),
        messaged(&mut journal, &absent, 34, 2, 4, 9),
    ] {
        refused(&journal, result);
    }

    let page = journal
        .read(|read| read.room_publications(&room, None, fact_budget(8)))
        .expect("room publications read");
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.fact.kind())
            .collect::<Vec<_>>(),
        [FactKind::RoomPublished, FactKind::RoomMessaged]
    );
    journal.verify().expect("messaged publications verify");
}
