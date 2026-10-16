//! Retained working-group partitions cross to explicit activity stance.

use super::*;

fn partition(revision: u64) -> serde_json::Value {
    json!({
        "revision": revision,
        "groups": [
            {"group": "1kKRRsDR1", "anchors": ["1kKRRsDP"], "stance": "explore"},
            {"group": "1kKRRsDR2", "anchors": ["1kKRRsDQ"], "stance": "passive"}
        ]
    })
}

fn predecessor() -> Journal {
    let journal = authority_fixture();
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::WorkingGroupMoved,
            json!({
                "design": "1kKRRsDP", "to": "1kKRRsDR1", "partition": partition(1)
            }),
        ),
    );
    insert_fact(
        &journal,
        fact(
            4,
            Some(3),
            FactKind::WorkingGroupsArranged,
            json!({
                "from": partition(1), "partition": partition(2)
            }),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO group_partition(singleton, head_seq, body) VALUES (1, 4, ?1)",
            [serde_json::to_vec(&partition(2)).unwrap()],
        )
        .unwrap();
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT),
             '$.fact.partition.groups[0].stance', '$.fact.from.groups[0].stance') AS BLOB)
         WHERE kind IN ('group::moved', 'group::arranged');
         UPDATE group_partition SET body = CAST(json_remove(CAST(body AS TEXT),
             '$.groups[0].stance') AS BLOB);",
        )
        .unwrap();
    restore_predecessor(&journal, 65);
    journal
}

fn rows(journal: &Journal) -> Vec<Vec<u8>> {
    journal.connection.prepare(
        "SELECT envelope AS bytes FROM fact_envelopes WHERE kind IN ('group::moved', 'group::arranged')
         UNION ALL SELECT body FROM group_partition ORDER BY bytes"
    ).unwrap().query_map([], |row| row.get(0)).unwrap()
        .collect::<Result<_, _>>().unwrap()
}

#[test]
fn writable_crossing_materializes_group_stance_without_changing_explicit_authority() {
    let mut journal = predecessor();
    let before = rows(&journal);
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (65, SCHEMA_VERSION));
    for seq in [3, 4] {
        let bytes: Vec<u8> = journal
            .connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
                [seq],
                |row| row.get(0),
            )
            .unwrap();
        let envelope: Envelope<AnyFact> = wire::decode(&bytes).unwrap();
        assert_eq!(envelope.seq.get(), seq as u64);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["fact"]["partition"],
            partition(if seq == 3 { 1 } else { 2 })
        );
        if seq == 4 {
            assert_eq!(value["fact"]["from"], partition(1));
        }
    }
    let (head, bytes): (i64, Vec<u8>) = journal
        .connection
        .query_row("SELECT head_seq, body FROM group_partition", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(head, 4);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        partition(2)
    );
    assert_ne!(rows(&journal), before);
    let crossed = rows(&journal);
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), crossed);
}

#[test]
fn invalid_group_stance_rolls_back_the_entire_crossing() {
    let mut journal = predecessor();
    journal
        .connection
        .execute_batch(
            "UPDATE group_partition SET body = CAST(json_set(CAST(body AS TEXT),
             '$.groups[1].stance', 'unknown') AS BLOB);",
        )
        .unwrap();
    let before = rows(&journal);
    let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
    assert!(matches!(error, OpenError::Migration(_)));
    assert_eq!(rows(&journal), before);
    assert_eq!(
        crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
        65
    );
}
