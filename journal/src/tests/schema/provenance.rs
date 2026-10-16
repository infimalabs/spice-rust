//! Retained task provenance spellings cross into the closed provenance type.

use super::*;

/// Add one statement head copied from the fixture task under another stamp
/// and provenance spelling.
fn head(journal: &Journal, stamp: &str, provenance: &str) {
    journal
        .connection
        .execute(
            "INSERT INTO task_heads(stamp, head_seq, body) SELECT ?1, head_seq, \
             CAST(json_set(CAST(body AS TEXT), '$.born', ?1, '$.provenance', ?2) AS BLOB) \
             FROM task_heads WHERE stamp = '1kKRRsDP'",
            [stamp, provenance],
        )
        .unwrap();
}

fn steering(journal: &Journal, key: &str) {
    journal
        .connection
        .execute(
            "INSERT INTO steering_heads(directive, steer_key, lane, disposition, created_seq, \
             head_seq, body) VALUES (?1, ?1, 'lane', 'acked', 1, 1, x'7b7d')",
            [key],
        )
        .unwrap();
}

fn predecessor(retained: &[(&str, &str)]) -> Journal {
    let journal = authority_fixture();
    steering(&journal, "dTKHYYzQy");
    for (stamp, provenance) in retained {
        head(&journal, stamp, provenance);
    }
    restore_predecessor(&journal, 67);
    journal
}

fn heads(journal: &Journal) -> Vec<(String, Vec<u8>)> {
    journal
        .connection
        .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn writable_crossing_resolves_each_retained_spelling_through_the_journal() {
    let mut journal = predecessor(&[
        ("1kKRRsDQ", "steering:dTKHYYzQy"),
        ("1kKRRsDR", "review:JOURNAL-1kKRRsDP"),
        ("1kKRRsDS", "observation:1kKRRsDP"),
        ("1kKRRsDT", "gate:complexity"),
        ("1kKRRsDV", "directive:dTKHYYzQz"),
    ]);
    let canonical = heads(&journal)[0].clone();
    let migration = crate::migrate::admit(&mut journal.connection)
        .unwrap()
        .unwrap();
    assert_eq!((migration.from(), migration.to()), (67, SCHEMA_VERSION));
    let crossed = heads(&journal);
    assert_eq!(crossed[0], canonical, "a canonical head is not rewritten");
    let provenance = crossed
        .iter()
        .map(|(stamp, bytes)| {
            let head: TaskHead = wire::decode(bytes).unwrap();
            assert_eq!(&wire::encode(&head).unwrap(), bytes, "{stamp} is canonical");
            head.provenance.to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        provenance,
        [
            "task:1kKRRsDP",
            "ack:dTKHYYzQy",
            "task:1kKRRsDP",
            "task:1kKRRsDP",
            "parentless",
            "parentless",
        ]
    );
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(heads(&journal), crossed);
}

#[test]
fn an_unresolvable_spelling_rolls_back_the_entire_crossing() {
    for retained in ["orphan", "steer:1kKRRsDP"] {
        let mut journal = predecessor(&[("1kKRRsDQ", "steering:dTKHYYzQy")]);
        steering(&journal, "1kKRRsDP");
        head(&journal, "1kKRRsDR", retained);
        let before = heads(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(
            matches!(
                &error,
                OpenError::Migration(MigrationError::RetainedRow {
                    release: "schema-68",
                    ..
                })
            ),
            "{retained}: {error}"
        );
        assert_eq!(heads(&journal), before, "{retained}");
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            67
        );
    }
}
