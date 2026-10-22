//! A retained task statement crosses into schema 96 carrying a pitch derived
//! once from its title and description, and a statement with neither refuses
//! the crossing at its row.

use super::*;

fn predecessor(title: &str, description: Option<&str>) -> Journal {
    let journal = authority_fixture();
    let (sql, parameters): (&str, Vec<&str>) = match description {
        Some(description) => (
            "UPDATE task_heads SET body = CAST(json_set(json_remove(CAST(body AS TEXT), \
             '$.pitch'), '$.title', ?1, '$.description', ?2) AS BLOB)",
            vec![title, description],
        ),
        None => (
            "UPDATE task_heads SET body = CAST(json_remove(json_set(CAST(body AS TEXT), \
             '$.title', ?1), '$.pitch', '$.description') AS BLOB)",
            vec![title],
        ),
    };
    journal
        .connection
        .execute(sql, rusqlite::params_from_iter(parameters))
        .unwrap();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.pitch') AS BLOB) WHERE kind = 'task::added'",
            [],
        )
        .unwrap();
    restore_predecessor(&journal, 95);
    journal
}

fn added_fact_pitch(journal: &Journal) -> String {
    journal
        .connection
        .query_row(
            "SELECT json_extract(CAST(envelope AS TEXT), '$.fact.pitch') FROM fact_envelopes \
             WHERE kind = 'task::added' AND json_extract(CAST(envelope AS TEXT), '$.fact.task') \
             = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn head(journal: &Journal) -> wire::TaskHead {
    let body: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    wire::decode(&body).unwrap()
}

#[test]
fn writable_crossing_derives_each_pitch_once_without_a_hold() {
    let mut journal = predecessor(
        "Journal fixture",
        Some("Label. Readers lose the why at the top of every task. Fix follows."),
    );
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (95, SCHEMA_VERSION));
    let crossed = head(&journal);
    assert_eq!(
        crossed.pitch,
        "Readers lose the why at the top of every task."
    );
    assert_eq!(added_fact_pitch(&journal), crossed.pitch);
    assert_eq!(crossed.taste, None);
    assert_eq!(crossed.revision.get(), 1);
}

#[test]
fn writable_crossing_takes_the_title_when_no_description_sentence_exists() {
    let mut journal = predecessor("Journal fixture", None);
    crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!(head(&journal).pitch, "Journal fixture");
}

#[test]
fn writable_crossing_cuts_a_long_sentence_to_the_pitch_bound() {
    let sentence = "why ".repeat(60);
    let mut journal = predecessor("Journal fixture", Some(&sentence));
    crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    let pitch = head(&journal).pitch;
    assert_eq!(pitch.chars().count(), wire::pitch::PITCH_CHARS);
    assert!(pitch.ends_with('…'));
}

#[test]
fn writable_crossing_refuses_a_statement_with_nothing_to_derive_from() {
    let mut journal = predecessor("  ", None);
    let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::RetainedRow {
            table: "task_heads",
            row,
            ..
        }) if row == "1kKRRsDP head_seq=1"
    ));
}
