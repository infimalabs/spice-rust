//! Retained task notes cross into their tasks' rooms once; audits stay out.
//! A schema 75 predecessor's notes take their retained spelling first.

use super::*;
use wire::rooms::{RoomAuthorityStamp, RoomPublicationContent, RoomPublished};

const TASK: &str = "1kKRRsDP";
const PREDECESSOR_SCHEMA: u32 = crate::migrate::RETAINED_NOTES_SCHEMA - 1;

fn at(text: &str) -> Instant {
    text.parse().unwrap()
}

/// The identity a note at `seq` and `instant` crosses under.
fn stamp(instant: &str, seq: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(at(instant).get().as_microsecond() + seq).unwrap()
}

fn note(journal: &Journal, position: u64, task: &str, text: &str, instant: &str) {
    let mut envelope = retired_note(position, task, text);
    envelope.at = at(instant);
    plant_root(journal, &envelope);
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
    let journal = &mut authority_fixture();
    let (earlier, later) = ("2030-04-12T18:44:09.001Z", "2030-04-12T18:44:09.004Z");
    note(journal, 3, TASK, "first observation", earlier);
    note(
        journal,
        4,
        TASK,
        "taste-review audit policy=abc123 outcome=clear matches=[]",
        "2030-04-12T18:44:09.002Z",
    );
    note(
        journal,
        5,
        "1kQwPZtK",
        "no such task",
        "2030-04-12T18:44:09.003Z",
    );
    note(journal, 6, TASK, "second observation", later);
    restore_predecessor(journal, PREDECESSOR_SCHEMA);
    assert_eq!(index(journal), [], "a predecessor's rooms hold no note");

    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!(
        (migration.from(), migration.to()),
        (PREDECESSOR_SCHEMA, SCHEMA_VERSION)
    );
    assert_eq!(
        index(journal),
        [
            (TASK.to_owned(), stamp(earlier, 3).to_string(), 3),
            (TASK.to_owned(), stamp(later, 6).to_string(), 6),
        ],
        "only non-audit notes on known tasks join their room, in journal order"
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
    let journal = &mut authority_fixture();
    let instant = "2030-04-12T18:44:09.001Z";
    note(journal, 3, TASK, "taken", instant);
    // Position 1003 shares position 3's last three digits in one millisecond.
    note(journal, 1_003, TASK, "same instant", instant);
    restore_predecessor(journal, PREDECESSOR_SCHEMA);

    crate::migrate::admit(&mut journal.connection).expect_err("one identity names one note");
    assert_eq!(index(journal), []);
    let version: u32 = journal
        .connection
        .query_row("SELECT version FROM journal_schema", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, PREDECESSOR_SCHEMA);
}
