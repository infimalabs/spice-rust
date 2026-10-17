use super::*;

#[test]
fn schema_seventy_eight_crosses_authorizations_to_output_decision_identity() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authorization-heads.sqlite3");
    let journal = Journal::open(&path).expect("current authority opens");
    journal
        .connection
        .execute_batch(
            "DROP INDEX authorization_heads_decision;
             ALTER TABLE authorization_heads RENAME TO authorization_heads_multiple;
             CREATE TABLE authorization_heads (
                 output TEXT PRIMARY KEY,
                 decision TEXT NOT NULL,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             INSERT INTO authorization_heads(output, decision, head_seq, body)
                 SELECT output, decision, head_seq, body FROM authorization_heads_multiple;
             DROP TABLE authorization_heads_multiple;
             CREATE INDEX authorization_heads_decision ON authorization_heads(decision);",
        )
        .expect("restore singleton authorization projection");
    restore_predecessor(&journal, 78);
    drop(journal);

    let crossed = Journal::open(&path).expect("schema-78 authority crosses");
    assert_eq!(
        crossed
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((78, SCHEMA_VERSION)),
    );
    let primary_key = crossed
        .connection
        .query_row(
            "SELECT group_concat(part, ',') FROM (
                 SELECT name || ':' || pk AS part
                 FROM pragma_table_info('authorization_heads')
                 WHERE pk > 0
                 ORDER BY cid
             )",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("authorization identity reads");
    assert_eq!(primary_key, "output:1,decision:2");
    crossed.verify().expect("crossed authority verifies");
}
