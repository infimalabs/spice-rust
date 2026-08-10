use serde::de::DeserializeOwned;
use serde_json::Value;

use super::*;

const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116330";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116331";
const OTHER: &str = "018f82d4-6f59-7d06-92e3-bce913116332";
const LINEAGE: &str = "018f82d4-6f59-7d06-92e3-bce913116333";
const OCCUPANT: &str = "018f82d4-6f59-7d06-92e3-bce913116334";

const RECEIPTS: &[(CommandKind, &str, u64)] = &[
    (
        CommandKind::LaneSeat,
        "018f82d4-6f59-7d06-92e3-bce913116340",
        3,
    ),
    (
        CommandKind::LaneArrange,
        "018f82d4-6f59-7d06-92e3-bce913116341",
        4,
    ),
    (
        CommandKind::LineageConfigure,
        "018f82d4-6f59-7d06-92e3-bce913116342",
        5,
    ),
    (
        CommandKind::LineageArrange,
        "018f82d4-6f59-7d06-92e3-bce913116343",
        6,
    ),
    (
        CommandKind::LaneState,
        "018f82d4-6f59-7d06-92e3-bce913116344",
        7,
    ),
];

#[derive(Clone, Copy)]
enum StoredRevision {
    Missing,
    Current,
    Mismatched,
}

#[test]
fn schema_five_derives_one_revision_authority_from_topology_facts() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-five-topology.sqlite3");
    schema_five_topology_authority(&path, StoredRevision::Missing);

    let mut journal = Journal::open(&path).expect("schema-five topology authority migrates");

    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((5, SCHEMA_VERSION))
    );
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    assert_eq!(
        topology_task_key_column(&journal.connection),
        "stamp",
        "the combined schema-five crossing also contracts task identity"
    );
    assert_eq!(residency(&journal.connection).topology_revision, 4);
    assert_eq!(
        request::<wire::intent::LaneSeat>(&journal.connection, CommandKind::LaneSeat)
            .body
            .topology_revision,
        0
    );
    assert_eq!(
        request::<wire::intent::LaneArrange>(&journal.connection, CommandKind::LaneArrange)
            .body
            .topology_revision,
        1
    );
    assert_eq!(
        request::<wire::intent::LineageConfigure>(
            &journal.connection,
            CommandKind::LineageConfigure,
        )
        .body
        .topology_revision,
        2
    );
    assert_eq!(
        request::<wire::intent::LineageArrange>(&journal.connection, CommandKind::LineageArrange,)
            .body
            .topology_revision,
        3
    );
    assert_eq!(
        response::<wire::intent::LaneChangeAnswer>(&journal.connection, CommandKind::LaneSeat)
            .topology_revision,
        1
    );
    assert_eq!(
        response::<wire::intent::LaneAnswer>(&journal.connection, CommandKind::LaneArrange)
            .topology_revision,
        2
    );
    assert_eq!(
        response::<wire::intent::LaneAnswer>(&journal.connection, CommandKind::LineageConfigure,)
            .topology_revision,
        3
    );
    assert_eq!(
        response::<wire::intent::LaneAnswer>(&journal.connection, CommandKind::LineageArrange)
            .topology_revision,
        4
    );
    assert_eq!(
        response::<wire::intent::LaneChangeAnswer>(&journal.connection, CommandKind::LaneState)
            .topology_revision,
        4,
        "lifecycle-only receipts retain the current topology revision"
    );
    admit_empty_steering_fold(&mut journal);
    journal.verify().expect("migrated authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("migrated topology authority reopens");
    assert_eq!(
        reopened.migration(),
        None,
        "the migration runs exactly once"
    );
}

#[test]
fn exact_schema_five_revisions_cross_without_an_alternate_rewrite() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-five-current-topology.sqlite3");
    schema_five_topology_authority(&path, StoredRevision::Current);
    let before = topology_carriers(&Connection::open(&path).expect("open exact fixture"));

    let mut journal = Journal::open(&path).expect("exact topology authority migrates");

    assert_eq!(topology_carriers(&journal.connection), before);
    admit_empty_steering_fold(&mut journal);
    journal.verify().expect("exact migrated authority verifies");
}

#[test]
fn a_mismatched_schema_five_revision_refuses_and_rolls_back() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("schema-five-mismatched-topology.sqlite3");
    schema_five_topology_authority(&path, StoredRevision::Mismatched);
    let before = topology_carriers(&Connection::open(&path).expect("open mismatch fixture"));

    let error = Journal::open(&path).expect_err("mismatched topology authority must refuse");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 5,
            to: 6,
            table: "residency_head",
            ..
        })
    ));
    assert!(error.to_string().contains("derived revision 4"));
    let connection = Connection::open(&path).expect("reopen refused authority");
    assert_eq!(version_records(&connection), (5, 5));
    assert_eq!(topology_carriers(&connection), before);
}

