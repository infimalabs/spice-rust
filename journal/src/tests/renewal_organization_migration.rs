//! Schema 16 to 17 durable renewal-slot migration.

use rusqlite::params;
use wire::body::LaneRenewalChanged;
use wire::{LaneRenewalRecord, LaneRenewalState, RenewalIdentity};

use super::*;

const LINEAGE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const PREDECESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const SUCCESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116304";
const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116390";

#[test]
fn schema_sixteen_renames_every_renewal_state_across_all_durable_carriers_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("renewal-organization.sqlite3");
    let journal = authority_file_fixture(&path);
    let states = states();
    for (offset, state) in states.iter().cloned().enumerate() {
        let position = u64::try_from(offset).expect("offset fits") + 3;
        let changed = LaneRenewalChanged {
            lineage_id: uuid(LINEAGE),
            lane: uuid(LANE),
            renewal: Some(record(state)),
        };
        insert_fact(
            &journal,
            fact(
                position,
                Some(position - 1),
                FactKind::LaneRenewalChanged,
                serde_json::to_value(changed).expect("renewal fact body encodes"),
            ),
        );
        journal
            .connection
            .execute(
                "INSERT INTO thread_index(seq, origin, cause, root, depth) \
                 VALUES (?1, 'task:1kKRRsDP', ?2, 1, ?3)",
                params![
                    i64::try_from(position).expect("position fits"),
                    i64::try_from(position - 1).expect("cause fits"),
                    i64::try_from(position - 1).expect("depth fits")
                ],
            )
            .expect("renewal thread row inserts");
    }
    let mut head = residency_head();
    head.renewals = vec![record(states[0].clone())];
    journal
        .connection
        .execute(
            "UPDATE residency_head SET head_seq = 6, body = ?1 WHERE singleton = 1",
            [wire::encode(&head).expect("renewal residency encodes")],
        )
        .expect("renewal residency persists");
    insert_renewal_receipt(&journal, record(states[3].clone()));
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 6; \
             UPDATE steering_index SET indexed_seq = 6;",
        )
        .expect("fixture cursor advances");
    retire_slots(&journal.connection);
    set_schema_sixteen(&journal.connection);
    drop(journal);

    let journal = Journal::open(&path).expect("schema sixteen renewal authority migrates");
    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((16, SCHEMA_VERSION))
    );
    let facts = journal.fact_snapshot().expect("migrated facts decode");
    let renewal_states = facts
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<LaneRenewalChanged>())
        .map(|changed| changed.renewal.expect("fixture renewal remains").state)
        .collect::<Vec<_>>();
    assert_eq!(renewal_states, states);
    let head = journal
        .read(|read| read.residency_head())
        .expect("migrated residency reads")
        .expect("migrated residency exists");
    assert_eq!(head.body().renewals, vec![record(states[0].clone())]);
    let response = renewal_response(&journal.connection);
    let Outcome::Reply(reply) = response.outcome else {
        panic!("renewal receipt remains successful");
    };
    assert_eq!(reply.body.renewal, Some(record(states[3].clone())));
    assert_no_retired_slots(&journal.connection);
    let once = migrated_carriers(&journal.connection);
    drop(journal);

    let reopened = Journal::open(&path).expect("current schema reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(migrated_carriers(&reopened.connection), once);
}

#[test]
fn ambiguous_renewal_slot_rolls_back_carriers_and_both_schema_versions() {
    let scratch = Scratch::new();
    let path = scratch.path().join("renewal-organization-conflict.sqlite3");
    let journal = authority_file_fixture(&path);
    let mut head = residency_head();
    head.renewals = vec![record(states()[0].clone())];
    let mut document = serde_json::to_value(&head).expect("residency encodes");
    let state = document["renewals"][0]["state"]
        .as_object_mut()
        .expect("renewal state object");
    state.insert("team_slot".to_owned(), json!(0));
    let planted = serde_json::to_vec(&document).expect("conflict encodes");
    journal
        .connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            [&planted],
        )
        .expect("conflict persists");
    set_schema_sixteen(&journal.connection);
    drop(journal);

    let error = Journal::open(&path).expect_err("ambiguous slot refuses migration");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 16,
            to: 17,
            table: "residency_head",
            ref detail,
            ..
        }) if detail.contains("both team_slot and org_slot")
    ));
    let connection = Connection::open(&path).expect("rolled-back authority reopens directly");
    let row_version: u32 = connection
        .query_row("SELECT version FROM journal_schema", [], |row| row.get(0))
        .expect("schema row reads");
    let pragma_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("schema pragma reads");
    let body: Vec<u8> = connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("residency bytes read");
    assert_eq!((row_version, pragma_version), (16, 16));
    assert_eq!(body, planted);
}

fn states() -> Vec<LaneRenewalState> {
    vec![
        LaneRenewalState::Requested {
            predecessor: predecessor(),
            org_slot: 0,
        },
        LaneRenewalState::HandoffPending {
            predecessor: predecessor(),
            org_slot: 1,
        },
        LaneRenewalState::SuccessorStarted {
            predecessor: predecessor(),
            successor: successor(),
            org_slot: 2,
        },
        LaneRenewalState::LaunchRefused {
            predecessor: predecessor(),
            org_slot: 3,
            reason: "fixture refusal".to_owned(),
        },
    ]
}

