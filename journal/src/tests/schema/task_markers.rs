//! Retained document-slug and taste-proposal tags cross into typed statement
//! fields.

use super::*;

const FINDING: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OCCURRENCE: &str = "2222222222222222222222222222222222222222222222222222222222222222";

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

#[test]
fn writable_crossing_lifts_marker_tags_into_typed_fields_once() {
    let mut journal = predecessor(
        json!(["alpha", "document-slug:intro", "omega"]),
        json!([
            "taste-proposal",
            "alpha",
            format!("taste-finding:{FINDING}"),
            format!("taste-occurrence:{OCCURRENCE}"),
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
    assert_eq!(head.tags, ["alpha", "omega"]);
    assert_eq!(head.document_slug.as_deref(), Some("intro"));
    assert_eq!(head.taste_proposal, None);

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
    assert_eq!(added.tags, ["alpha", "omega"]);
    assert_eq!(added.document_slug, None);
    assert_eq!(
        added.taste_proposal,
        Some(wire::TasteProposal {
            finding: FINDING.parse().unwrap(),
            occurrence: OCCURRENCE.parse().unwrap(),
            recurrence: 3,
        })
    );

    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn marker_tags_without_one_current_meaning_roll_back_the_entire_crossing() {
    for tags in [
        json!(["document-slug:a", "document-slug:b"]),
        json!(["document-slug:"]),
        json!(["taste-proposal", format!("taste-finding:{FINDING}")]),
        json!([
            "taste-proposal",
            format!("taste-finding:{FINDING}"),
            format!("taste-occurrence:{OCCURRENCE}"),
            "taste-recurrence:1"
        ]),
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