fn schema_five_topology_authority(path: &Path, stored: StoredRevision) {
    let journal = authority_file_fixture(path);
    downgrade_task_identity_to_five(&journal.connection);
    insert_topology_facts(&journal);
    insert_lane_receipts(&journal);
    let mut head = residency_head();
    head.epoch = uuid(EPOCH);
    head.topology_revision = 4;
    head.lanes[0].lane = uuid(LANE);
    head.lanes[0].lineage_id = uuid(LINEAGE);
    journal
        .connection
        .execute(
            "UPDATE residency_head SET head_seq = 7, body = ?1 WHERE singleton = 1",
            params![wire::encode(&head).expect("encode topology head")],
        )
        .expect("advance topology head");
    journal
        .connection
        .execute("UPDATE cursor_head SET seq = 7 WHERE singleton = 1", [])
        .expect("advance topology cursor");

    match stored {
        StoredRevision::Missing => remove_all_revisions(&journal.connection),
        StoredRevision::Current => {}
        StoredRevision::Mismatched => set_head_revision(&journal.connection, 99),
    }
    drop_steering_index(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 5 WHERE singleton = 1;
             PRAGMA user_version = 5;",
        )
        .expect("downgrade topology fixture to schema 5");
}

fn topology_task_key_column(connection: &Connection) -> String {
    connection
        .query_row(
            "SELECT name FROM pragma_table_info('task_heads') WHERE pk = 1",
            [],
            |row| row.get(0),
        )
        .expect("read task head primary key after combined crossing")
}

fn insert_topology_facts(journal: &Journal) {
    for envelope in [
        fact(
            3,
            Some(2),
            FactKind::LaneSeated,
            json!({
                "lane": LANE,
                "lineage_id": LINEAGE,
                "capability": OTHER,
                "worktree": "/worktrees/topology",
                "life": "drive",
                "vendor": "codex"
            }),
        ),
        fact(
            4,
            Some(3),
            FactKind::LaneArranged,
            json!({
                "lane": LANE,
                "from_lineage_id": LINEAGE,
                "lineage_id": LINEAGE,
                "before": OTHER
            }),
        ),
        fact(
            5,
            Some(4),
            FactKind::LineageConfigured,
            json!({
                "lineage_id": LINEAGE,
                "from": "drive",
                "life": "drain"
            }),
        ),
        fact(
            6,
            Some(5),
            FactKind::LineageArranged,
            json!({
                "from": [{"lineage_id": LINEAGE, "lanes": [LANE]}],
                "lineages": [{"lineage_id": LINEAGE, "lanes": [LANE]}]
            }),
        ),
        fact(
            7,
            Some(6),
            FactKind::LaneDrained,
            json!({"lane": LANE, "occupant": OCCUPANT}),
        ),
    ] {
        insert_fact(journal, envelope);
    }
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES
             (3, 'task:JOURNAL-a1', 2, 1, 2),
             (4, 'task:JOURNAL-a1', 3, 1, 3),
             (5, 'task:JOURNAL-a1', 4, 1, 4),
             (6, 'task:JOURNAL-a1', 5, 1, 5),
             (7, 'task:JOURNAL-a1', 6, 1, 6);",
        )
        .expect("index topology facts");
}

fn insert_lane_receipts(journal: &Journal) {
    for (kind, command, seq) in RECEIPTS {
        let revision = seq - 3;
        let request_body = match kind {
            CommandKind::LaneSeat => json!({
                "topology_revision": revision,
                "lane": LANE,
                "lineage_id": LINEAGE,
                "worktree": "/worktrees/topology",
                "life": "drive",
                "vendor": "codex"
            }),
            CommandKind::LaneArrange => json!({
                "topology_revision": revision,
                "lane": LANE,
                "before": OTHER
            }),
            CommandKind::LineageConfigure => json!({
                "topology_revision": revision,
                "lineage_id": LINEAGE,
                "life": "drain"
            }),
            CommandKind::LineageArrange => json!({
                "topology_revision": revision,
                "lineages": [{"lineage_id": LINEAGE, "lanes": [LANE]}]
            }),
            CommandKind::LaneState => json!({"lane": LANE, "action": "drain"}),
            _ => unreachable!("fixture names only lane receipt kinds"),
        };
        let response_body = match kind {
            CommandKind::LaneSeat => json!({
                "lane": LANE,
                "topology_revision": 1,
                "from": "absent",
                "to": "occupied"
            }),
            CommandKind::LaneState => json!({
                "lane": LANE,
                "topology_revision": 4,
                "from": "occupied",
                "to": "vacant"
            }),
            _ => lane_answer(seq - 2),
        };
        insert_lane_receipt(journal, *kind, command, *seq, request_body, response_body);
    }
}

