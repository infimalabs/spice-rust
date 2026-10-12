//! Room authority linked to exact transcript hunks: packets recorded in rooms
//! and per-lane catch-up cursors.

use wire::rooms::{RoomDelivered, RoomMessaged};

use super::*;

fn hunk(head: &TranscriptSourceHead) -> TranscriptHunk {
    TranscriptHunk {
        head: head.clone(),
        locus: wire::MessageLocus {
            member: 1,
            offset: 0,
            length: 64,
            event: 0,
        },
        text: TextRange { start: 0, end: 8 },
    }
}

fn deliver(journal: &mut Journal, head: &TranscriptSourceHead, through: Seq) -> Commit {
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("lane:{}", head.lane)
            .parse()
            .expect("resident origin"),
        instant(),
        Producer::ResidentRecognition,
    );
    write
        .fact(
            RoomDelivered {
                through: SequenceCode::new(through.get()),
                hunk: hunk(head),
            },
            FactCause::Root,
        )
        .expect("delivery builds");
    journal.apply(write)
}

fn room_cursor(journal: &Journal, lane: CanonicalUuid) -> Option<(Seq, u64)> {
    journal
        .read(|read| read.room_cursor(lane))
        .expect("room cursor reads")
        .map(|head| (head.position(), head.body().through.get()))
}

fn published(journal: &mut Journal, room: &TaskStamp, micros: i64) -> Seq {
    commit(
        journal,
        publication(
            room,
            micros,
            RoomPublicationContent::Authored {
                text: format!("post {micros}"),
            },
        ),
    )
    .expect("publication commits");
    cursor(journal).expect("publication is the head")
}

/// A landed delivery strictly advances its own logical lane's cursor to an
/// earlier room publication; a renewed generation of that lane continues the
/// same cursor, another lane keeps its own, and replay rebuilds them all.
#[test]
fn landed_deliveries_advance_one_monotone_cursor_per_logical_lane() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let origin = TranscriptPosition {
        member: 1,
        offset: 0,
    };
    let head = transcript_source_head();
    ingest(&mut journal, head.clone(), origin, None);
    let mut other = head.clone();
    other.lane = uuid("018f82d4-6f59-7d06-92e3-bce913116306");
    ingest(&mut journal, other.clone(), origin, None);
    let first = published(&mut journal, &room, 1);
    let second = published(&mut journal, &room, 2);
    let third = published(&mut journal, &room, 3);
    assert_eq!(room_cursor(&journal, head.lane), None, "no delivery landed");

    deliver(&mut journal, &head, first).expect("the first delivery lands");
    let landed = cursor(&journal).expect("delivery is the head");
    assert_eq!(
        room_cursor(&journal, head.lane),
        Some((landed, first.get()))
    );

    let mut unaccounted = head.clone();
    unaccounted.source = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    let task_fact = Seq::new(1).expect("task birth position");
    let future = Seq::new(landed.get() + 1).expect("future position");
    for (from, through) in [
        (&head, first),
        (&head, task_fact),
        (&head, future),
        (&unaccounted, second),
    ] {
        let result = deliver(&mut journal, from, through);
        refused(&journal, result);
    }

    let mut renewed = head.clone();
    renewed.source = uuid("018f82d4-6f59-7d06-92e3-bce913116398");
    ingest(&mut journal, renewed.clone(), origin, Some(head.source));
    deliver(&mut journal, &renewed, second).expect("the renewed lane continues its cursor");
    let continued = cursor(&journal).expect("delivery is the head");
    let result = deliver(&mut journal, &renewed, first);
    refused(&journal, result);
    deliver(&mut journal, &other, third).expect("another lane keeps its own cursor");
    assert_eq!(
        room_cursor(&journal, head.lane),
        Some((continued, second.get()))
    );
    assert_eq!(
        room_cursor(&journal, other.lane).map(|(_, through)| through),
        Some(third.get())
    );

    let expected = projection(&journal);
    journal.verify().expect("cursors verify");
    journal.reindex().expect("cursors rebuild");
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
