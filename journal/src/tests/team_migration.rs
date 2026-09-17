//! Schema 25 to 26: the lane group is named `team` in every durable carrier.

use wire::{LaneRenewalRecord, LaneRenewalState, RenewalIdentity};

use super::*;

const TEAM: &str = "018f82d4-6f59-7d06-92e3-bce913116350";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116351";
const OTHER: &str = "018f82d4-6f59-7d06-92e3-bce913116352";
const CONFIGURE: &str = "018f82d4-6f59-7d06-92e3-bce913116353";
const ARRANGE: &str = "018f82d4-6f59-7d06-92e3-bce913116354";

type Receipt = (String, String, Vec<u8>, Vec<u8>, Vec<u8>);

#[test]
fn schema_twenty_five_names_the_lane_group_team_in_every_carrier_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-twenty-five-team.sqlite3");
    let before = schema_twenty_five_authority(&path);
    assert!(
        before
            .facts
            .iter()
            .any(|(kind, bytes)| kind == "lineage::configured"
                && contains(bytes, b"\"lineage_id\":"))
            && before
                .residency
                .windows(11)
                .any(|window| window == b"\"org_slot\":")
            && before.receipts.iter().any(|(_, kind, request, _, _)| {
                kind == "lineage::configure"
                    && contains(request, b"\"kind\":\"lineage::configure\"")
            }),
        "the fixture is spelled as schema 25 spelled it"
    );

    let journal = Journal::open(&path).expect("schema-twenty-five authority migrates");

    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((25, SCHEMA_VERSION))
    );
    let after = carriers(&journal.connection);
    assert_eq!(after, before.with_team_spelling());
    let snapshot = journal.fact_snapshot().expect("migrated facts decode");
    let configured = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<wire::body::TeamConfigured>())
        .expect("the configured team survives");
    assert_eq!(configured.team, uuid(TEAM));
    let arranged = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<wire::body::LaneArranged>())
        .expect("the arranged lane survives");
    assert_eq!(
        (arranged.from_team, arranged.team),
        (uuid(OTHER), uuid(TEAM))
    );
    let head = journal
        .read(|read| read.residency_head())
        .expect("migrated residency reads")
        .expect("migrated residency exists");
    assert_eq!(head.body().renewals, vec![renewal()]);
    let configure: Command<wire::intent::TeamConfigure> = wire::decode(
        &after
            .receipts
            .iter()
            .find(|(command, ..)| command == CONFIGURE)
            .expect("the configure receipt survives")
            .2,
    )
    .expect("configure request is current command wire");
    assert_eq!(configure.kind, CommandKind::TeamConfigure);
    assert_eq!(configure.body.team, uuid(TEAM));
    journal.verify().expect("migrated authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("current authority reopens");
    assert_eq!(
        reopened.migration(),
        None,
        "the migration runs exactly once"
    );
    assert_eq!(carriers(&reopened.connection), after);
}

#[test]
fn a_carrier_the_current_wire_refuses_rolls_back_both_schema_versions() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-twenty-five-refused.sqlite3");
    schema_twenty_five_authority(&path);
    let connection = Connection::open(&path).expect("open predecessor fixture");
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE kind = 'lineage::configured'",
            [replace_all(
                &connection
                    .query_row(
                        "SELECT envelope FROM fact_envelopes WHERE kind = 'lineage::configured'",
                        [],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .expect("read the configured fact"),
                b"\"life\":",
                b"\"lift\":",
            )],
        )
        .expect("corrupt the configured fact");
    let before = carriers(&connection);
    drop(connection);

    let error = Journal::open(&path).expect_err("a carrier that is not wire refuses");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 25,
            to: 26,
            table: "fact_envelopes",
            ..
        })
    ));
    let connection = Connection::open(&path).expect("reopen refused authority");
    assert_eq!(version_records(&connection), (25, 25));
    assert_eq!(carriers(&connection), before);
}

