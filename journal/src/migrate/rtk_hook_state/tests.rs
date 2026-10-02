use super::*;
use wire::body::{LaneLaunchIdentity, LaneSessionDeclared, RtkHealth, RtkHealthState};
use wire::intent::{LaneAnswer, LaneAnswerRow, LaneChangeAnswer, LaneLiveness};
use wire::{
    Actor, AnyFact, CanonicalUuid, Envelope, Fact, Instant, LaneResidency, LaneSession, Outcome,
    Reply, ReplyPosition, ResidencyHead, ResidentState, Response, Seq, SeqRange, SpeechPlayback,
    TapeLocus, WireVersion,
};

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn health() -> RtkHealth {
    RtkHealth::new(
        RtkHealthState::Active,
        Some("0.45.0".to_owned()),
        "rewrite protocol passed",
    )
}

fn launch() -> LaneLaunchIdentity {
    LaneLaunchIdentity::new("claude-opus-4-6", "high")
        .with_git("current")
        .with_rtk(health())
}

fn declaration() -> LaneSessionDeclared {
    LaneSessionDeclared {
        lane: uuid(1),
        occupant: uuid(2),
        worktree: "/worktrees/lane".to_owned(),
        vendor: "codex".parse().expect("historical vendor name"),
        launch: Some(launch()),
        epoch: uuid(3),
        stream: "stdout".to_owned(),
        source: Some(uuid(4)),
        identity: "session-a".to_owned(),
        member: 1,
        offset: 2,
        length: 3,
    }
}

fn session() -> LaneSession {
    LaneSession {
        lane: uuid(1),
        occupant: uuid(2),
        worktree: "/worktrees/lane".to_owned(),
        vendor: "codex".parse().expect("historical vendor name"),
        launch: Some(launch()),
        epoch: uuid(3),
        stream: "stdout".to_owned(),
        identity: "session-a".to_owned(),
        locus: TapeLocus {
            member: 1,
            offset: 2,
            length: 3,
        },
    }
}

fn residency() -> ResidencyHead {
    ResidencyHead {
        epoch: uuid(3),
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![LaneResidency {
            lane: uuid(1),
            team: uuid(1),
            name: None,
            worktree: "/worktrees/lane".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("historical vendor name"),
            model: None,
            effort: None,
            occupant: Some(uuid(2)),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: Some(session()),
            disposition: "active".to_owned(),
        }],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    }
}

fn lane_row() -> LaneAnswerRow {
    LaneAnswerRow {
        lane: uuid(1),
        team: uuid(1),
        name: None,
        worktree: "/worktrees/lane".to_owned(),
        speech: SpeechPlayback::Disabled {},
        life: "drive".to_owned(),
        vendor: "codex".parse().expect("historical vendor name"),
        model: Some("claude-opus-4-6".to_owned()),
        seat_model: None,
        seat_effort: None,
        effort: Some("high".to_owned()),
        fast_mode: false,
        git: Some("current".to_owned()),
        rtk: Some(health()),
        occupant: Some(uuid(2)),
        lineage: Vec::new(),
        session: Some("session-a".to_owned()),
        bound: true,
        disposition: "active".to_owned(),
        liveness: LaneLiveness::Working,
        jobs: Vec::new(),
    }
}

fn response<T>(id: u64, body: T) -> Response<T> {
    Response {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(3),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(1).expect("positive sequence"),
                last: Seq::new(1).expect("positive sequence"),
            }),
            body,
        }),
    }
}

fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory migration fixture");
    connection
        .execute_batch(
            "CREATE TABLE fact_envelopes (
                seq INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                envelope BLOB NOT NULL
            ) STRICT;
            CREATE TABLE residency_head (
                singleton INTEGER PRIMARY KEY,
                body BLOB NOT NULL
            ) STRICT;
            CREATE TABLE command_receipts (
                command_id TEXT PRIMARY KEY,
                command_kind TEXT NOT NULL,
                response BLOB NOT NULL
            ) STRICT;",
        )
        .expect("migration carrier schema");
    connection
}

fn encode_with_hook<T: serde::Serialize>(value: &T, pointer: &str) -> Vec<u8> {
    let mut document = serde_json::to_value(value).expect("fixture encodes to JSON");
    document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .expect("fixture RTK health is an object")
        .insert("hook".to_owned(), Value::String("installed".to_owned()));
    serde_json::to_vec(&document).expect("schema-31 fixture encodes")
}

fn stored(connection: &Connection, sql: &str, key: impl rusqlite::ToSql) -> Vec<u8> {
    connection
        .query_row(sql, [key], |row| row.get(0))
        .expect("migrated carrier is stored")
}

