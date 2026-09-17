//! Schema 16 to 17 renewal-slot migration.

use rusqlite::params;
use wire::{LaneRenewalRecord, LaneRenewalState, RenewalIdentity};

use super::*;

const LINEAGE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const PREDECESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const SUCCESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116304";
const RECEIPT_KINDS: &[CommandKind] = &[
    CommandKind::BoardRead,
    CommandKind::LaneRead,
    CommandKind::LaneArrange,
    CommandKind::TeamConfigure,
    CommandKind::TeamArrange,
    CommandKind::LaneRenewal,
];

type Corruption = (&'static str, fn(&Connection));
type ReceiptBytes = (String, Vec<u8>, Vec<u8>, Vec<u8>);

#[test]
fn schema_sixteen_rewrites_every_renewal_carrier_and_replays_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-sixteen-renewal-slots.sqlite3");
    let (before, replay_command) = renewal_authority(&path, 16, true);

    let journal = Journal::open(&path).expect("schema-sixteen renewal authority migrates");
    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((16, SCHEMA_VERSION))
    );
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    let after = carrier_snapshot(&journal.connection);
    assert_eq!(after, before.with_team_spelling());
    assert_current_carriers(&journal.connection);
    journal.verify().expect("migrated authority verifies");
    assert_lane_answer_replays(&journal, replay_command);
    drop(journal);

    let reopened = Journal::open(&path).expect("current renewal authority reopens");
    assert_eq!(reopened.migration(), None, "migration runs exactly once");
    assert_eq!(carrier_snapshot(&reopened.connection), after);
}

#[test]
fn schema_fifteen_chains_through_rollout_and_renewal_slot_migration() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-fifteen-renewal-slots.sqlite3");
    let (before, _) = renewal_authority(&path, 15, true);

    let journal = Journal::open(&path).expect("schema-fifteen renewal authority migrates");

    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((15, SCHEMA_VERSION))
    );
    assert_eq!(
        carrier_snapshot(&journal.connection),
        before.with_team_spelling()
    );
    journal.verify().expect("chained migration verifies");
}

#[test]
fn schema_seventeen_slots_cross_schema_sixteen_without_an_alternate_rewrite() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("schema-sixteen-seventeen-renewal-slots.sqlite3");
    let (before, _) = renewal_authority(&path, 16, false);

    let journal = Journal::open(&path).expect("schema-seventeen slots cross by identity only");

    assert_eq!(
        carrier_snapshot(&journal.connection),
        before.with_team_spelling()
    );
    assert_current_carriers(&journal.connection);
    journal
        .verify()
        .expect("current renewal authority verifies");
}

#[test]
fn ambiguous_missing_and_noncanonical_slots_refuse_and_roll_back() {
    let cases: &[Corruption] = &[
        ("both", add_current_slot),
        ("neither", remove_retired_slot),
        ("noncanonical", make_fact_noncanonical),
        (
            "noncanonical-receipt-answer",
            make_receipt_answer_noncanonical,
        ),
    ];
    for &(name, corrupt) in cases {
        let scratch = Scratch::new();
        let path = scratch
            .path()
            .join(format!("schema-sixteen-{name}.sqlite3"));
        renewal_authority(&path, 16, true);
        let connection = Connection::open(&path).expect("open predecessor fixture");
        corrupt(&connection);
        let before = carrier_snapshot(&connection);
        drop(connection);

        let error = Journal::open(&path).expect_err("invalid renewal authority refuses migration");

        assert!(matches!(
            error,
            OpenError::Migration(MigrationError::StoredContent {
                from: 16,
                to: 17,
                ..
            })
        ));
        let after = Connection::open(&path).expect("reopen refused predecessor authority");
        assert_eq!(version_records(&after), (16, 16));
        assert_eq!(
            carrier_snapshot(&after),
            before,
            "{name} rollback is atomic"
        );
    }
}

