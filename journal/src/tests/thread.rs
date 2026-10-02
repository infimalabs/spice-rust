//! Bounded cause-thread pages through the thread index.

use super::*;

#[test]
fn thread_pages_resume_at_the_first_withheld_member_and_decode_only_the_page() {
    let journal = long_thread_fixture();
    let resolve = |reference: &str| {
        journal
            .read(|read| Ok(read.thread_root(reference)))
            .expect("thread index reads")
    };
    assert_eq!(resolve("150").expect("descendant resolves"), seq(1));
    assert_eq!(resolve("task:1kKRRsDP").expect("origin resolves"), seq(1));
    assert!(matches!(
        resolve("201"),
        Err(crate::ThreadError::NotFound { .. })
    ));
    let page = |at, rows| {
        journal
            .read(|read| read.thread_page(seq(1), at, fact_budget(rows)))
            .expect("thread page reads")
    };
    assert_eq!(
        page(Some(seq(201)), 1),
        None,
        "a cursor outside the thread cannot resume it"
    );

    reset_indexed_read_metrics();
    let first = page(Some(seq(198)), 1).expect("member cursor resumes");
    assert_eq!(
        first.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(198)],
        "the cursor is the first withheld fact, so it is returned"
    );
    assert_eq!(
        first.overflow,
        Some(wire::Overflow {
            omitted: 2,
            continuation: seq(199),
        })
    );
    let metrics = indexed_read_metrics();
    assert_eq!(
        metrics.rows, 2,
        "the page decodes its row plus one overflow probe, not the thread"
    );

    let last = page(first.overflow.map(|overflow| overflow.continuation), 4)
        .expect("continuation resumes");
    assert_eq!(
        last.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        [seq(199), seq(200)]
    );
    assert_eq!(last.overflow, None);
    assert_eq!(
        page(None, 2)
            .expect("thread opens")
            .overflow
            .map(|overflow| overflow.omitted),
        Some(198)
    );
}
