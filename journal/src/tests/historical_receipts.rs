use super::*;

const RETIRED: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
const AUTHORIZATION: &str = "018f82d4-6f59-7d06-92e3-bce913116304";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";

#[test]
fn archived_identity_cannot_replay_or_commit_as_a_new_live_command() {
    let scratch = Scratch::new();
    let path = scratch.path().join("identity.sqlite3");
    let current = historical_fixture(&path);
    insert_retired(&current.connection, RETIRED);
    drop(current);
    let journal = Journal::open(&path).expect("current authority reopens");
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(RETIRED),
        epoch: uuid(EPOCH),
        kind: CommandKind::BoardRead,
        expected: Vec::new(),
        body: json!({"scope": "all"}),
    }
    .admit(
        &wire::Accepted {
            wire: WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: uuid(EPOCH),
            release: "1.2.3".parse().expect("fixture release"),
        },
        &wire::principal::PrincipalRoles::default(),
    )
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
        let current = historical_fixture(&path);
        insert_retired(&current.connection, RETIRED);
        drop(current);
        let journal = Journal::open(&path).expect("current authority reopens");
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

#[test]
fn retired_in_place_receipts_verify_but_unregistered_kinds_do_not() {
    // Witness bytes as their writer encoded them: tag first, then fields in
    // declaration order.
    let retired_witness = |kind: &str| {
        format!(
            r#"[{{"kind":"proposal","proposal":{{"kind":"{kind}","{kind}":{{"candidate":{{"controller":"dVxHkZhQY","candidate":"dVxJC0000"}},"{kind}":"1"}}}}}}]"#
        )
    };
    for (kind, witnesses) in [
        ("preparation::authorize", "[]".to_owned()),
        ("preparation::critique", retired_witness("critique")),
        ("preparation::cover", retired_witness("coverage")),
        ("preparation::nudge", "[]".to_owned()),
    ] {
        let expected: serde_json::Value =
            serde_json::from_str(&witnesses).expect("fixture witnesses parse");
        let scratch = Scratch::new();
        let path = scratch.path().join("retired.sqlite3");
        let current = historical_fixture(&path);
        let request = retired_request(AUTHORIZATION, kind, &expected);
        assert!(
            wire::decode::<Command<serde_json::Value>>(&request).is_err(),
            "a retained {kind} receipt must not restore its command to the live registry"
        );
        insert_in_place(
            &current.connection,
            "command_receipts",
            AUTHORIZATION,
            kind,
            &request,
            witnesses.as_bytes(),
        );
        let before = receipt(&current.connection, "command_receipts", AUTHORIZATION);
        drop(current);
        let journal = Journal::open(&path).expect("retired evidence reopens");
        assert_eq!(
            journal
                .verify()
                .expect("retired evidence verifies")
                .receipts,
            2,
            "{kind}"
        );
        assert_eq!(
            receipt(&journal.connection, "command_receipts", AUTHORIZATION),
            before,
            "verification leaves retained {kind} evidence byte-identical"
        );
    }

    let scratch = Scratch::new();
    let path = scratch.path().join("unknown.sqlite3");
    let current = historical_fixture(&path);
    let unknown = retired_request(AUTHORIZATION, "preparation::never-registered", &json!([]));
    insert_in_place(
        &current.connection,
        "command_receipts",
        AUTHORIZATION,
        "preparation::never-registered",
        &unknown,
        b"[]",
    );
    drop(current);
    let journal = Journal::open(&path).expect("unknown evidence reopens");
    assert!(
        refused_violations(&journal)
            .iter()
            .any(|v| v.class == ViolationClass::Receipt),
        "a never-registered command kind must remain invalid"
    );
}