/// A renewal-bearing authority at `schema`, its slots spelled as schema 16
/// spelled them (`team_slot`) or, when `sixteen_slots` is false, as schema 17
/// already spelled them (`org_slot`).
fn renewal_authority(
    path: &Path,
    schema: u32,
    sixteen_slots: bool,
) -> (CarrierSnapshot, Command<Value>) {
    let journal = authority_file_fixture(path);
    let renewals = renewal_records();
    for (index, renewal) in renewals.iter().enumerate() {
        let position = 3 + u64::try_from(index).expect("fixture index fits u64");
        let body = wire::body::LaneRenewalChanged {
            team: uuid(LINEAGE),
            lane: uuid(LANE),
            renewal: Some(renewal.clone()),
        };
        insert_fact(
            &journal,
            fact(
                position,
                Some(position - 1),
                FactKind::LaneRenewalChanged,
                serde_json::to_value(body).expect("renewal fact body encodes"),
            ),
        );
        journal
            .connection
            .execute(
                "INSERT INTO thread_index(seq, origin, cause, root, depth) \
                 VALUES (?1, 'task:1kKRRsDP', ?2, 1, ?3)",
                params![
                    i64::try_from(position).expect("fixture position fits SQLite"),
                    i64::try_from(position - 1).expect("fixture cause fits SQLite"),
                    i64::try_from(position - 1).expect("fixture depth fits SQLite"),
                ],
            )
            .expect("insert renewal thread row");
    }
    let mut residency = residency_head();
    residency.topology_revision = 4;
    residency.renewals = renewals.clone();
    journal
        .connection
        .execute(
            "UPDATE residency_head SET head_seq = 6, body = ?1 WHERE singleton = 1",
            params![wire::encode(&residency).expect("renewal residency encodes")],
        )
        .expect("store renewal residency");
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 6 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 6 WHERE singleton = 1;",
        )
        .expect("advance renewal fixture cursors");
    let lane_read = insert_receipts(&journal.connection, &residency, &renewals);
    spell_lineage(&journal.connection);
    if !sixteen_slots {
        spell_slots_as_seventeen(&journal.connection);
    }
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE journal_schema SET version = {schema} WHERE singleton = 1;
             PRAGMA user_version = {schema};"
        ))
        .expect("set predecessor schema identity");
    let snapshot = carrier_snapshot(&journal.connection);
    drop(journal);
    (snapshot, lane_read)
}

fn renewal_records() -> Vec<LaneRenewalRecord> {
    let predecessor = || RenewalIdentity {
        occupant: uuid(PREDECESSOR),
        session: "session-predecessor".to_owned(),
        thread: "thread-predecessor".to_owned(),
    };
    let successor = || RenewalIdentity {
        occupant: uuid(SUCCESSOR),
        session: "session-successor".to_owned(),
        thread: "thread-successor".to_owned(),
    };
    [
        LaneRenewalState::Requested {
            predecessor: predecessor(),
            team_slot: 0,
        },
        LaneRenewalState::HandoffPending {
            predecessor: predecessor(),
            team_slot: 1,
        },
        LaneRenewalState::SuccessorStarted {
            predecessor: predecessor(),
            successor: successor(),
            team_slot: 2,
        },
        LaneRenewalState::LaunchRefused {
            predecessor: predecessor(),
            team_slot: 3,
            reason: "vendor refused launch".to_owned(),
        },
    ]
    .into_iter()
    .map(|state| LaneRenewalRecord {
        team: uuid(LINEAGE),
        lane: uuid(LANE),
        revision: 4,
        state,
    })
    .collect()
}

