use super::*;

const RETIRED: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
const LATE: &str = "018f82d4-6f59-7d06-92e3-bce9131163ff";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";

#[test]
fn crossing_preserves_retired_bytes_live_receipts_and_facts_and_reopens_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retired.sqlite3");
    let old = predecessor(&path);
    insert_retired(&old.connection, RETIRED);
    let retired = receipt(&old.connection, "command_receipts", RETIRED);
    let live = receipt(
        &old.connection,
        "command_receipts",
        "018f82d4-6f59-7d06-92e3-bce9131163c4",
    );
    let facts = fact_bytes(&old.connection);
    let request: Vec<u8> = old
        .connection
        .query_row(
            "SELECT request FROM command_receipts WHERE command_id = ?1",
            [RETIRED],
            |row| row.get(0),
        )
        .expect("historical request reads");
    assert!(wire::decode::<Command<Value>>(&request).is_err());
    assert!("gate::findings".parse::<CommandKind>().is_err());
    drop(old);

    let migrated = Journal::open(&path).expect("historical authority migrates");
    assert_eq!(
        migrated.migration().map(|step| (step.from(), step.to())),
        Some((26, SCHEMA_VERSION))
    );
    assert_eq!(
        receipt(&migrated.connection, "historical_command_receipts", RETIRED),
        retired
    );
    assert_eq!(
        receipt(
            &migrated.connection,
            "command_receipts",
            "018f82d4-6f59-7d06-92e3-bce9131163c4"
        ),
        live
    );
    assert_eq!(fact_bytes(&migrated.connection), facts);
    assert_eq!(
        migrated
            .connection
            .query_row(
                "SELECT count(*) FROM command_receipts WHERE command_id = ?1",
                [RETIRED],
                |row| row.get::<_, u32>(0)
            )
            .expect("live count"),
        0
    );
    assert_eq!(
        migrated
            .verify()
            .expect("both receipt authorities verify")
            .receipts,
        2
    );
    drop(migrated);
    let reopened = Journal::open(&path).expect("migrated authority reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(
        receipt(&reopened.connection, "historical_command_receipts", RETIRED),
        retired
    );
    assert_eq!(
        reopened
            .verify()
            .expect("reopened authority verifies")
            .receipts,
        2
    );
}

#[test]
fn late_malformed_receipt_rolls_back_prior_copies_and_both_schema_markers() {
    let scratch = Scratch::new();
    let path = scratch.path().join("rollback.sqlite3");
    let old = predecessor(&path);
    insert_retired(&old.connection, RETIRED);
    insert_retired(&old.connection, LATE);
    old.connection
        .execute(
            "UPDATE command_receipts SET evaluated_witnesses = ?1 WHERE command_id = ?2",
            params![b"[null]".as_slice(), LATE],
        )
        .expect("corrupt later witnesses");
    let before = receipt(&old.connection, "command_receipts", RETIRED);
    let malformed = receipt(&old.connection, "command_receipts", LATE);
    drop(old);
    assert!(
        matches!(Journal::open(&path), Err(OpenError::Migration(MigrationError::StoredContent { from: 26, to: 27, ref row, .. })) if row == LATE)
    );
    let connection = Connection::open(&path).expect("refused authority reads");
    assert_eq!(receipt(&connection, "command_receipts", RETIRED), before);
    assert_eq!(receipt(&connection, "command_receipts", LATE), malformed);
    assert_eq!(
        connection
            .query_row("SELECT version FROM journal_schema", [], |row| row
                .get::<_, u32>(0))
            .expect("schema row"),
        26
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("schema pragma"),
        26
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'historical_command_receipts'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .expect("archive absent"),
        0
    );
}

#[test]
fn unknown_retired_kind_refuses_instead_of_becoming_unchecked_history() {
    let scratch = Scratch::new();
    let path = scratch.path().join("unknown.sqlite3");
    let old = predecessor(&path);
    insert_retired(&old.connection, RETIRED);
    old.connection
        .execute(
            "UPDATE command_receipts SET command_kind = 'gate::unknown' WHERE command_id = ?1",
            [RETIRED],
        )
        .expect("plant unknown kind");
    let before = receipt(&old.connection, "command_receipts", RETIRED);
    drop(old);
    assert!(matches!(
        Journal::open(&path),
        Err(OpenError::Migration(MigrationError::StoredContent {
            from: 26,
            to: 27,
            ..
        }))
    ));
    let connection = Connection::open(&path).expect("refused authority reads");
    assert_eq!(receipt(&connection, "command_receipts", RETIRED), before);
}

#[test]
fn archived_identity_cannot_replay_or_commit_as_a_new_live_command() {
    let scratch = Scratch::new();
    let path = scratch.path().join("identity.sqlite3");
    let old = predecessor(&path);
    insert_retired(&old.connection, RETIRED);
    drop(old);
    let journal = Journal::open(&path).expect("authority migrates");
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(RETIRED),
        epoch: uuid(EPOCH),
        kind: CommandKind::BoardRead,
        expected: Vec::new(),
        body: json!({"scope": "all"}),
    }
    .admit(&wire::Accepted {
        wire: WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: uuid(EPOCH),
    })
    .expect("different current command admits");
    assert_eq!(
        journal
            .read(|read| read.command_replay(&command))
            .expect("identity reads"),
        Some(crate::CommandReplay::Conflict)
    );
    assert!(
        journal
            .connection
            .execute(
                "INSERT INTO command_receipts SELECT * FROM historical_command_receipts",
                []
            )
            .is_err()
    );
    assert!(
        journal
            .connection
            .execute("UPDATE command_receipts SET command_id = ?1", [RETIRED])
            .is_err()
    );
    assert_eq!(
        journal
            .verify()
            .expect("failed reuse leaves authority unchanged")
            .receipts,
        2
    );
}