fn seed_schema_thirty_one(connection: &Connection) -> Vec<u8> {
    let envelope = Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(1).expect("positive sequence"),
        at: Instant::new("2030-04-12T18:44:09Z".parse().expect("fixture instant"))
            .expect("canonical instant"),
        actor: Actor::Spice,
        origin: format!("resident:{}", uuid(3))
            .parse()
            .expect("resident origin"),
        cause: None,
        fact: Fact {
            body: declaration(),
        },
    };
    connection
        .execute(
            "INSERT INTO fact_envelopes VALUES (1, 'lane::declared', ?1)",
            [encode_with_hook(&envelope, "/fact/rtk")],
        )
        .expect("schema-31 fact inserts");
    connection
        .execute(
            "INSERT INTO residency_head VALUES (1, ?1)",
            [encode_with_hook(&residency(), "/lanes/0/session/rtk")],
        )
        .expect("schema-31 residency inserts");

    let changed = response(
        10,
        LaneChangeAnswer {
            lane: uuid(1),
            topology_revision: 1,
            from: "vacant".to_owned(),
            to: "active".to_owned(),
            git: Some("current".into()),
            rtk: Some(health()),
        },
    );
    connection
        .execute(
            "INSERT INTO command_receipts VALUES (?1, 'lane::state', ?2)",
            params![
                uuid(10).to_string(),
                encode_with_hook(&changed, "/outcome/body/rtk")
            ],
        )
        .expect("schema-31 lane-state receipt inserts");
    let read = response(
        11,
        LaneAnswer {
            epoch: uuid(3),
            state: ResidentState::Seated,
            topology_revision: 1,
            lanes: vec![lane_row()],
            subscriptions: Vec::new(),
            renewals: Vec::new(),
            refresh_after_millis: None,
        },
    );
    connection
        .execute(
            "INSERT INTO command_receipts VALUES (?1, 'team::arrange', ?2)",
            params![
                uuid(11).to_string(),
                encode_with_hook(&read, "/outcome/body/lanes/0/rtk")
            ],
        )
        .expect("schema-31 lane-table receipt inserts");
    let unrelated = response(
        12,
        serde_json::json!({
            "rtk": {
                "state": "active",
                "hook": "installed",
                "detail": "authored task data"
            }
        }),
    );
    let unrelated = wire::encode(&unrelated).expect("unrelated receipt encodes");
    connection
        .execute(
            "INSERT INTO command_receipts VALUES (?1, 'task::done', ?2)",
            params![uuid(12).to_string(), &unrelated],
        )
        .expect("unrelated receipt inserts");
    unrelated
}

fn assert_current_carriers(connection: &Connection, unrelated: &[u8]) {
    let fact = stored(
        connection,
        "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
        1_i64,
    );
    assert!(!String::from_utf8_lossy(&fact).contains("\"hook\""));
    let envelope: Envelope<AnyFact> = wire::decode(&fact).expect("migrated fact is current wire");
    assert!(
        envelope
            .fact
            .read::<LaneSessionDeclared>()
            .is_some_and(|declared| declared.launch.is_some())
    );

    let head = stored(
        connection,
        "SELECT body FROM residency_head WHERE singleton = ?1",
        1_i64,
    );
    assert!(!String::from_utf8_lossy(&head).contains("\"hook\""));
    wire::decode::<ResidencyHead>(&head).expect("migrated residency is current wire");

    let state = stored(
        connection,
        "SELECT response FROM command_receipts WHERE command_id = ?1",
        uuid(10).to_string(),
    );
    assert!(!String::from_utf8_lossy(&state).contains("\"hook\""));
    wire::decode::<Response<LaneChangeAnswer>>(&state)
        .expect("migrated lane-state reply is current wire");
    let read = stored(
        connection,
        "SELECT response FROM command_receipts WHERE command_id = ?1",
        uuid(11).to_string(),
    );
    assert!(!String::from_utf8_lossy(&read).contains("\"hook\""));
    wire::decode::<Response<LaneAnswer>>(&read).expect("migrated lane-read reply is current wire");
    assert_eq!(
        stored(
            connection,
            "SELECT response FROM command_receipts WHERE command_id = ?1",
            uuid(12).to_string(),
        ),
        unrelated,
        "non-lane authored data is outside this migration"
    );
}

#[test]
fn schema_thirty_one_retires_hook_state_from_every_durable_rtk_carrier() {
    let connection = fixture();
    let unrelated = seed_schema_thirty_one(&connection);
    migrate(&connection).expect("schema-31 RTK carriers migrate");
    assert_current_carriers(&connection, &unrelated);
}

#[test]
fn an_unknown_retired_hook_state_refuses_without_rewriting_the_fact() {
    let connection = fixture();
    let envelope = Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(1).expect("positive sequence"),
        at: Instant::new("2030-04-12T18:44:09Z".parse().expect("fixture instant"))
            .expect("canonical instant"),
        actor: Actor::Spice,
        origin: format!("resident:{}", uuid(3))
            .parse()
            .expect("resident origin"),
        cause: None,
        fact: Fact {
            body: declaration(),
        },
    };
    let mut original = encode_with_hook(&envelope, "/fact/rtk");
    let mut document: Value = serde_json::from_slice(&original).expect("fixture JSON decodes");
    document["fact"]["rtk"]["hook"] = Value::String("future".to_owned());
    original = serde_json::to_vec(&document).expect("invalid fixture encodes");
    connection
        .execute(
            "INSERT INTO fact_envelopes VALUES (1, 'lane::declared', ?1)",
            [&original],
        )
        .expect("invalid schema-31 fact inserts");

    assert!(matches!(
        migrate(&connection),
        Err(OpenError::Migration(MigrationError::StoredContent {
            from: 31,
            to: 32,
            table: "fact_envelopes",
            ..
        }))
    ));
    assert_eq!(
        stored(
            &connection,
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            1_i64,
        ),
        original
    );
}
