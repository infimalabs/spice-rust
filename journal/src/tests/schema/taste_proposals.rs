//! A retained statement keeps its task across the schema 99 crossing and loses
//! only its mined taste-proposal evidence.

use super::*;

const PROPOSAL: &str = r#"{"finding":"1111111111111111111111111111111111111111111111111111111111111111","occurrence":"2222222222222222222222222222222222222222222222222222222222222222","recurrence":3}"#;

#[test]
fn writable_crossing_drops_retained_proposal_evidence_and_keeps_the_task() {
    let mut journal = authority_fixture();
    for sql in [
        "UPDATE task_heads SET body = \
         CAST(json_set(CAST(body AS TEXT), '$.taste_proposal', json(?1)) AS BLOB)",
        "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
         '$.fact.taste_proposal', json(?1)) AS BLOB) WHERE kind = 'task::added'",
    ] {
        assert_eq!(journal.connection.execute(sql, [PROPOSAL]).unwrap(), 1);
    }
    restore_predecessor(&journal, SCHEMA_VERSION - 1);

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
    assert_eq!(head.title, "Lay journal baseline");
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let envelope: Envelope<AnyFact> = wire::decode(&bytes).unwrap();
    envelope.fact.read::<wire::body::TaskAdded>().unwrap();
    for stored in [body, bytes] {
        assert!(!String::from_utf8_lossy(&stored).contains("taste_proposal"));
    }
}
