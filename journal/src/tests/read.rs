//! Scoped reads over an admitted authority.
//!
//! These observe one immutable snapshot through the read capability rather than
//! the connection, so what they prove is what a caller outside this crate can
//! see and what it is told when a head is incomplete.

use super::*;
use crate::GuardError;

#[test]
fn selected_fact_pages_use_the_kind_index_and_decode_only_matching_rows() {
    let journal = authority_fixture();
    let plan = journal
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE seq > 0 AND seq <= 2 AND kind IN ('task::phased') \
             ORDER BY seq LIMIT 257",
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
        .read(|read| {
            read.selected_fact_page(&[FactKind::TaskPhased], None, FactReadBudget::RETAINED)
        })
        .expect("selected fact page read");
    assert_eq!(selected.rows.len(), 1);
    assert_eq!(selected.rows[0].seq, Seq::new(2).expect("fixture seq"));
    assert_eq!(selected.rows[0].fact.kind(), FactKind::TaskPhased);
    assert_eq!(selected.continuation, None);
    assert_eq!(selected.horizon, Seq::new(2));
    let metrics = indexed_read_metrics();
    assert_eq!(metrics.queries, 1);
    assert_eq!(metrics.rows, 1);
    assert!(metrics.decoded_bytes > 0);
}

#[test]
fn retained_fact_pages_bound_matching_rows_and_bytes_independently_of_history() {
    let journal = authority_fixture();
    for position in 3..=302 {
        insert_fact(
            &journal,
            fact(
                position,
                None,
                FactKind::TaskAdded,
                json!({
                    "task": "1kKRRsDP",
                    "title": "unrelated retained authority",
                    "project": "journal",
                    "tags": [],
                    "depends": [],
                    "flow": ["todo", "review"],
                    "phase": "todo",
                    "revision": position
                }),
            ),
        );
    }
    for position in 303..=602 {
        insert_fact(
            &journal,
            fact(
                position,
                None,
                FactKind::TaskPhased,
                json!({"task": "1kKRRsDP", "phase": "review"}),
            ),
        );
    }
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 602 WHERE singleton = 1", [])
        .expect("advance retained-history fixture cursor");

    let row_budget =
        FactReadBudget::new(4, MAX_RETAINED_FACT_PAGE_BYTES).expect("small row budget is admitted");
    reset_indexed_read_metrics();
    let first = journal
        .read(|read| read.selected_fact_page(&[FactKind::TaskPhased], None, row_budget))
        .expect("bounded retained page");
    assert_eq!(
        first.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(2), seq(303), seq(304), seq(305)]
    );
    assert_eq!(first.continuation, Some(seq(305)));
    let row_metrics = indexed_read_metrics();
    assert_eq!((row_metrics.queries, row_metrics.rows), (1, 5));
    assert!(row_metrics.decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64);

    reset_indexed_read_metrics();
    let second = journal
        .read(|read| {
            read.selected_fact_page(&[FactKind::TaskPhased], first.continuation, row_budget)
        })
        .expect("retained page resumes after its cursor");
    assert_eq!(
        second.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(306), seq(307), seq(308), seq(309)]
    );
    assert_eq!(second.continuation, Some(seq(309)));
    let resumed_metrics = indexed_read_metrics();
    assert_eq!((resumed_metrics.queries, resumed_metrics.rows), (1, 5));
    assert!(resumed_metrics.decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64);

    let first_bytes: i64 = journal
        .connection
        .query_row(
            "SELECT length(envelope) FROM fact_envelopes WHERE seq = 2",
            [],
            |row| row.get(0),
        )
        .expect("measure first matching fact");
    let first_bytes = usize::try_from(first_bytes).expect("fact length fits this platform");
    let byte_budget = FactReadBudget::new(MAX_RETAINED_FACT_PAGE_ROWS, first_bytes)
        .expect("exact first-row byte budget is admitted");
    reset_indexed_read_metrics();
    let bytes = journal
        .read(|read| read.selected_fact_page(&[FactKind::TaskPhased], None, byte_budget))
        .expect("byte-bounded retained page");
    assert_eq!(bytes.rows.len(), 1);
    assert_eq!(bytes.continuation, Some(seq(2)));
    let byte_metrics = indexed_read_metrics();
    assert_eq!(byte_metrics.queries, 1);
    assert_eq!(byte_metrics.rows, 2);
    assert_eq!(byte_metrics.decoded_bytes, first_bytes as u64);

    let too_small = FactReadBudget::new(MAX_RETAINED_FACT_PAGE_ROWS, first_bytes - 1)
        .expect("smaller positive byte budget is admitted");
    reset_indexed_read_metrics();
    let error = journal
        .read(|read| read.selected_fact_page(&[FactKind::TaskPhased], None, too_small))
        .expect_err("a call cannot decode even its first row beyond budget");
    assert!(error.to_string().contains("this call's"));
    assert_eq!(indexed_read_metrics().decoded_bytes, 0);

    reset_indexed_read_metrics();
    let recent = journal
        .read(|read| read.recent_fact_page(FactKind::TaskPhased, None, row_budget))
        .expect("bounded newest-first retained page");
    assert_eq!(
        recent.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(602), seq(601), seq(600), seq(599)]
    );
    assert_eq!(recent.continuation, Some(seq(599)));
    let recent_metrics = indexed_read_metrics();
    assert_eq!((recent_metrics.queries, recent_metrics.rows), (1, 5));
    assert!(recent_metrics.decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64);
}

