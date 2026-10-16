//! Retained task notes cross into their tasks' rooms once; audits stay out.

use super::*;
use wire::rooms::{RoomAuthorityStamp, RoomPublicationContent, RoomPublished};

const TASK: &str = "1kKRRsDP";

fn at(text: &str) -> Instant {
    text.parse().unwrap()
}

/// The identity a note at `seq` and `instant` crosses under.
fn stamp(instant: &str, seq: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(at(instant).get().as_microsecond() + seq).unwrap()
}

fn note(journal: &mut Journal, task: &str, text: &str, instant: &str) {
    let mut write = crate::JournalWrite::new(
        Actor::Operator,
        format!("task:{task}").parse().unwrap(),
        at(instant),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::body::TaskNoted {
                task: task.parse().unwrap(),
                note: text.to_owned(),
            },
            crate::FactCause::Root,
        )
        .unwrap();
    journal.apply(write).unwrap();
}

/// Each indexed room publication as `(room, publication, seq)`.
fn index(journal: &Journal) -> Vec<(String, String, i64)> {
    journal
        .connection
        .prepare("SELECT room, publication, seq FROM room_publications ORDER BY seq")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn retained_notes_cross_into_their_rooms_once_without_audits() {
    let mut journal = authority_fixture();
    let (earlier, later) = ("2030-04-12T18:44:09.001Z", "2030-04-12T18:44:09.004Z");
    note(&mut journal, TASK, "first observation", earlier);
    note(
        &mut journal,
        TASK,
        "taste-review audit policy=abc123 outcome=clear matches=[]",
        "2030-04-12T18:44:09.002Z",
    );
    note(
        &mut journal,
        "1kQwPZtK",
        "no such task",
        "2030-04-12T18:44:09.003Z",
    );
    note(&mut journal, TASK, "second observation", later);
    let current = index(&journal);
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    assert_eq!(index(&journal), [], "a predecessor's rooms hold no note");

    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!(
        (migration.from(), migration.to()),
        (SCHEMA_VERSION - 1, SCHEMA_VERSION)
    );
    assert_eq!(
        index(&journal),
        [
            (TASK.to_owned(), stamp(earlier, 3).to_string(), 3),
            (TASK.to_owned(), stamp(later, 6).to_string(), 6),
        ],
        "only non-audit notes on known tasks join their room, in journal order"
    );
    assert_eq!(
        index(&journal),
        current,
        "the crossing indexes as commits do"
    );
    journal
        .verify()
        .expect("verification replays the crossed index");

    let room = TASK.parse().unwrap();
    let shown = journal
        .read(|read| read.room_publication(&room, &stamp(later, 6)))
        .unwrap()
        .expect("the crossed note is a room publication");
    let published = shown
        .fact
        .read::<RoomPublished>()
        .expect("rooms present it");
    assert_eq!(
        (published.room, published.content),
        (
            room,
            RoomPublicationContent::Authored {
                text: "second observation".to_owned()
            }
        )
    );
    assert_eq!((shown.seq.get(), shown.actor), (6, Actor::Operator));
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
}

#[test]
fn notes_sharing_one_identity_in_a_room_roll_back_the_crossing() {
    let mut journal = authority_fixture();
    let instant = "2030-04-12T18:44:09.001Z";
    note(&mut journal, TASK, "taken", instant);
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    // Position 1003 shares position 3's last three digits in one millisecond.
    let mut same = fact(
        1_003,
        None,
        FactKind::TaskNoted,
        json!({"task": TASK, "note": "same instant"}),
    );
    same.at = at(instant);
    insert_fact(&journal, same);

    crate::migrate::admit(&mut journal.connection).expect_err("one identity names one note");
    assert_eq!(index(&journal), []);
    let version: u32 = journal
        .connection
        .query_row("SELECT version FROM journal_schema", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION - 1);
}