#[test]
fn historical_corruption_is_reported_without_weakening_current_receipts() {
    for corrupt in [
        "UPDATE historical_command_receipts SET command_kind = 'gate::unknown'",
        "UPDATE historical_command_receipts SET epoch = '018f82d4-6f59-7d06-92e3-bce913116399'",
        "UPDATE historical_command_receipts SET first_seq = 2",
        "UPDATE historical_command_receipts SET evaluated_witnesses = X'5b6e756c6c5d'",
        "UPDATE historical_command_receipts SET request = CAST(request || ' ' AS BLOB)",
        "UPDATE command_receipts SET command_kind = 'gate::findings'",
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join("corruption.sqlite3");
        let old = predecessor(&path);
        insert_retired(&old.connection, RETIRED);
        drop(old);
        let journal = Journal::open(&path).expect("authority migrates");
        journal
            .connection
            .execute_batch(corrupt)
            .expect("plant corruption");
        let violations = refused_violations(&journal);
        assert!(
            violations
                .iter()
                .any(|v| v.class == ViolationClass::Receipt),
            "corruption escaped verification: {corrupt}"
        );
    }
}

#[cfg(unix)]
#[test]
fn authenticated_restore_keeps_and_verifies_historical_receipt_evidence() {
    use std::os::unix::fs::PermissionsExt;
    use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};

    let scratch = Scratch::new();
    let authority = scratch.path().join("spicejournal.sqlite3");
    let old = predecessor(&authority);
    insert_retired(&old.connection, RETIRED);
    let before = receipt(&old.connection, "command_receipts", RETIRED);
    drop(old);
    drop(Journal::open(&authority).expect("authority migrates before backup"));
    let bytes = fs::read(&authority).expect("checkpointed authority reads");
    let manifest = JournalBackupManifest {
        version: JOURNAL_BACKUP_VERSION,
        wire: WireVersion::CURRENT,
        journal_schema: SCHEMA_VERSION,
        authority: "spicejournal.sqlite3".to_owned(),
        members: vec![JournalBackupMember {
            path: "spicejournal.sqlite3".to_owned(),
            mode: fs::metadata(&authority)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o7777,
            bytes: u64::try_from(bytes.len()).expect("byte count"),
            digest: Digest::compute(wire::Domain::JournalBackupMember, &bytes),
        }],
    };
    fs::write(
        scratch.path().join("manifest.json"),
        wire::encode(&manifest).expect("manifest encodes"),
    )
    .expect("manifest writes");
    let capture =
        crate::capture_restore_bundle(scratch.path()).expect("historical backup authenticates");
    assert_eq!(capture.verification().receipts, 2);
    let restored_scratch = Scratch::new();
    let restored_path = restored_scratch.path().join("restored.sqlite3");
    let mut destination = fs::File::create(&restored_path).expect("restore destination opens");
    capture
        .authority()
        .copy_to(&mut destination)
        .expect("authenticated authority restores");
    drop(destination);
    let restored = JournalReader::open(&restored_path).expect("captured authority opens");
    assert_eq!(
        restored
            .verify()
            .expect("restored evidence verifies")
            .receipts,
        2
    );
    let connection = Connection::open(&restored_path).expect("captured row reads");
    assert_eq!(
        receipt(&connection, "historical_command_receipts", RETIRED),
        before
    );
}

fn predecessor(path: &Path) -> Journal {
    let mut journal = authority_file_fixture(path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::GateFindingsFiled,
            json!({
                "report": "a".repeat(64), "findings": 1, "created": 1, "reused": 0,
            }),
        ),
    );
    journal.reindex().expect("historical fact thread indexes");
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 3, wake = 3;
         UPDATE steering_index SET indexed_seq = 3;
         DROP TRIGGER command_receipts_historical_insert;
         DROP TRIGGER command_receipts_historical_update;
         DROP TABLE historical_command_receipts;
         UPDATE journal_schema SET version = 26;
         PRAGMA user_version = 26;",
        )
        .expect("construct genuine schema26 authority");
    journal
}

fn insert_retired(connection: &Connection, id: &str) {
    let request = json!({
        "v": 1, "id": id, "epoch": EPOCH, "kind": "gate::findings", "expected": [],
        "body": {"report": {"evidence": "historical payload stays byte-identical"}},
    });
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(1),
                last: seq(3),
            }),
            body: json!({"created": 1}),
        }),
    };
    connection.execute(
        "INSERT INTO command_receipts VALUES (?1, 'operator', 'gate::findings', ?2, ?3, ?4, 1, 3, NULL, ?5)",
        params![id, serde_json::to_vec(&request).expect("request encodes"), b"[]".as_slice(), wire::encode(&response).expect("response encodes"), EPOCH],
    ).expect("insert retired receipt");
}

fn receipt(connection: &Connection, table: &str, id: &str) -> Vec<rusqlite::types::Value> {
    connection
        .query_row(
            &format!("SELECT * FROM {table} WHERE command_id = ?1"),
            [id],
            |row| (0..10).map(|column| row.get(column)).collect(),
        )
        .expect("receipt reads")
}

fn fact_bytes(connection: &Connection) -> Vec<Vec<u8>> {
    connection
        .prepare("SELECT envelope FROM fact_envelopes ORDER BY seq")
        .expect("facts query")
        .query_map([], |row| row.get(0))
        .expect("facts read")
        .collect::<Result<_, _>>()
        .expect("fact bytes")
}
