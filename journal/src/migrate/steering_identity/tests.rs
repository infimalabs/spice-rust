use rusqlite::{Connection, params};

use super::*;

const ROOT: &str = "1kJK6N3R";
const CHILD: &str = "1kJK6M0V";
const GRANDCHILD: &str = "1kJK6M1W";
const GREAT_GRANDCHILD: &str = "1kJK6M2X";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const FIRST: &str = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d1";
const SECOND: &str = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d2";
const THIRD: &str = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d3";
const FOURTH: &str = "a0ae31a7-eb68-463a-b3c0-272bfeaaa5d4";
const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116399";
const EPOCH: &str = "e2e9f1f2-19b2-4ddc-83fa-340b02dee2c4";

#[test]
fn one_old_rung_and_its_terminal_fact_fold_onto_the_root() {
    let mut connection = fixture();
    insert(&connection, 1, SENT, &sent());
    insert(&connection, 2, RESENT, &resent(SECOND, CHILD, ROOT, 1));
    insert(&connection, 3, "steer::acked", &acked(SECOND, CHILD));
    insert_dispose_receipt(&connection);

    let transaction = connection.transaction().expect("migration transaction");
    super::super::advance(&transaction).expect("old steering graph migrates");
    transaction.commit().expect("migration commits");

    assert_eq!(version(&connection), crate::SCHEMA_VERSION);
    let escalated = stored_fact(&connection, 2);
    assert_eq!(escalated["kind"], ESCALATED);
    assert_eq!(escalated["directive"], FIRST);
    assert_eq!(escalated["key"], ROOT);
    assert_eq!(escalated["stage"], "reminded");
    assert!(escalated.get("ancestor").is_none());
    let terminal = stored_fact(&connection, 3);
    assert_eq!(terminal["directive"], FIRST);
    assert_eq!(terminal["key"], ROOT);
    let (request, witnesses, response): (Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT request, evaluated_witnesses, response FROM command_receipts \
             WHERE command_id = ?1",
            [COMMAND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("migrated disposition receipt reads");
    let command: wire::Command<Value> =
        wire::decode(&request).expect("current disposition request");
    assert_eq!(command.body["token"], ROOT);
    assert_eq!(
        command.expected,
        [wire::Witness::Directive {
            steer: FIRST.parse().expect("root UUID"),
            disposition: wire::Disposition::Live,
        }]
    );
    let evaluated: Vec<wire::Witness> =
        wire::decode(&witnesses).expect("current disposition witnesses");
    assert_eq!(evaluated, command.expected);
    let response: wire::Response<Value> =
        wire::decode(&response).expect("current disposition response");
    let wire::Outcome::Reply(reply) = response.outcome else {
        panic!("disposition receipt remains successful");
    };
    assert_eq!(reply.body["directive"], FIRST);
    assert_eq!(reply.body["key"], ROOT);
    for seq in 1..=3 {
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
                [seq],
                |row| row.get(0),
            )
            .expect("migrated bytes");
        wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes)
            .expect("every migrated fact is current wire");
    }
}

#[test]
fn all_three_old_stages_preserve_their_positions_instants_and_one_terminal_identity() {
    let mut connection = fixture();
    insert(&connection, 1, SENT, &sent());
    insert(&connection, 2, RESENT, &resent(SECOND, CHILD, ROOT, 1));
    insert(&connection, 3, RESENT, &resent(THIRD, GRANDCHILD, CHILD, 2));
    insert(
        &connection,
        4,
        RESENT,
        &resent(FOURTH, GREAT_GRANDCHILD, GRANDCHILD, 3),
    );
    insert(
        &connection,
        5,
        "steer::acked",
        &acked_at(5, FOURTH, GREAT_GRANDCHILD),
    );

    let transaction = connection.transaction().expect("migration transaction");
    super::super::advance(&transaction).expect("complete ladder migrates");
    transaction.commit().expect("migration commits");

    for (seq, stage) in [(2, "reminded"), (3, "escalated"), (4, "overdue")] {
        let fact = stored_fact(&connection, seq);
        assert_eq!(fact["kind"], ESCALATED);
        assert_eq!(fact["directive"], FIRST);
        assert_eq!(fact["key"], ROOT);
        assert_eq!(fact["stage"], stage);
        let envelope = stored_envelope(&connection, seq);
        assert_eq!(envelope["at"], format!("2026-08-06T03:00:0{seq}.000Z"));
    }
    let terminal = stored_fact(&connection, 5);
    assert_eq!(terminal["directive"], FIRST);
    assert_eq!(terminal["key"], ROOT);
}

