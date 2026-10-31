//! A predecessor holding fact kinds no release produces crosses them into
//! inert evidence once and drops the retained-only horizon projection.

use super::*;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const RECEIPT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const ORPHAN_PREDECESSOR: u32 = 103;
const RELEASE: &str = "schema-103";

fn orphans() -> Vec<serde_json::Value> {
    vec![
        json!({"kind": "consent::floored", "lane": LANE, "occupant": OCCUPANT, "floor": "ask"}),
        json!({"kind": "vendor::armed", "vendor": "omp", "version": "0.1.0"}),
        json!({
            "kind": "commit::landed",
            "commit": "9f2b4c7d1e6a08b35c4f9d2e7a1b8c05d6e3f4a2",
            "receipt": RECEIPT,
            "verdict": "clean",
        }),
        json!({
            "kind": "gate::found",
            "identity": "wire::untyped-body",
            "severity": "high",
            "locus": "wire/src/fact.rs:213",
            "owner": format!("lane:{LANE}"),
            "receipt": RECEIPT,
        }),
        json!({"kind": "gate::cleared", "identity": "wire::untyped-body", "locus": "wire/src/fact.rs:213"}),
        json!({"kind": "up::quiescing", "epoch": EPOCH}),
        json!({"kind": "up::draining", "epoch": EPOCH, "occupants": 1}),
        json!({"kind": "up::terminating", "epoch": EPOCH, "occupants": 1}),
        json!({"kind": "up::killing", "epoch": EPOCH, "occupants": 1}),
        json!({"kind": "up::verifying", "epoch": EPOCH}),
        json!({"kind": "room::horizon", "lane": LANE}),
    ]
}

fn plant_fact(journal: &Journal, position: i64, fact: &serde_json::Value) {
    let envelope = wire::Envelope {
        v: WireVersion::CURRENT,
        seq: seq(u64::try_from(position).unwrap()),
        at: instant(),
        actor: Actor::Spice,
        origin: format!("lane:{LANE}").parse().unwrap(),
        cause: None,
        fact: fact.clone(),
    };
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes \
             (seq, wire_version, instant, actor, origin, cause, kind, envelope) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
            params![
                position,
                envelope.v.0,
                envelope.at.to_string(),
                envelope.actor.to_string(),
                envelope.origin.as_str(),
                fact["kind"].as_str().unwrap(),
                wire::encode(&envelope).unwrap(),
            ],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (?1, ?2, NULL, ?1, 0)",
            params![position, envelope.origin.as_str()],
        )
        .unwrap();
}

#[test]
fn orphan_facts_cross_into_inert_evidence_once_and_the_horizon_projection_drops() {
    let scratch = Scratch::new();
    let path = scratch.path().join("orphan-facts.sqlite3");
    let journal = authority_file_fixture(&path);
    let orphans = orphans();
    let first = journal
        .connection
        .query_row("SELECT max(seq) + 1 FROM fact_envelopes", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let mut last = first;
    for (offset, orphan) in (0..).zip(&orphans) {
        last = first + offset;
        plant_fact(&journal, last, orphan);
    }
    restore_predecessor(&journal, ORPHAN_PREDECESSOR);
    journal
        .connection
        .execute(
            "INSERT INTO room_horizons(lane, head_seq, body) VALUES (?1, ?2, X'7b7d')",
            params![LANE, last],
        )
        .unwrap();
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE cursor_head SET seq = {last}, wake = {last} WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = {last} WHERE singleton = 1;"
        ))
        .unwrap();
    drop(journal);

    let journal = Journal::open(&path).expect("orphan facts cross on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((ORPHAN_PREDECESSOR, SCHEMA_VERSION)),
    );
    let snapshot = journal.fact_snapshot().expect("crossed facts read");
    let crossed = &snapshot.facts()[snapshot.facts().len() - orphans.len()..];
    for (envelope, original) in crossed.iter().zip(&orphans) {
        let retained = envelope
            .fact
            .read::<wire::body::JournalRetained>()
            .unwrap_or_else(|| panic!("{} became inert evidence", original["kind"]));
        assert_eq!(retained.release, RELEASE);
        assert_eq!(&retained.fact, original);
    }
    assert!(!authority_tables(&journal).contains("room_horizons"));
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let journal = Journal::open(&path).expect("crossed authority reopens");
    assert!(journal.migration().is_none(), "the crossing runs once");
}