fn insert_receipts(
    connection: &Connection,
    residency: &ResidencyHead,
    renewals: &[LaneRenewalRecord],
) -> Command<Value> {
    let lane_answer = wire::intent::LaneAnswer {
        epoch: residency.epoch,
        state: residency.state,
        topology_revision: residency.topology_revision,
        lanes: Vec::new(),
        subscriptions: residency.subscriptions.clone(),
        renewals: renewals.to_vec(),
    };
    let board_answer = wire::intent::BoardAnswer {
        schema: 16,
        cursor: seq(6),
        principal: Actor::Operator,
        project_policy: wire::intent::BoardProjectPolicy {
            public_stems: vec!["journal".to_owned()],
            public_min_depth: 1,
            public_max_depth: 3,
        },
        actor_route: wire::intent::BoardActorRoute {
            actor: "operator".to_owned(),
            project: "agent.operator.task".to_owned(),
            origin_thread: "operator".to_owned(),
        },
        allocation_band: 0,
        recent_cell: None,
        tasks: Vec::new(),
        trust: Vec::new(),
        residency: wire::intent::Positioned {
            position: seq(6),
            body: residency.clone(),
        },
    };
    let bodies = [
        json!({}),
        json!({}),
        json!({"topology_revision": 4, "lane": LANE, "before": LANE}),
        json!({"topology_revision": 4, "team": LINEAGE}),
        json!({
            "topology_revision": 4,
            "teams": [{"team": LINEAGE, "lanes": [LANE]}]
        }),
        json!({
            "topology_revision": 4,
            "team": LINEAGE,
            "lane": LANE,
            "requested": true
        }),
    ];
    let mut replay_command = None;
    for (index, (&kind, body)) in RECEIPT_KINDS.iter().zip(bodies).enumerate() {
        let command = Command {
            v: WireVersion::CURRENT,
            id: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131164{index:02}")),
            epoch: residency.epoch,
            kind,
            expected: Vec::new(),
            body,
        };
        let answer = match kind {
            CommandKind::BoardRead => serde_json::to_value(&board_answer),
            CommandKind::LaneRenewal => serde_json::to_value(wire::intent::LaneRenewalAnswer {
                team: uuid(LINEAGE),
                lane: uuid(LANE),
                topology_revision: 4,
                renewal: Some(renewals[0].clone()),
            }),
            _ => serde_json::to_value(&lane_answer),
        }
        .expect("receipt answer encodes");
        insert_response(connection, &command, answer, index < 2);
        if kind == CommandKind::LaneArrange {
            replay_command = Some(command);
        }
    }
    replay_command.expect("committed lane answer receipt is inserted")
}

fn insert_response(connection: &Connection, command: &Command<Value>, body: Value, snapshot: bool) {
    let position = if snapshot {
        ReplyPosition::Snapshot(seq(6))
    } else {
        ReplyPosition::Committed(SeqRange {
            first: seq(3),
            last: seq(6),
        })
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply { position, body }),
    };
    let (first, last, snapshot_seq) = if snapshot {
        (None, None, Some(6))
    } else {
        (Some(3), Some(6), None)
    };
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                command.id.to_string(),
                command.kind.as_str(),
                command
                    .canonical_json()
                    .expect("receipt request is canonical")
                    .into_bytes(),
                wire::encode(&command.expected).expect("receipt witnesses encode"),
                wire::encode(&response).expect("receipt response encodes"),
                first,
                last,
                snapshot_seq,
                command.epoch.to_string(),
            ],
        )
        .expect("insert renewal-bearing receipt");
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CarrierSnapshot {
    facts: Vec<(i64, Vec<u8>)>,
    residency: Vec<u8>,
    receipts: Vec<ReceiptBytes>,
}

impl CarrierSnapshot {
    /// The snapshot as schema 26 spells it: a request is canonical command
    /// JSON, so its keys sort again once the lane group is named `team`.
    fn with_team_spelling(mut self) -> Self {
        for (_, bytes) in &mut self.facts {
            *bytes = team_spelling(bytes);
        }
        self.residency = team_spelling(&self.residency);
        for (_, request, witnesses, response) in &mut self.receipts {
            let command: Command<Value> =
                wire::decode(&team_spelling(request)).expect("respelled request decodes");
            *request = command
                .canonical_json()
                .expect("respelled request is canonical")
                .into_bytes();
            *witnesses = team_spelling(witnesses);
            *response = team_spelling(response);
        }
        self
    }
}

fn carrier_snapshot(connection: &Connection) -> CarrierSnapshot {
    let facts = rows(
        connection,
        "SELECT seq, envelope FROM fact_envelopes ORDER BY seq",
    );
    let residency = connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("read residency carrier");
    let mut statement = connection
        .prepare(
            "SELECT command_id, request, evaluated_witnesses, response \
             FROM command_receipts ORDER BY command_id",
        )
        .expect("prepare receipt carriers");
    let receipts = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("read receipt carriers")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect receipt carriers");
    CarrierSnapshot {
        facts,
        residency,
        receipts,
    }
}

