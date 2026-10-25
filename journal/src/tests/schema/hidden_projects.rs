//! A retained row naming a hidden project refuses the crossing into schema 98
//! with the last release that admitted hidden projects and leaves the journal
//! at its predecessor schema; a dotted public label crosses.

use super::*;

fn predecessor_with_project(table: &str, project: &str) -> Journal {
    let journal = authority_fixture();
    let sql = match table {
        "task_heads" => {
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.project', ?1) AS BLOB)"
        }
        _ => {
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.project', ?1) AS BLOB) WHERE kind = 'task::added'"
        }
    };
    assert_eq!(journal.connection.execute(sql, [project]).unwrap(), 1);
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    journal
}

#[test]
fn writable_crossing_refuses_a_retained_hidden_project_at_its_owning_release() {
    for table in ["task_heads", "fact_envelopes"] {
        let mut journal = predecessor_with_project(table, ".secret.work");
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(
            matches!(
                &error,
                OpenError::Migration(MigrationError::RetainedRow {
                    table: refused,
                    release: "8aaba9962",
                    ..
                }) if *refused == table
            ),
            "{error}"
        );
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            SCHEMA_VERSION - 1
        );
    }
}

#[test]
fn writable_crossing_admits_a_dotted_public_project() {
    for table in ["task_heads", "fact_envelopes"] {
        let mut journal = predecessor_with_project(table, "secret.work");
        crate::migrate::admit(&mut journal.connection)
            .unwrap()
            .unwrap();
    }
}