#[test]
fn every_retained_page_surface_keeps_decoded_bytes_within_the_durable_maximum() {
    let journal = authority_fixture();
    for (position, cause) in [(3, 2), (4, 3)] {
        insert_fact(
            &journal,
            fact(
                position,
                Some(cause),
                FactKind::TaskNoted,
                json!({"task": "1kKRRsDP", "note": "x".repeat(600_000)}),
            ),
        );
        journal
            .connection
            .execute(
                "INSERT INTO thread_index(seq, origin, cause, root, depth)
                 VALUES (?1, 'task:1kKRRsDP', ?2, 1, ?3)",
                params![position as i64, cause as i64, position as i64 - 1],
            )
            .expect("extend large-fact causal index");
    }
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 4 WHERE singleton = 1", [])
        .expect("advance large-fact cursor");

    let assert_bounded = |read: &dyn Fn() -> Result<(), GuardError>| {
        reset_indexed_read_metrics();
        let _ = read();
        assert!(
            indexed_read_metrics().decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64,
            "one retained page decoded beyond the durable maximum"
        );
    };
    assert_bounded(&|| {
        journal.read(|read| {
            read.selected_fact_page(
                &[FactKind::TaskNoted],
                Some(seq(2)),
                FactReadBudget::RETAINED,
            )?;
            Ok(())
        })
    });
    assert_bounded(&|| {
        journal.read(|read| {
            read.recent_fact_page(FactKind::TaskNoted, None, FactReadBudget::RETAINED)?;
            Ok(())
        })
    });
    assert_bounded(&|| {
        journal.read(|read| {
            read.latest_fact(FactKind::TaskNoted)?;
            Ok(())
        })
    });
    assert_bounded(&|| {
        journal.read(|read| {
            read.recent_origin_facts(
                &"task:1kKRRsDP".parse().expect("fixture origin"),
                FactKind::TaskNoted,
                None,
                fact_budget(2),
            )?;
            Ok(())
        })
    });
    assert_bounded(&|| {
        journal.read(|read| {
            read.fact_page(FactPageStart::At(seq(3)), seq(4), fact_budget(2))?;
            Ok(())
        })
    });
    assert_bounded(&|| {
        journal.read(|read| {
            read.causal_page(seq(4), fact_budget(2))?;
            Ok(())
        })
    });
}

