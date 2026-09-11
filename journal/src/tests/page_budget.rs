//! Independent row and byte probes for every budgeted retained-page API.

use super::*;

#[test]
fn remaining_page_apis_bound_row_and_byte_probes_independently() {
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
            .expect("extend bounded-api causal index");
    }
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 4 WHERE singleton = 1", [])
        .expect("advance bounded-api cursor");
    let origin = "task:1kKRRsDP".parse().expect("fixture origin");
    let budgets = [
        FactReadBudget::new(1, MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("row-limited repository budget"),
        FactReadBudget::new(2, 700_000).expect("byte-limited repository budget"),
    ];

    for budget in budgets {
        reset_indexed_read_metrics();
        let recent = journal
            .read(|read| read.recent_origin_facts(&origin, FactKind::TaskNoted, None, budget))
            .expect("recent-origin page is bounded");
        assert_eq!(recent.rows.len(), 1);
        assert!(recent.continuation.is_some());
        assert_page_probe_metrics(budget);

        reset_indexed_read_metrics();
        let facts = journal
            .read(|read| read.fact_page(FactPageStart::At(seq(3)), seq(4), budget))
            .expect("primary-key page is bounded");
        assert_eq!(facts.rows.len(), 1);
        assert!(facts.overflow.is_some());
        assert_page_probe_metrics(budget);

        reset_indexed_read_metrics();
        let causal = journal
            .read(|read| read.causal_page(seq(4), budget))
            .expect("causal page is bounded")
            .expect("causal position exists");
        assert_eq!(causal.rows.len(), 1);
        assert!(causal.overflow.is_some());
        assert_page_probe_metrics(budget);
    }
}

fn assert_page_probe_metrics(budget: FactReadBudget) {
    let metrics = indexed_read_metrics();
    assert_eq!((metrics.queries, metrics.rows), (1, 2));
    assert!(metrics.decoded_bytes > 0);
    assert!(metrics.decoded_bytes <= budget.decoded_bytes() as u64);
}
