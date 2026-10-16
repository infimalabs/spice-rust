//! Writable admission crosses retained terminal reviews without reopening work.

use super::*;

const BEFORE_TYPED_WORKFLOWS: u32 = 71;
const AT: &str = "2026-08-01T06:07:19.000Z";

fn retained(status: &str, review: &str) -> serde_json::Value {
    json!({
        "flow": ["todo", "review"],
        "phase": "review",
        "status": status,
        "review": review,
        "completion": AT,
        "deferral": "2026-08-02T06:07:19.000Z",
        "generation": 17
    })
}

fn replace(journal: &Journal, value: &serde_json::Value) {
    journal
        .connection
        .execute(
            "UPDATE workflow_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            [wire::encode(value).unwrap()],
        )
        .unwrap();
}

fn rows(journal: &Journal) -> Vec<(String, i64, Option<i64>, Vec<u8>)> {
    journal
        .connection
        .prepare("SELECT stamp, head_seq, revised_seq, body FROM workflow_heads ORDER BY stamp")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn retained_terminal_reviews_cross_once_preserving_standing_and_history() {
    for (status, review, expected) in [
        (
            "deleted",
            "unassigned",
            wire::WorkflowStatus::deleted(AT.parse().unwrap()),
        ),
        (
            "deleted",
            "assigned",
            wire::WorkflowStatus::deleted(AT.parse().unwrap()),
        ),
        (
            "deleted",
            "finding",
            wire::WorkflowStatus::deleted(AT.parse().unwrap()),
        ),
        (
            "deleted",
            "clean",
            wire::WorkflowStatus::deleted(AT.parse().unwrap()),
        ),
        (
            "completed",
            "assigned",
            wire::WorkflowStatus::completed(AT.parse().unwrap()),
        ),
        (
            "completed",
            "finding",
            wire::WorkflowStatus::completed(AT.parse().unwrap()),
        ),
    ] {
        let mut journal = authority_fixture();
        replace(&journal, &retained(status, review));
        restore_predecessor(&journal, BEFORE_TYPED_WORKFLOWS);
        let before = rows(&journal);
        let history = journal
            .connection
            .query_row(
                "SELECT (SELECT group_concat(hex(envelope)) FROM fact_envelopes), \
             (SELECT group_concat(hex(response)) FROM command_receipts)",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        let migration = crate::migrate::admit(&mut journal.connection)
            .unwrap()
            .expect("retained workflows require a schema crossing");
        assert_eq!(
            (migration.from(), migration.to()),
            (BEFORE_TYPED_WORKFLOWS, SCHEMA_VERSION)
        );
        let task: TaskStamp = "1kKRRsDP".parse().unwrap();
        let head = journal
            .read(|read| read.workflow_head(&task))
            .unwrap()
            .unwrap()
            .into_body();
        assert_eq!(head.status, expected, "{status}/{review}");
        assert_eq!(head.concluded_positively(), status == "completed");
        assert_eq!(head.generation, 17);
        assert_eq!(
            head.flow,
            vec!["todo".parse().unwrap(), "review".parse().unwrap()]
        );
        assert_eq!(head.phase.as_str(), "review");
        assert_eq!(
            head.deferral,
            Some("2026-08-02T06:07:19.000Z".parse().unwrap())
        );
        let crossed = rows(&journal);
        assert_eq!(
            (&crossed[0].0, crossed[0].1, crossed[0].2),
            (&before[0].0, before[0].1, before[0].2)
        );
        assert_eq!(crossed[0].3, wire::encode(&head).unwrap());
        let after_history = journal
            .connection
            .query_row(
                "SELECT (SELECT group_concat(hex(envelope)) FROM fact_envelopes), \
             (SELECT group_concat(hex(response)) FROM command_receipts)",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(after_history, history);
        assert_eq!(
            crate::migrate::admit(&mut journal.connection).unwrap(),
            None
        );
        assert_eq!(rows(&journal), crossed);
    }
}

#[test]
fn current_workflow_heads_are_not_rewritten_by_the_crossing() {
    let mut journal = authority_fixture();
    restore_predecessor(&journal, BEFORE_TYPED_WORKFLOWS);
    let before = rows(&journal);
    crate::migrate::admit(&mut journal.connection).unwrap();
    assert_eq!(rows(&journal), before);
}

#[test]
fn contradictory_retained_workflows_roll_back_every_head_and_schema_change() {
    for invalid in [
        json!({"status":"pending", "completion": AT}),
        json!({"status":"completed", "completion": null}),
        json!({"status":"deleted", "review":"unknown"}),
        json!({"status":"completed", "assignment":"operator"}),
    ] {
        let mut journal = authority_fixture();
        replace(&journal, &retained("deleted", "unassigned"));
        journal
            .connection
            .execute(
                "INSERT INTO task_heads(stamp,head_seq,body) SELECT '1kKRRsDQ', head_seq, \
             CAST(json_set(CAST(body AS TEXT), '$.born', '1kKRRsDQ') AS BLOB) FROM task_heads",
                [],
            )
            .unwrap();
        let mut malformed = retained("completed", "finding");
        malformed
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        journal
            .connection
            .execute(
                "INSERT INTO workflow_heads(stamp,head_seq,body) VALUES ('1kKRRsDQ',2,?1)",
                [wire::encode(&malformed).unwrap()],
            )
            .unwrap();
        restore_predecessor(&journal, BEFORE_TYPED_WORKFLOWS);
        let before = rows(&journal);
        let error = crate::migrate::admit(&mut journal.connection).unwrap_err();
        assert!(
            matches!(
                error,
                OpenError::Migration(MigrationError::RetainedRow {
                    table: "workflow_heads",
                    release: "schema-72",
                    ..
                })
            ),
            "{error}"
        );
        assert_eq!(rows(&journal), before);
        assert_eq!(
            crate::migrate::inspect_stored_version(&journal.connection).unwrap(),
            BEFORE_TYPED_WORKFLOWS
        );
    }
}
