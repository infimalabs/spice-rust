use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

#[test]
fn visible_card_target_stops_a_season_tail_before_byte_zero() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-tail-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("season tape opens");
    // Three hundred records exceed the first 32 KiB window, while more than
    // one initial page fits inside it. Cold replay must stop there rather than
    // expanding until the complete retained history is materialized.
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..300 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("season is durable");

    let scan = tail_backlog(&root, listen::Dialect::Codex).expect("season tail scans");
    assert!(
        visible_cards(&scan) > canvas::stream::INITIAL_MESSAGE_LIMIT,
        "the selected tail supplies one initial page and its look-behind card",
    );
    assert!(
        visible_cards(&scan) < canvas::stream::MESSAGE_CAPACITY,
        "cold replay does not materialize the retained-history horizon",
    );
    let first_offset = scan
        .records
        .first()
        .and_then(|record| match &record.payload {
            listen::RecordPayload::Events(events) => events.first(),
            listen::RecordPayload::Residue(_) => None,
        })
        .and_then(|event| event.spans.first())
        .map(|span| span.offset)
        .expect("tail has a first sourced event");
    assert!(
        first_offset > 0,
        "the visible-card target is reached without scanning byte zero",
    );

    fs::remove_dir_all(root).expect("season fixture cleanup");
}

#[test]
fn tool_only_tail_expands_until_the_initial_visible_budget_is_satisfied() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-backlog-sparse-tail-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let tape = tape::Tape::open(&root).expect("sparse tape opens");
    let filler = "x".repeat(1_800);
    let mut season = Vec::new();
    for index in 0..30 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-{index}\",\"type\":\"agent_message\",\"text\":\"{index} {filler}\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    for index in 0..400 {
        season.extend_from_slice(
            format!(
                "{{\"type\":\"item.started\",\"item\":{{\"id\":\"call-{index}\",\"type\":\"command_execution\",\"command\":\"echo {filler}\",\"aggregated_output\":\"\",\"exit_code\":null,\"status\":\"in_progress\"}}}}\n"
            )
            .as_bytes(),
        );
    }
    tape.tee(&season).expect("sparse season is durable");

    let scan = tail_backlog(&root, listen::Dialect::Codex).expect("sparse tail scans");
    assert_eq!(
        visible_cards(&scan),
        30,
        "tool activity does not consume the visible-card budget",
    );
    assert_eq!(
        scan.records.len(),
        430,
        "the scanner crosses the sparse tool tail to reach visible cards",
    );

    fs::remove_dir_all(root).expect("sparse fixture cleanup");
}
