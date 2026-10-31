//! A predecessor holding Design shadows crosses them into inert evidence.

use super::room_coordination::plant_fact;
use super::*;

/// Last schema whose authority recorded and indexed Design shadows.
const SHADOW_PREDECESSOR: u32 = 103;
const RELEASE: &str = "6e0121328";

fn shadow(score: &str) -> serde_json::Value {
    json!({
        "kind": "room::shadowed",
        "design": "1kKRRsDP",
        "statement": "1",
        "lane": "00000000-0000-4000-8000-000000000001",
        "score": score,
        "floor": "0.1",
    })
}

#[test]
fn retained_shadows_cross_into_inert_evidence_once_and_their_index_drops() {
    let scratch = Scratch::new();
    let path = scratch.path().join("design-shadows.sqlite3");
    let journal = authority_file_fixture(&path);
    let (first, second) = (shadow("0.08"), shadow("0.09"));
    plant_fact(&journal, 3, "room::shadowed", first.clone());
    plant_fact(&journal, 4, "room::shadowed", second.clone());
    journal
        .connection
        .execute_batch(
            "CREATE TABLE design_shadows (
                 design TEXT NOT NULL,
                 seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
                 PRIMARY KEY (design, seq)
             ) STRICT, WITHOUT ROWID;
             CREATE INDEX design_shadows_design_seq ON design_shadows(design, seq);
             INSERT INTO design_shadows(design, seq) VALUES ('1kKRRsDP', 3), ('1kKRRsDP', 4);
             UPDATE cursor_head SET seq = 4, wake = 4 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 4 WHERE singleton = 1;",
        )
        .unwrap();
    restore_predecessor(&journal, SHADOW_PREDECESSOR);
    drop(journal);

    let crossed = |journal: &Journal| {
        let snapshot = journal.fact_snapshot().expect("crossed facts read");
        [2, 3].map(|index| {
            snapshot.facts()[index]
                .fact
                .read::<wire::body::JournalRetained>()
                .expect("a shadow became inert evidence")
        })
    };
    let journal = Journal::open(&path).expect("shadow authority crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((SHADOW_PREDECESSOR, SCHEMA_VERSION)),
    );
    let retained = crossed(&journal);
    for (retained, original) in retained.iter().zip([&first, &second]) {
        assert_eq!(retained.release, RELEASE);
        assert_eq!(
            &retained.fact, original,
            "the retired body is kept verbatim"
        );
    }
    let remaining: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM fact_envelopes WHERE kind = 'room::shadowed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "no row keeps the retired kind");
    assert!(!authority_tables(&journal).contains("design_shadows"));
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let journal = Journal::open(&path).expect("crossed authority reopens");
    assert!(journal.migration().is_none(), "the crossing runs once");
    assert_eq!(
        crossed(&journal),
        retained,
        "a reopen leaves crossed evidence as it was"
    );
}
