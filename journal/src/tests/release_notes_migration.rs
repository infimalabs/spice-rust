use serde_json::Value;

use super::*;

const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116320";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116321";
const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MANIFEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NOTES: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const TASK_STAMP: &str = "1kKRRsDP";

#[test]
fn schema_four_publication_absence_is_materialized_once_and_remains_replayable() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-four-release.sqlite3");
    schema_four_release_authority(&path, true);

    let mut journal = Journal::open(&path).expect("schema-four publication authority migrates");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((4, SCHEMA_VERSION))
    );
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    let heads = journal
        .read(|read| read.task_heads())
        .expect("combined migration task heads read");
    assert_eq!(
        heads[0].statement().body().incepted.as_str(),
        TASK_STAMP,
        "the release crossing composes through sole task identity",
    );
    let snapshot = journal.fact_snapshot().expect("migrated snapshot opens");
    let published: wire::body::ReleasePublished = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read())
        .expect("migrated publication fact replays");
    assert_eq!(
        published.notes,
        wire::ReleaseNotesIdentity::HistoricalAbsent
    );

    let request = release_request(&journal.connection);
    assert_eq!(
        request.body.notes,
        wire::ReleaseNotesIdentity::HistoricalAbsent
    );
    admit_empty_steering_projection(&mut journal);
    journal.verify().expect("migrated authority verifies");
    drop(journal);

    let reader = JournalReader::open(&path).expect("migrated authority opens read-only");
    assert_eq!(
        reader
            .fact_snapshot()
            .expect("read-only snapshot verifies")
            .facts()
            .len(),
        3
    );
    assert_eq!(
        reader
            .thread("3")
            .expect("publication thread replays")
            .entries
            .len(),
        3
    );
    drop(reader);

    let reopened = Journal::open(&path).expect("migrated authority reopens");
    assert_eq!(
        reopened.migration(),
        None,
        "the migration advances exactly once"
    );
    assert_eq!(
        release_request(&reopened.connection).body.notes,
        wire::ReleaseNotesIdentity::HistoricalAbsent
    );
}

#[test]
fn schema_four_exact_note_identities_survive_the_combined_crossing() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-four-current-release.sqlite3");
    schema_four_release_authority(&path, false);
    let before = release_carriers(&Connection::open(&path).expect("open exact fixture"));

    let mut journal = Journal::open(&path).expect("exact schema-four rows migrate");

    assert_eq!(release_carriers(&journal.connection), before);
    let published: wire::body::ReleasePublished = journal
        .fact_snapshot()
        .expect("exact snapshot")
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read())
        .expect("exact publication fact");
    assert_eq!(
        published.notes.digest().map(ToString::to_string).as_deref(),
        Some(NOTES)
    );
    assert_eq!(
        release_request(&journal.connection)
            .body
            .notes
            .digest()
            .map(ToString::to_string)
            .as_deref(),
        Some(NOTES)
    );
    admit_empty_steering_projection(&mut journal);
    journal.verify().expect("exact migrated authority verifies");
}

fn schema_four_release_authority(path: &Path, remove_note_identity: bool) {
    let journal = authority_file_fixture(path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::ReleasePublished,
            json!({
                "manifest": MANIFEST,
                "notes": NOTES,
                "targets": []
            }),
        ),
    );
    drop_steering_index(&journal.connection);
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (3, 'task:JOURNAL-a1', 2, 1, 2)",
            [],
        )
        .expect("index publication fact");
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 3 WHERE singleton = 1", [])
        .expect("advance publication cursor");
    insert_release_receipt(&journal);
    if remove_note_identity {
        remove_notes(
            &journal.connection,
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 3",
            "/fact",
        );
        remove_notes(
            &journal.connection,
            &format!("SELECT request FROM command_receipts WHERE command_id = '{COMMAND}'"),
            &format!("UPDATE command_receipts SET request = ?1 WHERE command_id = '{COMMAND}'"),
            "/body",
        );
    }
    downgrade_task_identity_to_five(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 4 WHERE singleton = 1;
             PRAGMA user_version = 4;",
        )
        .expect("downgrade release fixture to schema 4");
}

fn insert_release_receipt(journal: &Journal) {
    let id = uuid(COMMAND);
    let epoch = uuid(EPOCH);
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::ReleasePublish,
        expected: Vec::new(),
        body: wire::intent::ReleasePublish {
            key: KEY.parse().expect("release key"),
            manifest: MANIFEST.parse().expect("manifest digest"),
            notes: NOTES.parse::<Digest>().expect("notes digest").into(),
        },
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(3).expect("first seq"),
                last: Seq::new(3).expect("last seq"),
            }),
            body: json!({"key": KEY, "fresh": true}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'spice', 'release::publish', ?2, ?3, ?4, 3, 3, NULL, ?5)",
            params![
                id.to_string(),
                request
                    .canonical_json()
                    .expect("canonical release request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
                wire::encode(&response).expect("encode response"),
                epoch.to_string(),
            ],
        )
        .expect("insert release receipt");
}

fn remove_notes(connection: &Connection, select: &str, update: &str, pointer: &str) {
    let bytes: Vec<u8> = connection
        .query_row(select, [], |row| row.get(0))
        .expect("read publication carrier");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode publication carrier");
    let body = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("publication body");
    body.remove("notes").expect("current carrier has notes");
    connection
        .execute(
            update,
            params![serde_json::to_vec(&document).expect("encode schema-four carrier")],
        )
        .expect("store schema-four carrier");
}

fn release_request(connection: &Connection) -> Command<wire::intent::ReleasePublish> {
    let bytes: Vec<u8> = connection
        .query_row(
            &format!("SELECT request FROM command_receipts WHERE command_id = '{COMMAND}'"),
            [],
            |row| row.get(0),
        )
        .expect("read release request");
    wire::decode(&bytes).expect("release request decodes")
}

#[derive(Debug, PartialEq, Eq)]
struct ReleaseCarriers {
    fact: Value,
    request: Vec<u8>,
}

fn release_carriers(connection: &Connection) -> ReleaseCarriers {
    let envelope: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get(0),
        )
        .expect("read release fact carrier");
    let envelope: Value = serde_json::from_slice(&envelope).expect("decode release fact carrier");
    let request = connection
        .query_row(
            &format!("SELECT request FROM command_receipts WHERE command_id = '{COMMAND}'"),
            [],
            |row| row.get(0),
        )
        .expect("read release request carrier");
    ReleaseCarriers {
        fact: envelope["fact"].clone(),
        request,
    }
}

fn version_records(connection: &Connection) -> (u32, u32) {
    (
        connection
            .query_row(
                "SELECT version FROM journal_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("read schema row version"),
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read schema pragma version"),
    )
}
