//! A predecessor holding fact kinds no release produces crosses them into
//! inert evidence once, keeping the projections current readers still need
//! and dropping the authority that only those kinds sustained.

use super::*;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
const OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const SOURCE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c5";
const ACKED_DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce9131163d1";
const CANCELED_DIRECTIVE: &str = "018f82d4-6f59-7d06-92e3-bce9131163d2";
const ACKED_KEY: &str = "1kQxbttM";
const CANCELED_KEY: &str = "1kR2YwYV";
const RECEIPT: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SHUTDOWN_PREDECESSOR: u32 = 103;
const WARNING_PREDECESSOR: u32 = 104;

fn shutdown_era_orphans() -> Vec<serde_json::Value> {
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

fn locus() -> serde_json::Value {
    json!({
        "source": SOURCE, "member": 4, "offset": 8213, "length": 512,
        "event": 0, "line": 1, "column": 5
    })
}

fn warning(directive: &str, key: &str) -> serde_json::Value {
    json!({
        "kind": "claim::warned",
        "task": "0000000B",
        "claim": 1,
        "lease": 1,
        "lane": LANE,
        "occupant": OCCUPANT,
        "epoch": EPOCH,
        "expiry": "2030-04-12T19:44:09.271Z",
        "directive": directive,
        "key": key,
        "text": "your lease is about to lapse",
    })
}

/// Two confirmed-then-cleared rulings and two warned directives: one the lane
/// acknowledged, one the resident canceled before a late ACK missed it.
fn warning_era_orphans() -> Vec<serde_json::Value> {
    vec![
        json!({"kind": "taste::confirmed", "taste": "polling", "locus": "turn:48213#3"}),
        json!({"kind": "taste::cleared", "taste": "polling", "locus": "turn:48214#1"}),
        warning(ACKED_DIRECTIVE, ACKED_KEY),
        json!({
            "kind": "steer::acked",
            "directive": ACKED_DIRECTIVE,
            "key": ACKED_KEY,
            "lane": LANE,
            "epoch": EPOCH,
            "locus": locus(),
        }),
        warning(CANCELED_DIRECTIVE, CANCELED_KEY),
        json!({"kind": "steer::canceled", "key": CANCELED_KEY, "lane": LANE}),
        json!({
            "kind": "steer::missed",
            "epoch": EPOCH,
            "key": CANCELED_KEY,
            "lane": LANE,
            "locus": locus(),
            "miss": "historical",
        }),
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

/// Plant `facts` after the fixture's authority, answering their positions.
fn plant_all(journal: &Journal, facts: &[serde_json::Value]) -> Vec<i64> {
    let first = journal
        .connection
        .query_row("SELECT max(seq) + 1 FROM fact_envelopes", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    (first..)
        .zip(facts)
        .map(|(position, fact)| {
            plant_fact(journal, position, fact);
            position
        })
        .collect()
}

fn advance_cursor(journal: &Journal, last: i64) {
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE cursor_head SET seq = {last}, wake = {last} WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = {last} WHERE singleton = 1;"
        ))
        .unwrap();
}

/// Open the planted predecessor, requiring one crossing from `predecessor`
/// that leaves `originals` as the journal's final facts, each inert evidence
/// naming `release`.
fn cross(path: &Path, predecessor: u32, originals: &[serde_json::Value], release: &str) -> Journal {
    let journal = Journal::open(path).expect("orphan facts cross on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((predecessor, SCHEMA_VERSION)),
    );
    let snapshot = journal.fact_snapshot().expect("crossed facts read");
    let crossed = &snapshot.facts()[snapshot.facts().len() - originals.len()..];
    for (envelope, original) in crossed.iter().zip(originals) {
        let retained = envelope
            .fact
            .read::<wire::body::JournalRetained>()
            .unwrap_or_else(|| panic!("{} became inert evidence", original["kind"]));
        assert_eq!(retained.release, release);
        assert_eq!(&retained.fact, original);
    }
    journal
}

fn verify_and_reopen(journal: Journal, path: &Path) {
    journal.verify().expect("crossed authority verifies");
    drop(journal);
    let journal = Journal::open(path).expect("crossed authority reopens");
    assert!(journal.migration().is_none(), "the crossing runs once");
}

#[test]
fn orphan_facts_cross_into_inert_evidence_once_and_the_horizon_projection_drops() {
    let scratch = Scratch::new();
    let path = scratch.path().join("orphan-facts.sqlite3");
    let journal = authority_file_fixture(&path);
    let orphans = shutdown_era_orphans();
    let positions = plant_all(&journal, &orphans);
    let last = *positions.last().unwrap();
    restore_predecessor(&journal, SHUTDOWN_PREDECESSOR);
    journal
        .connection
        .execute(
            "INSERT INTO room_horizons(lane, head_seq, body) VALUES (?1, ?2, X'7b7d')",
            params![LANE, last],
        )
        .unwrap();
    advance_cursor(&journal, last);
    drop(journal);

    let journal = cross(&path, SHUTDOWN_PREDECESSOR, &orphans, "schema-103");
    assert!(!authority_tables(&journal).contains("room_horizons"));
    verify_and_reopen(journal, &path);
}

#[test]
fn retained_rulings_keep_their_tally_and_warnings_take_their_whole_directive() {
    let scratch = Scratch::new();
    let path = scratch.path().join("warnings-and-rulings.sqlite3");
    let journal = authority_file_fixture(&path);
    let orphans = warning_era_orphans();
    let positions = plant_all(&journal, &orphans);
    let last = *positions.last().unwrap();
    restore_predecessor(&journal, WARNING_PREDECESSOR);
    journal
        .connection
        .execute_batch(&format!(
            "INSERT INTO taste_judgments(seq, taste, ordinal, confirmed, cleared) \
                 VALUES ({}, 'polling', 1, 1, 0), ({}, 'polling', 2, 1, 1);
             INSERT INTO steering_heads(directive, steer_key, lane, disposition, created_seq, \
                 head_seq, body) VALUES \
                 ('{ACKED_DIRECTIVE}', '{ACKED_KEY}', '{LANE}', 'acked', {}, {}, X'7b7d'), \
                 ('{CANCELED_DIRECTIVE}', '{CANCELED_KEY}', '{LANE}', 'canceled', {}, {}, X'7b7d');
             INSERT INTO steering_misses(identity, head_seq, body) \
                 VALUES ('historical-miss', {last}, X'7b7d');",
            positions[0], positions[1], positions[2], positions[3], positions[4], positions[5],
        ))
        .unwrap();
    advance_cursor(&journal, last);
    drop(journal);

    let journal = cross(&path, WARNING_PREDECESSOR, &orphans, "schema-104");
    let directives = journal
        .connection
        .query_row(
            "SELECT (SELECT count(*) FROM steering_heads) + (SELECT count(*) FROM steering_misses)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(directives, 0, "no steering authority outlives its warning");
    let page = journal
        .read(|read| {
            let horizon = read.cursor()?.expect("crossed journal has a cursor");
            read.taste_evidence_page("polling", horizon, horizon, 10)
        })
        .expect("crossed taste page reads");
    assert_eq!((page.population, page.confirmed, page.cleared), (2, 1, 1));
    verify_and_reopen(journal, &path);
}