#[test]
fn old_recovery_receipt_loses_its_lineage_witness_and_rebinds_to_the_root() {
    let mut connection = fixture();
    insert(&connection, 1, SENT, &sent());
    insert(&connection, 2, RESENT, &resent(SECOND, CHILD, ROOT, 1));
    insert(&connection, 3, "steer::lapsed", &lapsed(3, CHILD));
    insert(
        &connection,
        4,
        "steer::recovered",
        &recovered(4, SECOND, CHILD),
    );
    insert_recovery_receipt(&connection);

    let transaction = connection.transaction().expect("migration transaction");
    super::super::advance(&transaction).expect("recovery receipt migrates");
    transaction.commit().expect("migration commits");

    let (kind, request, witnesses, response): (String, Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT command_kind, request, evaluated_witnesses, response \
             FROM command_receipts WHERE command_id = ?1",
            [COMMAND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("migrated receipt reads");
    assert_eq!(kind, "steer::recover");
    let command: wire::Command<Value> = wire::decode(&request).expect("current request wire");
    assert_eq!(command.kind, wire::CommandKind::SteerRecover);
    assert_eq!(
        command.expected,
        [wire::Witness::Recoverable {
            steer: FIRST.parse().expect("root UUID"),
        }]
    );
    assert_eq!(command.body["directive"], FIRST);
    command.witnessed().expect("migrated command obeys its law");
    let evaluated: Vec<wire::Witness> =
        wire::decode(&witnesses).expect("current evaluated witnesses");
    assert_eq!(evaluated, command.expected);
    let response: wire::Response<Value> = wire::decode(&response).expect("current response wire");
    let wire::Outcome::Reply(reply) = response.outcome else {
        panic!("receipt response remains successful");
    };
    assert_eq!(reply.body["directive"], FIRST);
    assert_eq!(reply.body["key"], ROOT);
}

#[test]
fn refused_old_recovery_receipt_is_renamed_without_a_committed_fact_range() {
    let mut connection = fixture();
    insert(&connection, 1, SENT, &sent());
    insert(&connection, 2, RESENT, &resent(SECOND, CHILD, ROOT, 1));
    insert_refused_recovery_receipt(&connection);

    let transaction = connection.transaction().expect("migration transaction");
    super::super::advance(&transaction).expect("refused recovery receipt migrates");
    transaction.commit().expect("migration commits");

    let (kind, request, witnesses, response): (String, Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT command_kind, request, evaluated_witnesses, response \
             FROM command_receipts WHERE command_id = ?1",
            [COMMAND],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("migrated refused receipt reads");
    assert_eq!(kind, "steer::recover");
    let command: wire::Command<Value> = wire::decode(&request).expect("current request wire");
    assert_eq!(command.kind, wire::CommandKind::SteerRecover);
    assert_eq!(command.body["directive"], FIRST);
    assert_eq!(
        command.expected,
        [wire::Witness::Recoverable {
            steer: FIRST.parse().expect("root UUID"),
        }]
    );
    command.witnessed().expect("migrated command obeys its law");
    let evaluated: Vec<wire::Witness> =
        wire::decode(&witnesses).expect("current evaluated witnesses");
    assert_eq!(evaluated, command.expected);
    let response: wire::Response<Value> = wire::decode(&response).expect("current response wire");
    let wire::Outcome::Refusal(refusal) = response.outcome else {
        panic!("receipt remains a refusal");
    };
    assert_eq!(
        refusal.reason(),
        "steer::recovery-live",
        "migration preserves the refusal code through its semantic accessor"
    );
    assert_eq!(
        refusal.explanation(),
        "recover only a lapsed directive",
        "the historical remedy becomes non-runnable diagnostic detail"
    );
}

#[test]
fn contradictory_old_shapes_each_refuse_without_mutating_any_row() {
    let other_lane = "018f82d4-6f59-7d06-92e3-bce913116303";
    let cases = [
        (
            vec![(2, RESENT, resent(SECOND, CHILD, ROOT, 1))],
            "ancestor does not name an earlier steering node",
        ),
        (
            vec![(1, SENT, sent().replace(ROOT, "bad-key!"))],
            "eight-character steering key",
        ),
        (
            vec![
                (1, SENT, sent()),
                (
                    2,
                    RESENT,
                    resent(SECOND, CHILD, ROOT, 1).replace(LANE, other_lane),
                ),
            ],
            "changes lane or does not advance exactly one",
        ),
        (
            vec![
                (1, SENT, sent()),
                (2, RESENT, resent(SECOND, CHILD, ROOT, 2)),
            ],
            "changes lane or does not advance exactly one",
        ),
        (
            vec![
                (1, SENT, sent()),
                (
                    2,
                    RESENT,
                    resent(SECOND, CHILD, ROOT, 1)
                        .replace("2026-08-06T03:00:02.000Z", "2026-08-06T03:00:00.000Z"),
                ),
            ],
            "predates the ancestor",
        ),
        (
            vec![
                (1, SENT, sent()),
                (2, RESENT, resent(SECOND, CHILD, ROOT, 1)),
                (3, "steer::acked", acked_at(3, SECOND, ROOT)),
            ],
            "key and directive identity name different steering nodes",
        ),
        (
            vec![
                (1, SENT, sent()),
                (2, RESENT, resent(SECOND, CHILD, ROOT, 1)),
                (3, "steer::lapsed", lapsed(3, CHILD)),
                (
                    4,
                    "steer::recovered",
                    recovered(4, SECOND, CHILD)
                        .replace("2026-08-08T03:00:01.000Z", "2026-08-07T03:00:01.000Z"),
                ),
            ],
            "expiry does not follow its lapse",
        ),
    ];
    for (rows, expected) in cases {
        assert_migration_refuses_unchanged(rows, expected);
    }
}

#[test]
fn a_fork_refuses_the_whole_migration_without_rewriting_a_row() {
    let mut connection = fixture();
    insert(&connection, 1, SENT, &sent());
    insert(&connection, 2, RESENT, &resent(SECOND, CHILD, ROOT, 1));
    insert(
        &connection,
        3,
        RESENT,
        &resent("a0ae31a7-eb68-463a-b3c0-272bfeaaa5d3", "1kJK7Sqy", ROOT, 1),
    );
    let before: Vec<Vec<u8>> = envelopes(&connection);

    let transaction = connection.transaction().expect("migration transaction");
    let error = super::super::advance(&transaction).expect_err("fork refuses");
    assert!(error.to_string().contains("forks an ancestor"));
    transaction.rollback().expect("failed migration rolls back");

    assert_eq!(version(&connection), 3);
    assert_eq!(envelopes(&connection), before);
    let kinds: Vec<String> = connection
        .prepare("SELECT kind FROM fact_envelopes ORDER BY seq")
        .expect("kind query")
        .query_map([], |row| row.get(0))
        .expect("kind rows")
        .collect::<Result<_, _>>()
        .expect("kind values");
    assert_eq!(kinds, [SENT, RESENT, RESENT]);
}

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("fixture SQLite");
    connection
        .execute_batch(super::super::BASELINE_SQL)
        .expect("current baseline");
    connection
        .execute_batch(
            "DROP TABLE claim_index;
             DROP TABLE steering_index;
             DROP TABLE steering_misses;
             DROP TABLE steering_heads;",
        )
        .expect("remove post-schema-6 indexes from schema-3 fixture");
    crate::tests::downgrade_task_identity_to_five(&connection);
    connection
        .execute(
            "UPDATE journal_schema SET version = 3 WHERE singleton = 1",
            [],
        )
        .expect("downgrade schema row");
    connection
        .pragma_update(None, "user_version", 3)
        .expect("downgrade pragma");
    connection
}

fn insert(connection: &Connection, seq: i64, kind: &str, envelope: &str) {
    connection
        .execute(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) \
             VALUES (?1, 1, ?2, 'operator', ?3, NULL, ?4, ?5)",
            params![
                seq,
                format!("2026-08-06T03:00:0{seq}.000Z"),
                format!("resident:{EPOCH}"),
                kind,
                envelope.as_bytes(),
            ],
        )
        .expect("old fact inserts");
}

fn sent() -> String {
    format!(
        concat!(
            r#"{{"v":1,"seq":1,"at":"2026-08-06T03:00:01.000Z","actor":"operator","origin":"resident:{}","fact":{{"kind":"steer::sent","directive":"{}","key":"{}","lane":"{}","text":"hold","expires":"2026-08-07T03:00:01.000Z"}}}}"#
        ),
        EPOCH, FIRST, ROOT, LANE
    )
}

fn resent(directive: &str, key: &str, ancestor: &str, stage: u32) -> String {
    let seq = i64::from(stage) + 1;
    format!(
        r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"operator","origin":"resident:{}","fact":{{"kind":"steer::resent","directive":"{}","key":"{}","ancestor":"{}","lane":"{}","escalation":{}}}}}"#,
        seq, seq, EPOCH, directive, key, ancestor, LANE, stage
    )
}

