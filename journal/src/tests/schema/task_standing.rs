//! Retained priority and taste-review tags cross into typed statement fields.

use super::*;

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
    restore_predecessor(&journal, 66);
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
fn writable_crossing_lifts_standing_tags_into_typed_fields_once() {
    let mut journal = predecessor(
        json!([
            "taste-review:required",
            "alpha",
            "priority:high",
            "taste-review:plan-prepended",
            "omega"
        ]),
        json!(["alpha", "taste-review:required", "omega"]),
    );
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (66, SCHEMA_VERSION));

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
    assert_eq!(head.priority, Some(wire::TaskPriority::High));
    assert_eq!(head.taste, Some(wire::TasteHold::Prepended));

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
    assert_eq!(added.priority, None);
    assert_eq!(added.taste, Some(wire::TasteHold::Authored));

    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn standing_tags_without_one_current_meaning_roll_back_the_entire_crossing() {
    for tags in [
        json!(["priority:urgent"]),
        json!(["priority:high", "priority:low"]),
        json!(["taste-review:plan-prepended"]),
    ] {
        let mut journal = predecessor(tags.clone(), json!([]));
        let before = rows(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(matches!(error, OpenError::Migration(_)), "{tags}: {error}");
        assert_eq!(rows(&journal), before, "{tags}");
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            66,
            "{tags}"
        );
    }
}
