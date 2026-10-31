//! A predecessor holding room relevance records crosses them into inert
//! evidence and loses their projection tables.

use super::room_coordination::plant_fact;
use super::*;

/// Last schema whose authority recorded and indexed room relevance.
const RELEVANCE_PREDECESSOR: u32 = 105;
const RELEASE: &str = "2bd90ad06";
const OBSERVATION: &str = "1kKRRsDQ";

/// The relevance projection every predecessor from schema 49 through 105
/// carried.
pub(super) const PREDECESSOR_TABLES: &str = "
CREATE TABLE IF NOT EXISTS room_relevance_observations (
    observation TEXT PRIMARY KEY,
    room TEXT NOT NULL,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    through_seq INTEGER NOT NULL CHECK (through_seq >= 0 AND through_seq < head_seq),
    classification TEXT NOT NULL,
    progress TEXT NOT NULL,
    body BLOB NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS room_relevance_observations_room_seq
    ON room_relevance_observations(room, head_seq);
CREATE TABLE IF NOT EXISTS room_relevance_assessments (
    observation TEXT PRIMARY KEY REFERENCES room_relevance_observations(observation),
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    verdict TEXT NOT NULL,
    body BLOB NOT NULL
) STRICT;";

fn observed() -> serde_json::Value {
    json!({
        "kind": "room::observed",
        "observation": {
            "observation": OBSERVATION,
            "room": "1kKRRsDP",
            "through": "2",
            "sources": [{"state": "available", "source": {"kind": "journal", "fact": "2"}}],
            "classification": "relevant-contribution",
            "progress": "advanced",
            "explanation": "the cited fact advanced item one",
            "exceptions": "",
            "uncertainty": "",
        },
    })
}

fn assessed() -> serde_json::Value {
    json!({
        "kind": "room::assessed",
        "assessment": {
            "observation": OBSERVATION,
            "verdict": "confirmed",
            "explanation": "the classification holds",
        },
    })
}

#[test]
fn retained_relevance_records_cross_into_inert_evidence_once_and_their_tables_drop() {
    let scratch = Scratch::new();
    let path = scratch.path().join("room-relevance.sqlite3");
    let journal = authority_file_fixture(&path);
    let (observed, assessed) = (observed(), assessed());
    plant_fact(&journal, 3, "room::observed", observed.clone());
    plant_fact(&journal, 4, "room::assessed", assessed.clone());
    restore_predecessor(&journal, RELEVANCE_PREDECESSOR);
    journal
        .connection
        .execute_batch(&format!(
            "INSERT INTO room_relevance_observations \
                 (observation, room, head_seq, through_seq, classification, progress, body) \
                 VALUES ('{OBSERVATION}', '1kKRRsDP', 3, 2, 'relevant-contribution', 'advanced', x'00');
             INSERT INTO room_relevance_assessments(observation, head_seq, verdict, body) \
                 VALUES ('{OBSERVATION}', 4, 'confirmed', x'00');
             UPDATE cursor_head SET seq = 4, wake = 4 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 4 WHERE singleton = 1;"
        ))
        .unwrap();
    drop(journal);

    let crossed = |journal: &Journal| {
        let snapshot = journal.fact_snapshot().expect("crossed facts read");
        [2, 3].map(|index| {
            snapshot.facts()[index]
                .fact
                .read::<wire::body::JournalRetained>()
                .expect("a relevance record became inert evidence")
        })
    };
    let journal = Journal::open(&path).expect("relevance authority crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((RELEVANCE_PREDECESSOR, SCHEMA_VERSION)),
    );
    let retained = crossed(&journal);
    for (retained, original) in retained.iter().zip([&observed, &assessed]) {
        assert_eq!(retained.release, RELEASE);
        assert_eq!(
            &retained.fact, original,
            "the retired body is kept verbatim"
        );
    }
    let remaining: i64 = journal
        .connection
        .query_row(
            "SELECT count(*) FROM fact_envelopes \
             WHERE kind IN ('room::observed', 'room::assessed')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "no row keeps a retired kind");
    let tables = authority_tables(&journal);
    assert!(!tables.contains("room_relevance_observations"));
    assert!(!tables.contains("room_relevance_assessments"));
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