fn acked(directive: &str, key: &str) -> String {
    acked_at(3, directive, key)
}

fn acked_at(seq: i64, directive: &str, key: &str) -> String {
    format!(
        r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"operator","origin":"resident:{}","fact":{{"kind":"steer::acked","directive":"{}","key":"{}","lane":"{}","epoch":"{}","locus":{{"member":1,"offset":2,"length":3,"event":0,"line":1,"column":1}}}}}}"#,
        seq, seq, EPOCH, directive, key, LANE, EPOCH
    )
}

fn lapsed(seq: i64, key: &str) -> String {
    format!(
        r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"spice","origin":"resident:{}","fact":{{"kind":"steer::lapsed","key":"{}","lane":"{}","expires":"2026-08-07T03:00:01.000Z"}}}}"#,
        seq, seq, EPOCH, key, LANE
    )
}

fn recovered(seq: i64, directive: &str, key: &str) -> String {
    format!(
        r#"{{"v":1,"seq":{},"at":"2026-08-06T03:00:0{}.000Z","actor":"operator","origin":"resident:{}","fact":{{"kind":"steer::recovered","directive":"{}","key":"{}","lane":"{}","lapsed_at":"2026-08-07T03:00:01.000Z","expires":"2026-08-08T03:00:01.000Z"}}}}"#,
        seq, seq, EPOCH, directive, key, LANE
    )
}

