use super::*;
use wire::{Actor, Fact, Instant, Seq};

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory fact store");
    connection
        .execute_batch(
            "CREATE TABLE fact_envelopes (
                seq INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                envelope BLOB NOT NULL
            ) STRICT;",
        )
        .expect("fact schema");
    connection
}

fn insert<F: wire::Body>(connection: &Connection, seq: u64, body: F) {
    let envelope = Envelope {
        v: wire::WireVersion::CURRENT,
        seq: Seq::new(seq).expect("positive sequence"),
        at: Instant::new("2030-04-12T18:44:09Z".parse().unwrap()).unwrap(),
        actor: Actor::Spice,
        origin: format!("resident:{}", uuid(9)).parse().unwrap(),
        cause: None,
        fact: Fact { body },
    };
    connection
        .execute(
            "INSERT INTO fact_envelopes VALUES (?1, ?2, ?3)",
            params![
                i64::try_from(seq).unwrap(),
                F::KIND.as_str(),
                wire::encode(&envelope).expect("fixture envelope encodes")
            ],
        )
        .expect("fixture fact inserts");
}

fn ingestion(source: u64, from: u64, to: u64) -> body::JournalIngested {
    body::JournalIngested {
        lane: uuid(1),
        vendor: "codex".parse().unwrap(),
        stream: "session".to_owned(),
        source: uuid(source),
        supersedes: None,
        from: body::TranscriptPosition {
            member: 1,
            offset: from,
        },
        to: body::TranscriptPosition {
            member: 1,
            offset: to,
        },
        usage: Vec::new(),
        non_usage_records: 1,
        residue_records: 0,
    }
}

fn declaration() -> body::LaneSessionDeclared {
    body::LaneSessionDeclared {
        lane: uuid(1),
        occupant: uuid(2),
        worktree: "/worktrees/lane".to_owned(),
        vendor: "codex".parse().unwrap(),
        launch: None,
        epoch: uuid(9),
        stream: "stdout".to_owned(),
        source: None,
        identity: "session-a".to_owned(),
        member: 1,
        offset: 10,
        length: 1,
    }
}

fn stored(connection: &Connection, seq: u64) -> Vec<u8> {
    connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [i64::try_from(seq).unwrap()],
            |row| row.get(0),
        )
        .expect("stored envelope")
}

#[test]
fn retained_late_declaration_gains_the_generation_that_continued_past_it() {
    let connection = fixture();
    insert(&connection, 1, ingestion(6, 0, 20));
    insert(&connection, 2, declaration());
    insert(&connection, 3, ingestion(6, 20, 30));

    migrate(&connection).expect("retained declaration migrates");

    let envelope: Envelope<AnyFact> = wire::decode(&stored(&connection, 2)).unwrap();
    let declaration = envelope
        .fact
        .read::<body::LaneSessionDeclared>()
        .expect("declaration remains typed");
    assert_eq!(declaration.source, Some(uuid(6)));
}

#[test]
fn ambiguous_generation_refuses_before_rewriting_any_declaration() {
    let connection = fixture();
    insert(&connection, 1, ingestion(6, 0, 20));
    insert(&connection, 2, declaration());
    insert(&connection, 3, ingestion(7, 20, 30));
    let before = stored(&connection, 2);

    assert!(matches!(
        migrate(&connection),
        Err(OpenError::Migration(MigrationError::StoredContent {
            from: 30,
            to: 31,
            table: "fact_envelopes",
            ..
        }))
    ));
    assert_eq!(stored(&connection, 2), before);
}