#[test]
fn a_retired_tell_receipt_verifies_in_place_with_its_exact_request() {
    let scratch = Scratch::new();
    let path = scratch.path().join("tell.sqlite3");
    let current = historical_fixture(&path);
    insert_in_place(
        &current.connection,
        "command_receipts",
        RETIRED,
        "steer::tell",
        &tell_request(),
        b"[]",
    );
    drop(current);
    let journal = Journal::open(&path).expect("tell evidence reopens");
    assert_eq!(
        journal.verify().expect("tell evidence verifies").receipts,
        2
    );
    drop(journal);

    let mut padded = tell_request();
    padded.push(b' ');
    for (table, request) in [
        ("historical_command_receipts", tell_request()),
        ("command_receipts", padded),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join("tell-corruption.sqlite3");
        let current = historical_fixture(&path);
        insert_in_place(
            &current.connection,
            table,
            RETIRED,
            "steer::tell",
            &request,
            b"[]",
        );
        drop(current);
        let journal = Journal::open(&path).expect("current authority reopens");
        assert!(
            refused_violations(&journal)
                .iter()
                .any(|v| v.class == ViolationClass::Receipt),
            "a misplaced or altered tell receipt escaped verification: {table}"
        );
    }
}

#[test]
fn authenticated_restore_keeps_and_verifies_historical_receipt_evidence() {
    use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};

    let scratch = Scratch::new();
    let authority = scratch.path().join("spicejournal.sqlite3");
    let current = historical_fixture(&authority);
    insert_retired(&current.connection, RETIRED);
    let before = receipt(&current.connection, "historical_command_receipts", RETIRED);
    drop(current);
    drop(Journal::open(&authority).expect("current authority reopens before backup"));
    let bytes = fs::read(&authority).expect("checkpointed authority reads");
    let manifest = JournalBackupManifest {
        version: JOURNAL_BACKUP_VERSION,
        wire: WireVersion::CURRENT,
        journal_schema: SCHEMA_VERSION,
        authority: "spicejournal.sqlite3".to_owned(),
        members: vec![JournalBackupMember {
            path: "spicejournal.sqlite3".to_owned(),
            mode: sill::authority::file_mode(&fs::File::open(&authority).expect("opens"))
                .expect("mode reads"),
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

fn historical_fixture(path: &Path) -> Journal {
    authority_file_fixture(path)
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
                last: seq(2),
            }),
            body: json!({"created": 1}),
        }),
    };
    connection.execute(
        "INSERT INTO historical_command_receipts VALUES (?1, 'operator', 'gate::findings', ?2, ?3, ?4, 1, 2, NULL, ?5)",
        params![id, serde_json::to_vec(&request).expect("request encodes"), b"[]".as_slice(), wire::encode(&response).expect("response encodes"), EPOCH],
    ).expect("insert historical receipt");
}

fn tell_request() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "v": 1, "id": RETIRED, "epoch": EPOCH, "kind": "steer::tell", "expected": [],
        "body": {"lane": "018f82d4-6f59-7d06-92e3-bce913116302", "message": "the parser moved"},
    }))
    .expect("request encodes")
}

fn retired_request(id: &str, kind: &str, expected: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "v": 1, "id": id, "epoch": EPOCH, "kind": kind, "expected": expected,
        "body": {"authorization": {"candidate": "dW4zAAAAA", "outputs": ["retained"]}},
    }))
    .expect("request encodes")
}

fn insert_in_place(
    connection: &Connection,
    table: &str,
    id: &str,
    kind: &str,
    request: &[u8],
    witnesses: &[u8],
) {
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(1),
                last: seq(2),
            }),
            body: json!({"key": "dVhAAAAAA"}),
        }),
    };
    connection
        .execute(
            &format!("INSERT INTO {table} VALUES (?1, 'lane:018f82d4-6f59-7d06-92e3-bce913116303', ?2, ?3, ?4, ?5, 1, 2, NULL, ?6)"),
            params![
                id,
                kind,
                request,
                witnesses,
                wire::encode(&response).expect("response encodes"),
                EPOCH
            ],
        )
        .expect("insert in-place receipt");
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