fn rows(connection: &Connection, sql: &str) -> Vec<(i64, Vec<u8>)> {
    let mut statement = connection.prepare(sql).expect("prepare carrier rows");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("read carrier rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect carrier rows")
}

fn assert_current_carriers(connection: &Connection) {
    for (_, bytes) in rows(
        connection,
        "SELECT seq, envelope FROM fact_envelopes WHERE kind = 'lane::renewal'",
    ) {
        wire::decode::<Envelope<AnyFact>>(&bytes).expect("renewal fact is current wire");
    }
    let head: Vec<u8> = connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("read migrated residency");
    wire::decode::<ResidencyHead>(&head).expect("residency is current wire");
    let mut statement = connection
        .prepare("SELECT command_kind, response FROM command_receipts ORDER BY command_id")
        .expect("prepare migrated receipts");
    let receipts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("read migrated receipts");
    for receipt in receipts {
        let (kind, bytes) = receipt.expect("migrated receipt row");
        match kind.as_str() {
            "board::read" => {
                wire::decode::<Response<wire::intent::BoardAnswer>>(&bytes)
                    .expect("board response is current wire");
            }
            "lane::read" | "lane::arrange" | "team::configure" | "team::arrange" => {
                wire::decode::<Response<wire::intent::LaneAnswer>>(&bytes)
                    .expect("lane response is current wire");
            }
            "lane::renewal" => {
                wire::decode::<Response<wire::intent::LaneRenewalAnswer>>(&bytes)
                    .expect("renewal response is current wire");
            }
            _ => {}
        }
    }
}

fn assert_lane_answer_replays(journal: &Journal, command: Command<Value>) {
    let epoch = command.epoch;
    let admitted = command
        .admit(&wire::Accepted {
            wire: WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
        })
        .expect("lane command admits");
    let replay = journal
        .read(move |read| read.command_replay(&admitted))
        .expect("lane answer replay reads")
        .expect("lane answer replay exists");
    assert!(matches!(
        replay,
        crate::CommandReplay::Exact(Outcome::Reply(Reply {
            body: wire::intent::CommandAnswer::Lane(_),
            ..
        }))
    ));
}

fn add_current_slot(connection: &Connection) {
    mutate_first_residency_state(connection, |state| {
        state.insert("org_slot".to_owned(), json!(0));
    });
}

fn remove_retired_slot(connection: &Connection) {
    mutate_first_residency_state(connection, |state| {
        state.remove("team_slot");
    });
}

fn mutate_first_residency_state(
    connection: &Connection,
    mutate: impl FnOnce(&mut serde_json::Map<String, Value>),
) {
    let bytes: Vec<u8> = connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("read residency for corruption");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode residency JSON");
    let state = document["renewals"][0]["state"]
        .as_object_mut()
        .expect("renewal state object");
    mutate(state);
    connection
        .execute(
            "UPDATE residency_head SET body = ?1",
            [serde_json::to_vec(&document).expect("encode corrupted residency")],
        )
        .expect("store corrupted residency");
}

fn make_fact_noncanonical(connection: &Connection) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'lane::renewal' ORDER BY seq LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("read renewal fact for corruption");
    let mut noncanonical = bytes;
    noncanonical.push(b' ');
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 \
             WHERE seq = (SELECT min(seq) FROM fact_envelopes WHERE kind = 'lane::renewal')",
            [noncanonical],
        )
        .expect("store noncanonical fact");
}

fn make_receipt_answer_noncanonical(connection: &Connection) {
    let command = CommandKind::BoardRead.as_str();
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_kind = ?1",
            [command],
            |row| row.get(0),
        )
        .expect("read board response for corruption");
    let mut response: Response<Value> =
        wire::decode(&bytes).expect("decode generic board response");
    let Outcome::Reply(reply) = &mut response.outcome else {
        panic!("board response is a reply");
    };
    reply.body["residency"]["body"]["subscriptions"][0]
        .as_object_mut()
        .expect("board response lineage subscription")
        .insert("manual_projects".to_owned(), json!([]));
    connection
        .execute(
            "UPDATE command_receipts SET response = ?1 WHERE command_kind = ?2",
            params![
                wire::encode(&response).expect("encode generic-canonical board response"),
                command,
            ],
        )
        .expect("store noncanonical board response");
}