fn insert_recovery_receipt(connection: &Connection) {
    let expected = serde_json::json!([
        {"kind": "directive", "steer": SECOND, "disposition": "lapsed"},
        {"kind": "lineage", "steer": SECOND}
    ]);
    let request = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "kind": "steer::resend",
        "expected": expected,
        "body": {"directive": SECOND}
    });
    let response = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "outcome": {
            "kind": "reply",
            "seq": {"first": 4, "last": 4},
            "body": {"directive": SECOND, "key": CHILD, "disposition": "live"}
        }
    });
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'steer::resend', ?2, ?3, ?4, 4, 4, NULL, ?5)",
            params![
                COMMAND,
                serde_json::to_vec(&request).expect("old request encodes"),
                serde_json::to_vec(&expected).expect("old witnesses encode"),
                serde_json::to_vec(&response).expect("old response encodes"),
                EPOCH,
            ],
        )
        .expect("old recovery receipt inserts");
}

fn insert_dispose_receipt(connection: &Connection) {
    let expected = serde_json::json!([
        {"kind": "directive", "steer": SECOND, "disposition": "live"}
    ]);
    let request = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "kind": "steer::dispose",
        "expected": expected,
        "body": {"token": CHILD, "disposition": "acked"}
    });
    let response = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "outcome": {
            "kind": "reply",
            "seq": {"first": 3, "last": 3},
            "body": {"directive": SECOND, "key": CHILD, "disposition": "acked"}
        }
    });
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'lane:018f82d4-6f59-7d06-92e3-bce913116302', \
             'steer::dispose', ?2, ?3, ?4, 3, 3, NULL, ?5)",
            params![
                COMMAND,
                serde_json::to_vec(&request).expect("old request encodes"),
                serde_json::to_vec(&expected).expect("old witnesses encode"),
                serde_json::to_vec(&response).expect("old response encodes"),
                EPOCH,
            ],
        )
        .expect("old disposition receipt inserts");
}

