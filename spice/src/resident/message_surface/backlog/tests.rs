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
    // Three hundred records exceed the 512 KiB expanded window, while the
    // newest 256 fit inside it. The card target can therefore stop before the
    // season's first byte instead of succeeding only because the fixture is
    // smaller than one expansion.
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
        visible_cards(&scan) >= canvas::stream::MESSAGE_CAPACITY,
        "the selected tail supplies the visible card target",
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
