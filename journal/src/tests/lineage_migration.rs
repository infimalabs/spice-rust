use serde_json::Value;

use super::*;

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116310";
const LINEAGE: &str = "018f82d4-6f59-7d06-92e3-bce913116311";
const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116312";

#[derive(Clone, Copy)]
enum StoredLineage {
    Missing,
    Renamed,
    Current,
    Conflict,
}

#[test]
fn schema_two_pre_lineage_rows_derive_the_lane_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("pre-lineage.sqlite3");
    schema_two_lineage_authority(&path, [StoredLineage::Missing; 3]);

    let journal = Journal::open(&path).expect("the pre-lineage authority migrates");

    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((2, SCHEMA_VERSION))
    );
    assert_eq!(
        residency_head_from(&journal.connection).topology_revision,
        0,
        "schema six derives the revision from the schema-two head position"
    );
    assert_all_carriers(&journal.connection, LANE);
}

#[test]
fn schema_two_previous_name_rows_are_renamed_without_changing_identity() {
    let scratch = Scratch::new();
    let path = scratch.path().join("team.sqlite3");
    schema_two_lineage_authority(&path, [StoredLineage::Renamed; 3]);

    let journal = Journal::open(&path).expect("the team authority migrates");

    assert_all_carriers(&journal.connection, LINEAGE);
}

#[test]
fn current_rows_cross_schema_two_without_an_alternate_rewrite() {
    let scratch = Scratch::new();
    let path = scratch.path().join("current.sqlite3");
    schema_two_lineage_authority(&path, [StoredLineage::Current; 3]);
    let before = carrier_bytes(&Connection::open(&path).expect("open current fixture"));

    let journal = Journal::open(&path).expect("the current authority migrates");

    let after = carrier_bytes(&journal.connection);
    assert_eq!(without_topology_revision(&after[0], ""), before[0]);
    assert_eq!(after[1], before[1]);
    assert_eq!(without_topology_revision(&after[2], "/body"), before[2]);
    assert_all_carriers(&journal.connection, LINEAGE);
}

#[test]
fn conflicting_names_refuse_and_roll_back_the_whole_migration() {
    let scratch = Scratch::new();
    let path = scratch.path().join("conflict.sqlite3");
    schema_two_lineage_authority(&path, [StoredLineage::Conflict; 3]);
    let before = carrier_bytes(&Connection::open(&path).expect("open conflict fixture"));

    let error = Journal::open(&path).expect_err("a conflicting rename must refuse");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 2,
            to: 3,
            table: "residency_head",
            ..
        })
    ));
    assert!(error.to_string().contains("name different lineages"));
    let connection = Connection::open(&path).expect("reopen refused authority");
    assert_eq!(version_records(&connection), (2, 2));
    assert_eq!(carrier_bytes(&connection), before);
}

#[test]
fn a_late_receipt_conflict_rolls_back_earlier_carrier_rewrites() {
    let scratch = Scratch::new();
    let path = scratch.path().join("late-conflict.sqlite3");
    schema_two_lineage_authority(
        &path,
        [
            StoredLineage::Missing,
            StoredLineage::Missing,
            StoredLineage::Conflict,
        ],
    );
    let before = carrier_bytes(&Connection::open(&path).expect("open late conflict fixture"));

    let error = Journal::open(&path).expect_err("a late conflict must refuse");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 2,
            to: 3,
            table: "command_receipts",
            ..
        })
    ));
    let connection = Connection::open(&path).expect("reopen late conflict authority");
    assert_eq!(version_records(&connection), (2, 2));
    assert_eq!(carrier_bytes(&connection), before);
}

fn schema_two_lineage_authority(path: &Path, shapes: [StoredLineage; 3]) {
    let journal = authority_file_fixture(path);
    seed_head_identity(&journal.connection);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::LaneSeated,
            json!({
                "lane": LANE,
                "lineage_id": LINEAGE,
                "capability": "018f82d4-6f59-7d06-92e3-bce913116313",
                "worktree": "/worktrees/lineage",
                "life": "drive",
                "vendor": "codex"
            }),
        ),
    );
    insert_lane_seat_receipt(&journal);
    reshape_row(
        &journal.connection,
        "SELECT body FROM residency_head WHERE singleton = 1",
        "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
        "/lanes/0",
        shapes[0],
    );
    reshape_row(
        &journal.connection,
        "SELECT envelope FROM fact_envelopes WHERE seq = 3",
        "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 3",
        "/fact",
        shapes[1],
    );
    reshape_row(
        &journal.connection,
        "SELECT request FROM command_receipts WHERE command_id = '018f82d4-6f59-7d06-92e3-bce913116312'",
        "UPDATE command_receipts SET request = ?1 WHERE command_id = '018f82d4-6f59-7d06-92e3-bce913116312'",
        "/body",
        shapes[2],
    );
    remove_head_topology_revision(&journal.connection);
    journal
        .connection
        .execute_batch(
            "DELETE FROM lease_heads;
             DELETE FROM claim_heads;
             UPDATE journal_schema SET version = 2 WHERE singleton = 1;
             PRAGMA user_version = 2;",
        )
        .expect("downgrade lineage fixture to schema 2");
    let stored = carrier_bytes(&journal.connection);
    let head: Value = serde_json::from_slice(&stored[0]).expect("decode schema-two head");
    assert!(
        head.get("topology_revision").is_none(),
        "an authentic schema-two head predates topology revision"
    );
}