fn insert_refused_recovery_receipt(connection: &Connection) {
    let expected = serde_json::json!([
        {"kind": "directive", "steer": SECOND, "disposition": "lapsed"},
        {"kind": "lineage", "steer": SECOND}
    ]);
    let request = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "kind": "steer::resend",
        "expected": expected,
        "body": {"directive": SECOND}
    });
    let response = serde_json::json!({
        "v": 1,
        "id": COMMAND,
        "epoch": EPOCH,
        "outcome": {
            "kind": "refusal",
            "reason": "steer::recovery-live",
            "expected": "lapsed",
            "observed": "live",
            "remedy": "recover only a lapsed directive"
        }
    });
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'steer::resend', ?2, ?3, ?4, NULL, NULL, 2, ?5)",
            params![
                COMMAND,
                serde_json::to_vec(&request).expect("old request encodes"),
                serde_json::to_vec(&expected).expect("old witnesses encode"),
                serde_json::to_vec(&response).expect("old response encodes"),
                EPOCH,
            ],
        )
        .expect("old refused recovery receipt inserts");
}

fn assert_migration_refuses_unchanged(rows: Vec<(i64, &'static str, String)>, expected: &str) {
    let mut connection = fixture();
    for (seq, kind, envelope) in rows {
        insert(&connection, seq, kind, &envelope);
    }
    let before = envelopes(&connection);
    let transaction = connection.transaction().expect("migration transaction");
    let error = super::super::advance(&transaction).expect_err("contradiction refuses");
    assert!(
        error.to_string().contains(expected),
        "expected {expected:?} in {error}"
    );
    transaction.rollback().expect("failed migration rolls back");
    assert_eq!(version(&connection), 3);
    assert_eq!(envelopes(&connection), before);
}

fn stored_fact(connection: &Connection, seq: i64) -> Value {
    stored_envelope(connection, seq)["fact"].clone()
}

fn stored_envelope(connection: &Connection, seq: i64) -> Value {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq],
            |row| row.get(0),
        )
        .expect("stored envelope");
    serde_json::from_slice::<Value>(&bytes).expect("stored JSON")
}

fn version(connection: &Connection) -> u32 {
    connection
        .query_row(
            "SELECT version FROM journal_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("schema version")
}

fn envelopes(connection: &Connection) -> Vec<Vec<u8>> {
    connection
        .prepare("SELECT envelope FROM fact_envelopes ORDER BY seq")
        .expect("envelope query")
        .query_map([], |row| row.get(0))
        .expect("envelope rows")
        .collect::<Result<_, _>>()
        .expect("envelope values")
}