fn record(state: LaneRenewalState) -> LaneRenewalRecord {
    LaneRenewalRecord {
        lineage_id: uuid(LINEAGE),
        lane: uuid(LANE),
        revision: 0,
        state,
    }
}

fn predecessor() -> RenewalIdentity {
    RenewalIdentity {
        occupant: uuid(PREDECESSOR),
        session: "session-predecessor".to_owned(),
        thread: "thread-predecessor".to_owned(),
    }
}

fn successor() -> RenewalIdentity {
    RenewalIdentity {
        occupant: uuid(SUCCESSOR),
        session: "session-successor".to_owned(),
        thread: "thread-successor".to_owned(),
    }
}

fn insert_renewal_receipt(journal: &Journal, renewal: LaneRenewalRecord) {
    let id = uuid(COMMAND);
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116304");
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::LaneRenewal,
        expected: Vec::new(),
        body: wire::intent::LaneRenewal {
            topology_revision: 0,
            lineage_id: uuid(LINEAGE),
            lane: uuid(LANE),
            requested: true,
        },
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(6),
                last: seq(6),
            }),
            body: serde_json::to_value(wire::intent::LaneRenewalAnswer {
                lineage_id: uuid(LINEAGE),
                lane: uuid(LANE),
                topology_revision: 0,
                renewal: Some(renewal),
            })
            .expect("renewal answer encodes"),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(\
                 command_id, actor, command_kind, request, evaluated_witnesses, response, \
                 first_seq, last_seq, snapshot_seq, epoch\
             ) VALUES (?1, 'operator', 'lane::renewal', ?2, ?3, ?4, 6, 6, NULL, ?5)",
            params![
                id.to_string(),
                request
                    .canonical_json()
                    .expect("request canonical")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("witnesses encode"),
                wire::encode(&response).expect("response encodes"),
                epoch.to_string(),
            ],
        )
        .expect("renewal receipt inserts");
}

fn retire_slots(connection: &Connection) {
    for (select, update) in [
        (
            "SELECT CAST(seq AS TEXT), envelope FROM fact_envelopes WHERE kind = 'lane::renewal'",
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
        ),
        (
            "SELECT CAST(singleton AS TEXT), body FROM residency_head",
            "UPDATE residency_head SET body = ?1 WHERE singleton = ?2",
        ),
        (
            "SELECT command_id, response FROM command_receipts WHERE command_kind = 'lane::renewal'",
            "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
        ),
    ] {
        let mut statement = connection.prepare(select).expect("carrier query prepares");
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("carrier rows query")
            .collect::<Result<Vec<_>, _>>()
            .expect("carrier rows collect");
        drop(statement);
        for (key, bytes) in rows {
            connection
                .execute(update, params![retire_slot_bytes(&bytes), key])
                .expect("retired carrier writes");
        }
    }
}

fn retire_slot_bytes(bytes: &[u8]) -> Vec<u8> {
    let current = br#""org_slot":"#;
    let retired = br#""team_slot":"#;
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while let Some(relative) = bytes[cursor..]
        .windows(current.len())
        .position(|window| window == current)
    {
        let start = cursor + relative;
        output.extend_from_slice(&bytes[cursor..start]);
        output.extend_from_slice(retired);
        cursor = start + current.len();
    }
    output.extend_from_slice(&bytes[cursor..]);
    output
}

fn set_schema_sixteen(connection: &Connection) {
    connection
        .execute(
            "UPDATE journal_schema SET version = 16 WHERE singleton = 1",
            [],
        )
        .expect("schema row downgrades");
    connection
        .pragma_update(None, "user_version", 16)
        .expect("schema pragma downgrades");
}

fn renewal_response(connection: &Connection) -> Response<wire::intent::LaneRenewalAnswer> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_id = ?1",
            [COMMAND],
            |row| row.get(0),
        )
        .expect("renewal response reads");
    wire::decode(&bytes).expect("renewal response is current wire")
}

fn assert_no_retired_slots(connection: &Connection) {
    for (table, column) in [
        ("fact_envelopes", "envelope"),
        ("residency_head", "body"),
        ("command_receipts", "response"),
    ] {
        let sql = format!(
            "SELECT count(*) FROM {table} WHERE instr(CAST({column} AS TEXT), 'team_slot') > 0"
        );
        let count: u32 = connection.query_row(&sql, [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "{table}.{column} retains no retired slot key");
    }
}

fn migrated_carriers(connection: &Connection) -> Vec<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut statement = connection
        .prepare("SELECT envelope FROM fact_envelopes WHERE kind = 'lane::renewal' ORDER BY seq")
        .expect("fact carrier query");
    bytes.extend(
        statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
    );
    bytes.push(
        connection
            .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
            .unwrap(),
    );
    bytes.push(
        connection
            .query_row(
                "SELECT response FROM command_receipts WHERE command_id = ?1",
                [COMMAND],
                |row| row.get(0),
            )
            .unwrap(),
    );
    bytes
}