/// A schema-25 authority whose lane group is spelled as every schema before
/// 26 spelled it: `lineage_id` keys, `lineage::` kinds, and the renewal slot
/// as `org_slot`.
fn schema_twenty_five_authority(path: &Path) -> Carriers {
    let journal = authority_file_fixture(path);
    for envelope in [
        fact(
            3,
            Some(2),
            FactKind::TeamConfigured,
            json!({"team": TEAM, "from": "drive", "life": "drain"}),
        ),
        fact(
            4,
            Some(3),
            FactKind::LaneArranged,
            json!({"lane": LANE, "from_team": OTHER, "team": TEAM, "before": OTHER}),
        ),
        fact(
            5,
            Some(4),
            FactKind::TeamArranged,
            json!({
                "from": [{"team": OTHER, "lanes": [LANE]}],
                "teams": [{"team": TEAM, "lanes": [LANE]}]
            }),
        ),
        fact(
            6,
            Some(5),
            FactKind::LaneRenewalChanged,
            serde_json::to_value(wire::body::LaneRenewalChanged {
                team: uuid(TEAM),
                lane: uuid(LANE),
                renewal: Some(renewal()),
            })
            .expect("renewal fact body encodes"),
        ),
    ] {
        insert_fact(&journal, envelope);
    }
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES
             (3, 'task:1kKRRsDP', 2, 1, 2),
             (4, 'task:1kKRRsDP', 3, 1, 3),
             (5, 'task:1kKRRsDP', 4, 1, 4),
             (6, 'task:1kKRRsDP', 5, 1, 5);
             UPDATE cursor_head SET seq = 6 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 6 WHERE singleton = 1;",
        )
        .expect("index the team facts");
    let mut residency = residency_head();
    residency.lanes[0].lane = uuid(LANE);
    residency.lanes[0].team = uuid(TEAM);
    residency.subscriptions[0].team = uuid(TEAM);
    residency.renewals = vec![renewal()];
    journal
        .connection
        .execute(
            "UPDATE residency_head SET head_seq = 6, body = ?1 WHERE singleton = 1",
            [wire::encode(&residency).expect("residency encodes")],
        )
        .expect("store the team residency");
    let answer = serde_json::to_value(wire::intent::LaneAnswer {
        epoch: residency.epoch,
        state: residency.state,
        topology_revision: 0,
        lanes: Vec::new(),
        subscriptions: residency.subscriptions.clone(),
        renewals: residency.renewals.clone(),
    })
    .expect("lane answer encodes");
    insert_receipt_row(
        &journal,
        CONFIGURE,
        CommandKind::TeamConfigure,
        json!({"topology_revision": 0, "team": TEAM, "life": "drain"}),
        3,
        answer.clone(),
    );
    insert_receipt_row(
        &journal,
        ARRANGE,
        CommandKind::TeamArrange,
        json!({"topology_revision": 0, "teams": [{"team": TEAM, "lanes": [LANE]}]}),
        5,
        answer,
    );
    spell_lineage(&journal.connection);
    spell_slots_as_seventeen(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 25 WHERE singleton = 1;
             PRAGMA user_version = 25;",
        )
        .expect("downgrade the fixture to schema 25");
    let stored = carriers(&journal.connection);
    drop(journal);
    stored
}

fn renewal() -> LaneRenewalRecord {
    LaneRenewalRecord {
        team: uuid(TEAM),
        lane: uuid(LANE),
        revision: 0,
        state: LaneRenewalState::Requested {
            predecessor: RenewalIdentity {
                occupant: uuid(OTHER),
                session: "session-predecessor".to_owned(),
                thread: "thread-predecessor".to_owned(),
            },
            team_slot: 0,
        },
    }
}

fn insert_receipt_row(
    journal: &Journal,
    command: &str,
    kind: CommandKind,
    body: Value,
    committed: u64,
    answer: Value,
) {
    let id = uuid(command);
    let epoch = residency_head().epoch;
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
                first: seq(committed),
                last: seq(committed),
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
                    .expect("canonical request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
                wire::encode(&response).expect("encode response"),
                i64::try_from(committed).expect("fixture seq fits SQLite"),
                epoch.to_string(),
            ],
        )
        .expect("insert a team receipt");
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Carriers {
    facts: Vec<(String, Vec<u8>)>,
    residency: Vec<u8>,
    task_heads: Vec<Vec<u8>>,
    receipts: Vec<Receipt>,
}

impl Carriers {
    /// The carriers as schema 26 spells them: a request is canonical command
    /// JSON, so its keys sort again once the lane group is named `team`.
    fn with_team_spelling(mut self) -> Self {
        for (kind, bytes) in &mut self.facts {
            *kind = team_kind(kind);
            *bytes = team_spelling(bytes);
        }
        self.residency = team_spelling(&self.residency);
        for body in &mut self.task_heads {
            *body = team_spelling(body);
        }
        for (_, kind, request, witnesses, response) in &mut self.receipts {
            *kind = team_kind(kind);
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

fn team_kind(kind: &str) -> String {
    kind.strip_prefix("lineage::")
        .map_or_else(|| kind.to_owned(), |rest| format!("team::{rest}"))
}

fn carriers(connection: &Connection) -> Carriers {
    let facts = query(
        connection,
        "SELECT kind, envelope FROM fact_envelopes ORDER BY seq",
        |row| Ok((row.get(0)?, row.get(1)?)),
    );
    let residency = connection
        .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
        .expect("read residency carrier");
    let task_heads = query(
        connection,
        "SELECT body FROM task_heads ORDER BY stamp",
        |row| row.get(0),
    );
    let receipts = query(
        connection,
        "SELECT command_id, command_kind, request, evaluated_witnesses, response \
         FROM command_receipts ORDER BY command_id",
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    );
    Carriers {
        facts,
        residency,
        task_heads,
        receipts,
    }
}

fn query<T>(
    connection: &Connection,
    sql: &str,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let mut statement = connection.prepare(sql).expect("prepare carrier rows");
    statement
        .query_map([], map)
        .expect("read carrier rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect carrier rows")
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}
