//! Scoped reads over an admitted authority.
//!
//! These observe one immutable snapshot through the read capability rather than
//! the connection, so what they prove is what a caller outside this crate can
//! see and what it is told when a head is incomplete.

use super::*;

#[test]
fn selected_facts_use_the_kind_index_and_decode_only_matching_rows() {
    let journal = authority_fixture();
    let plan = journal
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE seq > 0 AND seq <= 2 AND kind IN ('task::phased') ORDER BY seq",
            [],
            |row| row.get::<_, String>(3),
        )
        .expect("selected fact query plan");
    assert!(
        plan.contains("fact_envelopes_kind_seq"),
        "selected fact read must use its kind index: {plan}"
    );

    reset_indexed_read_metrics();
    let selected = journal
        .read(|read| read.selected_facts(&[FactKind::TaskPhased], None))
        .expect("selected facts read");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].seq, Seq::new(2).expect("fixture seq"));
    assert_eq!(selected[0].fact.kind(), FactKind::TaskPhased);
    let metrics = indexed_read_metrics();
    assert_eq!(metrics.queries, 1);
    assert_eq!(metrics.rows, 1);
    assert!(metrics.decoded_bytes > 0);
}

#[test]
fn newest_fact_reads_are_indexed_and_obey_their_page_limit() {
    let journal = authority_fixture();
    let latest_plan = journal
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind = 'task::phased' AND seq <= 2 ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(3),
        )
        .expect("latest fact query plan");
    assert!(
        latest_plan.contains("fact_envelopes_kind_seq"),
        "latest fact read must use its kind index: {latest_plan}"
    );
    let origin_plan = journal
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE origin = 'task:1kKRRsDP' AND kind = 'task::phased' AND seq <= 2 \
             ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(3),
        )
        .expect("recent origin fact query plan");
    assert!(
        origin_plan.contains("fact_envelopes_origin_kind_seq"),
        "recent origin read must use its compound index: {origin_plan}"
    );

    reset_indexed_read_metrics();
    let (latest, recent) = journal
        .read(|read| {
            Ok((
                read.latest_fact(FactKind::TaskPhased)?,
                read.recent_origin_facts(
                    &"task:1kKRRsDP".parse().expect("fixture origin"),
                    FactKind::TaskPhased,
                    None,
                    1,
                )?,
            ))
        })
        .expect("bounded newest fact reads");
    assert_eq!(latest.expect("latest fact").seq, seq(2));
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].seq, seq(2));
    let metrics = indexed_read_metrics();
    assert_eq!((metrics.queries, metrics.rows), (2, 2));
    assert!(metrics.decoded_bytes > 0);
}

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
    second.born = "1kKRRsDQ".parse().expect("second task stamp");
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
        .map(|heads| heads.into_bodies().0.born.to_string())
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
    wrong_task.born = "1kKRRsDQ".parse().expect("alternate task stamp");
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