#[test]
fn recent_origin_facts_resume_after_a_byte_bounded_prefix_without_fault() {
    let journal = authority_fixture();
    for position in [3, 4] {
        insert_fact(
            &journal,
            fact(
                position,
                None,
                FactKind::TaskNoted,
                json!({"task": "1kKRRsDP", "note": "x".repeat(600_000)}),
            ),
        );
    }
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 4 WHERE singleton = 1", [])
        .expect("advance large recent-origin cursor");
    let origin = "task:1kKRRsDP".parse().expect("fixture origin");

    reset_indexed_read_metrics();
    let first = journal
        .read(|read| read.recent_origin_facts(&origin, FactKind::TaskNoted, None, fact_budget(2)))
        .expect("first byte-bounded recent-origin page");
    assert_eq!(
        first.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(4)]
    );
    assert_eq!(first.continuation, Some(seq(4)));
    assert!(indexed_read_metrics().decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64);

    reset_indexed_read_metrics();
    let second = journal
        .read(|read| {
            read.recent_origin_facts(
                &origin,
                FactKind::TaskNoted,
                first.continuation,
                fact_budget(2),
            )
        })
        .expect("second byte-bounded recent-origin page");
    assert_eq!(
        second.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(3)]
    );
    assert_eq!(second.continuation, None);
    assert!(indexed_read_metrics().decoded_bytes <= MAX_RETAINED_FACT_PAGE_BYTES as u64);
}

#[test]
fn retained_fact_readers_refuse_oversized_corruption_before_decoding_it() {
    let journal = authority_fixture();
    journal
        .connection
        .execute_batch(
            "DROP TRIGGER fact_envelopes_envelope_size_insert;
             DROP TRIGGER fact_envelopes_envelope_size_update;",
        )
        .expect("open a corruption fixture beneath finalized enforcement");
    insert_fact(
        &journal,
        fact(
            3,
            None,
            FactKind::TaskAdded,
            json!({
                "task": "2kKRRsDP",
                "title": "x".repeat(MAX_RETAINED_FACT_PAGE_BYTES + 1),
                "project": "journal",
                "tags": [],
                "depends": [],
                "flow": ["todo", "review"],
                "phase": "todo",
                "revision": 1
            }),
        ),
    );
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 3 WHERE singleton = 1", [])
        .expect("advance oversized-fact fixture cursor");
    journal
        .connection
        .execute_batch(&crate::migrate::bounds_sql())
        .expect("restore finalized enforcement around planted corruption");

    reset_indexed_read_metrics();
    let selected = journal
        .read(|read| {
            read.selected_fact_page(
                &[FactKind::TaskAdded],
                Some(seq(2)),
                FactReadBudget::RETAINED,
            )
        })
        .expect_err("oversized selected fact is corruption");
    assert!(selected.to_string().contains("sequence 3"));
    assert!(selected.to_string().contains("exceeding"));
    assert_eq!(indexed_read_metrics().decoded_bytes, 0);

    reset_indexed_read_metrics();
    let recent = journal
        .read(|read| read.recent_fact_page(FactKind::TaskAdded, None, FactReadBudget::RETAINED))
        .expect_err("oversized recent fact is corruption");
    assert!(recent.to_string().contains("sequence 3"));
    assert_eq!(indexed_read_metrics().decoded_bytes, 0);

    let snapshot = journal
        .fact_snapshot()
        .expect_err("unbounded snapshot still rejects before allocating corruption");
    assert!(snapshot.to_string().contains("Some(3)"));
    assert!(snapshot.to_string().contains("exceeding"));
}

#[test]
fn retained_fact_budget_refuses_zero_and_platform_maximum_work() {
    reset_indexed_read_metrics();
    for (rows, bytes) in [(0, 1), (1, 0), (usize::MAX, 1), (1, usize::MAX)] {
        assert!(FactReadBudget::new(rows, bytes).is_err());
    }
    assert_eq!(indexed_read_metrics(), Default::default());
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
                    fact_budget(1),
                )?,
            ))
        })
        .expect("bounded newest fact reads");
    assert_eq!(latest.expect("latest fact").seq, seq(2));
    assert_eq!(recent.rows.len(), 1);
    assert_eq!(recent.rows[0].seq, seq(2));
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