fn remove_head_topology_revision(connection: &Connection) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read current residency head");
    let mut document: Value =
        serde_json::from_slice(&bytes).expect("decode current residency head");
    document
        .as_object_mut()
        .expect("residency head object")
        .remove("topology_revision")
        .expect("current fixture carries topology revision");
    connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![serde_json::to_vec(&document).expect("encode schema-two residency head")],
        )
        .expect("store schema-two residency head");
}

fn seed_head_identity(connection: &Connection) {
    let mut head = residency_head();
    head.lanes[0].lane = uuid(LANE);
    head.lanes[0].lineage_id = uuid(LINEAGE);
    connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![wire::encode(&head).expect("encode residency fixture")],
        )
        .expect("store residency fixture identity");
}

fn insert_lane_seat_receipt(journal: &Journal) {
    let id = uuid(COMMAND);
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116314");
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::LaneSeat,
        expected: Vec::new(),
        body: json!({
            "lane": LANE,
            "lineage_id": LINEAGE,
            "worktree": "/worktrees/lineage",
            "life": "drive",
            "vendor": "codex"
        }),
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
            body: json!({"from": "vacant", "lane": LANE, "to": "seated"}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'lane::seat', ?2, ?3, ?4, 3, 3, NULL, ?5)",
            params![
                id.to_string(),
                request
                    .canonical_json()
                    .expect("canonical request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
                wire::encode(&response).expect("encode response"),
                epoch.to_string(),
            ],
        )
        .expect("insert lane seat receipt");
}

fn reshape_row(
    connection: &Connection,
    select: &str,
    update: &str,
    pointer: &str,
    shape: StoredLineage,
) {
    if matches!(shape, StoredLineage::Current) {
        return;
    }
    let bytes: Vec<u8> = connection
        .query_row(select, [], |row| row.get(0))
        .expect("read current lineage carrier");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode lineage carrier");
    let object = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("lineage carrier object");
    let current = object
        .remove("lineage_id")
        .expect("current fixture has lineage_id");
    match shape {
        StoredLineage::Missing => {}
        StoredLineage::Renamed => {
            object.insert("team".to_owned(), current);
        }
        StoredLineage::Current => unreachable!("current rows return before reshaping"),
        StoredLineage::Conflict => {
            object.insert("lineage_id".to_owned(), current);
            object.insert("team".to_owned(), Value::String(LANE.to_owned()));
        }
    }
    connection
        .execute(
            update,
            params![serde_json::to_vec(&document).expect("encode old carrier")],
        )
        .expect("store old lineage carrier");
}

fn assert_all_carriers(connection: &Connection, expected: &str) {
    let bytes = carrier_bytes(connection);
    let head: ResidencyHead = wire::decode(&bytes[0]).expect("migrated head decodes");
    assert_eq!(head.lanes[0].lineage_id, uuid(expected));
    let fact: Envelope<AnyFact> = wire::decode(&bytes[1]).expect("migrated fact decodes");
    assert_eq!(fact.fact.kind(), FactKind::LaneSeated);
    let request: Command<wire::intent::LaneSeat> =
        wire::decode(&bytes[2]).expect("migrated request decodes");
    assert_eq!(request.kind, CommandKind::LaneSeat);
    assert_eq!(request.body.lineage_id, uuid(expected));
    assert_eq!(
        wire::encode(&head).expect("canonical head"),
        bytes[0],
        "migrated head must use canonical wire bytes"
    );
    assert_eq!(
        wire::encode(&fact).expect("canonical fact"),
        bytes[1],
        "migrated fact must use canonical wire bytes"
    );
    assert_eq!(
        request
            .canonical_json()
            .expect("canonical request")
            .into_bytes(),
        bytes[2],
        "migrated request must use canonical command bytes"
    );
    for (index, (document, pointer)) in [
        (&bytes[0], "/lanes/0"),
        (&bytes[1], "/fact"),
        (&bytes[2], "/body"),
    ]
    .into_iter()
    .enumerate()
    {
        let value: Value = serde_json::from_slice(document).expect("decode migrated carrier");
        let object = value
            .pointer(pointer)
            .and_then(Value::as_object)
            .expect("migrated lineage object");
        assert_eq!(
            object.get("lineage_id").and_then(Value::as_str),
            Some(expected)
        );
        assert!(
            !object.contains_key("team"),
            "carrier {index} retained team"
        );
    }
}

fn carrier_bytes(connection: &Connection) -> Vec<Vec<u8>> {
    [
        "SELECT body FROM residency_head WHERE singleton = 1",
        "SELECT envelope FROM fact_envelopes WHERE seq = 3",
        "SELECT request FROM command_receipts WHERE command_id = '018f82d4-6f59-7d06-92e3-bce913116312'",
    ]
    .into_iter()
    .map(|sql| {
        connection
            .query_row(sql, [], |row| row.get(0))
            .expect("read lineage carrier")
    })
        .collect()
}

fn without_topology_revision(bytes: &[u8], pointer: &str) -> Vec<u8> {
    let mut document: Value = serde_json::from_slice(bytes).expect("decode migrated carrier");
    let object = if pointer.is_empty() {
        document.as_object_mut()
    } else {
        document.pointer_mut(pointer).and_then(Value::as_object_mut)
    }
    .expect("topology-bearing carrier object");
    object
        .remove("topology_revision")
        .expect("schema-six carrier carries topology revision");
    serde_json::to_vec(&document).expect("encode pre-topology request")
}

fn residency_head_from(connection: &Connection) -> ResidencyHead {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read migrated residency head");
    wire::decode(&bytes).expect("migrated residency head decodes")
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