fn insert_lane_receipt(
    journal: &Journal,
    kind: CommandKind,
    command: &str,
    seq: u64,
    body: Value,
    answer: Value,
) {
    let id = uuid(command);
    let epoch = uuid(EPOCH);
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind,
        expected: Vec::new(),
        body,
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(seq).expect("first seq"),
                last: Seq::new(seq).expect("last seq"),
            }),
            body: answer,
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', ?2, ?3, ?4, ?5, ?6, ?6, NULL, ?7)",
            params![
                id.to_string(),
                kind.as_str(),
                request
                    .canonical_json()
                    .expect("canonical lane request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
                wire::encode(&response).expect("encode lane response"),
                i64::try_from(seq).expect("fixture seq fits SQLite"),
                epoch.to_string(),
            ],
        )
        .expect("insert lane receipt");
}

fn lane_answer(topology_revision: u64) -> Value {
    json!({
        "epoch": EPOCH,
        "state": "seated",
        "topology_revision": topology_revision,
        "lanes": [{
            "lane": LANE,
            "lineage_id": LINEAGE,
            "worktree": "/worktrees/topology",
            "life": "drive",
            "vendor": "codex",
            "lineage": [],
            "bound": false,
            "disposition": "vacant",
            "liveness": "vacant"
        }],
        "subscriptions": [{"lineage_id": LINEAGE, "policy": "drive"}]
    })
}

fn remove_all_revisions(connection: &Connection) {
    rewrite(
        connection,
        "SELECT body FROM residency_head WHERE singleton = 1",
        "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
        "",
        None,
    );
    for (kind, command, _) in RECEIPTS {
        if !matches!(kind, CommandKind::LaneState) {
            rewrite(
                connection,
                &format!("SELECT request FROM command_receipts WHERE command_id = '{command}'"),
                &format!("UPDATE command_receipts SET request = ?1 WHERE command_id = '{command}'"),
                "/body",
                None,
            );
        }
        rewrite(
            connection,
            &format!("SELECT response FROM command_receipts WHERE command_id = '{command}'"),
            &format!("UPDATE command_receipts SET response = ?1 WHERE command_id = '{command}'"),
            "/outcome/body",
            None,
        );
    }
}

fn set_head_revision(connection: &Connection, revision: u64) {
    rewrite(
        connection,
        "SELECT body FROM residency_head WHERE singleton = 1",
        "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
        "",
        Some(revision),
    );
}

fn rewrite(
    connection: &Connection,
    select: &str,
    update: &str,
    pointer: &str,
    revision: Option<u64>,
) {
    let bytes: Vec<u8> = connection
        .query_row(select, [], |row| row.get(0))
        .expect("read topology carrier");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode topology carrier");
    let body = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("topology carrier body");
    match revision {
        Some(revision) => {
            body.insert("topology_revision".to_owned(), Value::from(revision));
        }
        None => {
            body.remove("topology_revision")
                .expect("current topology carrier has a revision");
        }
    }
    connection
        .execute(
            update,
            params![serde_json::to_vec(&document).expect("encode topology carrier")],
        )
        .expect("store topology carrier");
}

fn residency(connection: &Connection) -> ResidencyHead {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read residency head");
    wire::decode(&bytes).expect("residency head decodes")
}

fn request<B>(connection: &Connection, kind: CommandKind) -> Command<B>
where
    B: DeserializeOwned,
{
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT request FROM command_receipts WHERE command_kind = ?1",
            params![kind.as_str()],
            |row| row.get(0),
        )
        .expect("read topology request");
    wire::decode(&bytes).expect("topology request decodes")
}

fn response<B>(connection: &Connection, kind: CommandKind) -> B
where
    B: DeserializeOwned,
{
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_kind = ?1",
            params![kind.as_str()],
            |row| row.get(0),
        )
        .expect("read topology response");
    let response: Response<B> = wire::decode(&bytes).expect("topology response decodes");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("topology response is a reply");
    };
    reply.body
}

fn topology_carriers(connection: &Connection) -> Vec<Vec<u8>> {
    let mut carriers = vec![
        connection
            .query_row(
                "SELECT body FROM residency_head WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("read topology head carrier"),
    ];
    let mut statement = connection
        .prepare(
            "SELECT request, response FROM command_receipts \
             WHERE command_kind IN ('lane::arrange', 'lane::seat', 'lane::state', \
                                    'lineage::arrange', 'lineage::configure') \
             ORDER BY command_id",
        )
        .expect("prepare topology carrier query");
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("query topology carriers")
    {
        let (request, response) = row.expect("read topology carriers");
        carriers.extend([request, response]);
    }
    carriers
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
