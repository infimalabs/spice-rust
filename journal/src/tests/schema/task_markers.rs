//! A retained document-slug tag crosses into its typed statement field, while
//! retired taste-proposal markers and every generic statement tag are discarded.

use super::*;

const FINDING: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn predecessor(head_tags: serde_json::Value, fact_tags: serde_json::Value) -> Journal {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.tags', json(?1)) AS BLOB)",
            [head_tags.to_string()],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
             '$.fact.tags', json(?1)) AS BLOB) WHERE kind = 'task::added'",
            [fact_tags.to_string()],
        )
        .unwrap();
    restore_predecessor(&journal, 68);
    journal
}

fn rows(journal: &Journal) -> Vec<Vec<u8>> {
    journal
        .connection
        .prepare(
            "SELECT envelope AS bytes FROM fact_envelopes WHERE kind = 'task::added'
             UNION ALL SELECT body FROM task_heads ORDER BY bytes",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_no_statement_tags(journal: &Journal) {
    let heads: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM task_heads
             WHERE json_type(CAST(body AS TEXT), '$.tags') IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let facts: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM fact_envelopes WHERE kind = 'task::added'
             AND json_type(CAST(envelope AS TEXT), '$.fact.tags') IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((heads, facts), (0, 0));
}

#[test]
fn writable_crossing_lifts_marker_tags_into_typed_fields_once() {
    let mut journal = predecessor(
        json!(["alpha", "document-slug:intro", "omega"]),
        json!([
            "taste-proposal",
            "alpha",
            format!("taste-finding:{FINDING}"),
            "taste-recurrence:3",
            "omega"
        ]),
    );
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (68, SCHEMA_VERSION));

    let body: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let head: TaskHead = wire::decode(&body).unwrap();
    assert_no_statement_tags(&journal);
    assert_eq!(head.document_slug.as_deref(), Some("intro"));

    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let envelope: Envelope<AnyFact> = wire::decode(&bytes).unwrap();
    let added = envelope.fact.read::<wire::body::TaskAdded>().unwrap();
    assert_eq!(added.document_slug, None);

    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn schema_sixty_six_lifts_every_meaning_before_discarding_tags() {
    let mut journal = predecessor(
        json!(["priority:high", "document-slug:intro", "gate-finding"]),
        json!(["gate-finding"]),
    );
    restore_predecessor(&journal, 66);
    crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();

    let body: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let head: TaskHead = wire::decode(&body).unwrap();
    assert_eq!(head.priority, Some(wire::TaskPriority::High));
    assert_eq!(head.document_slug.as_deref(), Some("intro"));

    assert_no_statement_tags(&journal);
}

#[test]
fn schema_sixty_nine_discards_untyped_retired_labels() {
    let mut journal = predecessor(
        json!(["gate-finding", "gate-severity:high"]),
        json!(["gate-finding"]),
    );
    restore_predecessor(&journal, 69);
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (69, SCHEMA_VERSION));
    assert_no_statement_tags(&journal);
}

#[test]
fn marker_tags_without_one_current_meaning_roll_back_the_entire_crossing() {
    for tags in [
        json!(["document-slug:a", "document-slug:b"]),
        json!(["document-slug:"]),
    ] {
        let mut journal = predecessor(tags.clone(), json!([]));
        let before = rows(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(matches!(error, OpenError::Migration(_)), "{tags}: {error}");
        assert_eq!(rows(&journal), before, "{tags}");
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            68,
            "{tags}"
        );
    }
}
