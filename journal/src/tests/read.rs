//! Scoped reads over an admitted authority.
//!
//! These observe one immutable snapshot through the read capability rather than
//! the connection, so what they prove is what a caller outside this crate can
//! see and what it is told when a head is incomplete.

use super::*;

#[test]
fn scoped_read_returns_owned_cursor_complete_task_bundles_without_mutation() {
    let empty = Journal::in_memory().expect("empty journal");
    let empty_changes = empty.connection.total_changes();
    let (cursor, heads) = empty
        .read(|read| Ok((read.cursor()?, read.task_heads()?)))
        .expect("empty board read");
    assert_eq!(cursor, None);
    assert!(heads.is_empty());
    assert_eq!(empty.connection.total_changes(), empty_changes);

    let journal = authority_fixture();
    let mut second = task_head();
    second.incepted = "1kKRRsDQ".parse().expect("second task stamp");
    second.title = "Second journal task".to_owned();
    insert_head(
        &journal.connection,
        "task_heads",
        "stamp",
        "1kKRRsDQ",
        1,
        &second,
    );
    insert_head(
        &journal.connection,
        "workflow_heads",
        "stamp",
        "1kKRRsDQ",
        2,
        &workflow_head(),
    );
    let changes = journal.connection.total_changes();

    let (cursor, heads) = journal
        .read(|read| Ok((read.cursor()?, read.task_heads()?)))
        .expect("coherent board read");
    let handles = heads
        .into_iter()
        .map(|heads| heads.into_bodies().0.incepted.to_string())
        .collect::<Vec<_>>();

    assert_eq!(cursor, Seq::new(2));
    assert_eq!(handles, ["1kKRRsDP", "1kKRRsDQ"]);
    assert_eq!(journal.connection.total_changes(), changes);
}

#[test]
fn task_bundle_read_names_every_incomplete_or_malformed_head_class() {
    let missing_workflow = authority_fixture();
    missing_workflow
        .connection
        .execute("DELETE FROM workflow_heads WHERE stamp = '1kKRRsDP'", [])
        .expect("remove required workflow head");

    let loose_claim = authority_fixture();
    loose_claim
        .connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for corrupt fixture");
    insert_head(
        &loose_claim.connection,
        "claim_heads",
        "stamp",
        "1kKRRsDQ",
        2,
        &claim_head(),
    );
    loose_claim
        .connection
        .pragma_update(None, "foreign_keys", true)
        .expect("restore foreign keys");

    let mismatched_key = authority_fixture();
    let mut wrong_task = task_head();
    wrong_task.incepted = "1kKRRsDQ".parse().expect("alternate task stamp");
    mismatched_key
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            params![wire::encode(&wrong_task).expect("encode wrong task")],
        )
        .expect("mismatch indexed key and body");

    let invalid_bytes = authority_fixture();
    invalid_bytes
        .connection
        .execute(
            "UPDATE workflow_heads SET body = X'00' WHERE stamp = '1kKRRsDP'",
            [],
        )
        .expect("corrupt canonical body");

    let beyond_cursor = authority_fixture();
    insert_fact(
        &beyond_cursor,
        fact(
            3,
            Some(2),
            FactKind::TaskNoted,
            json!({"task": "1kKRRsDP", "note": "future head fixture"}),
        ),
    );
    beyond_cursor
        .connection
        .execute(
            "UPDATE workflow_heads SET head_seq = 3 WHERE stamp = '1kKRRsDP'",
            [],
        )
        .expect("move head beyond visible cursor");

    for (journal, expected) in [
        (
            missing_workflow,
            "task 1kKRRsDP has a statement head but no workflow head",
        ),
        (loose_claim, "task 1kKRRsDQ has loose claim head(s)"),
        (
            mismatched_key,
            "task head indexed as 1kKRRsDP carries body stamp 1kKRRsDQ",
        ),
        (
            invalid_bytes,
            "workflow_heads stamp \"1kKRRsDP\" is invalid",
        ),
        (
            beyond_cursor,
            "workflow_heads stamp \"1kKRRsDP\" head position 3 is beyond cursor Some(2)",
        ),
    ] {
        let error = journal
            .read(|read| read.task_heads().map(drop))
            .expect_err("malformed task heads refuse");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, found {error}"
        );
    }
}

#[test]
fn cursor_storage_refuses_backward_movement() {
    let journal = authority_fixture();
    let error = journal
        .connection
        .execute(
            "UPDATE cursor_head SET seq = 1, wake = 1 WHERE singleton = 1",
            [],
        )
        .expect_err("cursor cannot move backward");

    assert!(
        error
            .to_string()
            .contains("cursor head cannot move backward")
    );
}
