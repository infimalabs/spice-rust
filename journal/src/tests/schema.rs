//! Schema identity, admission, and the read-only probe.
//!
//! Every test here judges a database this build did not necessarily write: an
//! empty file, a half-migrated one, a forward one, or one that is not SQLite at
//! all. What they share is that admission must answer by name and change
//! nothing, which is why they sit apart from the tests that assume an admitted
//! authority and go on to read it.

use super::*;

#[test]
fn current_schema_is_the_only_executable_journal_shape() {
    let mut journal = Journal::in_memory().expect("empty journal creates the current schema");
    let migration = journal
        .migration()
        .expect("empty journal records its baseline");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).expect("baseline is idempotent"),
        None
    );
    assert_eq!(authority_tables(&journal), expected_tables());
    let verified = journal.verify().expect("empty baseline verifies");
    assert_eq!(
        (
            verified.facts,
            verified.receipts,
            verified.heads,
            verified.threads
        ),
        (0, 0, 0, 0)
    );
}

/// Return a current fixture to a predecessor's shape and identity.
fn restore_predecessor(journal: &Journal, predecessor: u32) {
    let rooms = if predecessor < crate::migrate::ROOM_AUTHORITY_SCHEMA {
        "DROP TABLE room_publications; DROP TABLE group_anchors; DROP TABLE group_heads;
         DROP TABLE proposal_heads; DROP TABLE authorization_heads;"
    } else {
        ""
    };
    journal
        .connection
        .execute_batch(&format!(
            "{rooms}
             UPDATE journal_schema SET version = {predecessor} WHERE singleton = 1;
             PRAGMA user_version = {predecessor};",
        ))
        .expect("restore predecessor shape");
}

#[test]
fn predecessor_authority_crosses_once_to_the_current_shape_without_rewriting_rows() {
    for predecessor in [33, 34, 35, 36, 37, 38, 39] {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let journal = authority_file_fixture(&path);
        restore_predecessor(&journal, predecessor);
        let rows = |journal: &Journal| -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            let fact = journal
                .connection
                .query_row(
                    "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
                    [],
                    |row| row.get(0),
                )
                .expect("read retained task fact");
            let head = journal
                .connection
                .query_row("SELECT body FROM task_heads", [], |row| row.get(0))
                .expect("read retained task head");
            let residency = journal
                .connection
                .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
                .expect("read current residency head");
            (fact, head, residency)
        };
        let before = rows(&journal);
        drop(journal);

        let journal = Journal::open(&path).expect("predecessor authority crosses forward");
        let migration = journal.migration().expect("the crossing is reported");
        assert_eq!(
            (migration.from(), migration.to()),
            (predecessor, SCHEMA_VERSION)
        );
        assert_eq!(
            journal.schema_version().expect("schema reads"),
            SCHEMA_VERSION
        );
        assert_eq!(rows(&journal), before, "retained authority bytes stay put");
        let task = "1kKRRsDP".parse().expect("fixture task stamp");
        let head = journal
            .read(|read| read.task_head(&task))
            .expect("retained task head reads")
            .expect("retained task exists");
        assert_eq!(head.body().intent, None, "no intent is inferred");
        assert_eq!(head.body().preparation, None, "no preparation is inferred");
        journal.verify().expect("crossed authority verifies");
        assert_eq!(
            authority_tables(&journal),
            expected_tables(),
            "the crossing creates exactly the current projection"
        );
        drop(journal);

        let reopened = Journal::open(&path).expect("current authority reopens");
        assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
    }
}

#[test]
fn schema_39_projects_frozen_peer_rooms_into_steering_heads_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("peer-room.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let room: TaskStamp = "1kKRRsDP".parse().expect("fixture room");
    let directive = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    let lane = uuid(FIXTURE_LANE);
    let sender = uuid("018f82d4-6f59-7d06-92e3-bce913116304");
    let at = instant();
    let expires: Instant = "2030-04-12T19:44:09.271Z".parse().expect("fixture expiry");
    let origin: Origin = "resident:018f82d4-6f59-7d06-92e3-bce913116305"
        .parse()
        .expect("fixture origin");
    let message = wire::body::SteerMessaged {
        directive,
        key: "1kJK6N3R".to_owned(),
        lane,
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116306"),
        from: lane,
        sender,
        text: "room-bound message".to_owned(),
        locus: wire::body::TranscriptLocus {
            source: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116307")),
            member: 1,
            offset: 32,
            length: 64,
            event: 0,
            line: 1,
            column: 1,
        },
        room: Some(room.clone()),
        expires,
    };
    let mut write = crate::JournalWrite::new(
        Actor::Lane(lane),
        origin.clone(),
        at,
        wire::Producer::AckScanner,
    );
    let created = write
        .fact(message.clone(), crate::FactCause::Root)
        .expect("message fact builds");
    write.steering_head(created, move |created| wire::SteeringHead {
        directive,
        fanout: None,
        key: message.key,
        lane,
        text: message.text,
        from: Some(message.from),
        room: message.room,
        control: None,
        attachments: Vec::new(),
        published: at,
        expires,
        stage: wire::body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: wire::Disposition::Live,
        origin,
        created,
        minted_micros: None,
    });
    journal.apply(write).expect("peer message commits");
    journal
        .connection
        .execute(
            "UPDATE steering_heads SET body = CAST(json_remove(CAST(body AS TEXT), '$.room') AS BLOB) \
             WHERE directive = ?1",
            [directive.to_string()],
        )
        .expect("restore schema-39 steering head");
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    drop(journal);

    let crossed = Journal::open(&path).expect("schema-39 peer room crosses");
    assert_eq!(
        crossed
            .read(|read| read.steering_head(directive))
            .expect("steering head reads")
            .expect("peer message head exists")
            .body()
            .room
            .as_ref(),
        Some(&room)
    );
    crossed
        .verify()
        .expect("crossed peer-room authority verifies");
    drop(crossed);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the room crosses exactly once");
}

#[test]
fn a_predecessor_omitting_task_creation_context_refuses_before_crossing() {
    for (table, omit) in [
        (
            "fact_envelopes",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), '$.fact.creation') AS BLOB) WHERE kind = 'task::added'",
        ),
        (
            "task_heads",
            "UPDATE task_heads SET body = CAST(json_remove(CAST(body AS TEXT), '$.creation') AS BLOB)",
        ),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let journal = authority_file_fixture(&path);
        journal
            .connection
            .execute_batch(&format!(
                "{omit};
                 UPDATE journal_schema SET version = 35 WHERE singleton = 1;
                 PRAGMA user_version = 35;"
            ))
            .expect("plant a retained omission");
        drop(journal);
        let authority = || fs::read(&path).expect("authority bytes read");
        let before = authority();

        let error = Journal::open(&path).expect_err("an omission refuses the crossing");
        let OpenError::Migration(MigrationError::CreationContextOmitted { table: named, .. }) =
            &error
        else {
            panic!("{table}: expected a creation-context refusal, got {error}");
        };
        assert_eq!(named, table);
        assert!(error.to_string().contains("6ad57c713"), "{error}");
        assert_eq!(authority(), before, "{table}: the refusal changes nothing");
    }
}

fn residency_lane(lane: &str, team: &str, life: &str) -> LaneResidency {
    LaneResidency {
        lane: uuid(lane),
        team: uuid(team),
        life: life.to_owned(),
        ..residency_head().lanes.remove(0)
    }
}

/// Plant an immediate-predecessor residency head written before its table.
fn plant_omitted_subscriptions(journal: &Journal, lanes: Vec<LaneResidency>) {
    let mut head = serde_json::to_value(ResidencyHead {
        lanes,
        ..residency_head()
    })
    .expect("residency head encodes");
    head.as_object_mut()
        .expect("residency head is an object")
        .remove("subscriptions");
    journal
        .connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![serde_json::to_vec(&head).expect("omitting head encodes")],
        )
        .expect("plant an omitted subscription table");
    restore_predecessor(journal, SCHEMA_VERSION - 1);
}

#[test]
fn an_omitted_residency_subscription_table_crosses_once_to_its_explicit_form() {
    const EARLY: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
    const LATE: &str = "018f82d4-6f59-7d06-92e3-bce913116309";
    let subscription = |team, policy, fast_mode| wire::TeamSubscription {
        team: uuid(team),
        policy,
        fast_mode,
    };
    let cases = [
        (
            vec![
                residency_lane("018f82d4-6f59-7d06-92e3-bce913116311", LATE, "steer"),
                residency_lane("018f82d4-6f59-7d06-92e3-bce913116312", EARLY, "drive"),
                residency_lane("018f82d4-6f59-7d06-92e3-bce913116313", EARLY, "drive"),
            ],
            vec![
                subscription(EARLY, wire::intent::TeamLife::Drive, false),
                subscription(LATE, wire::intent::TeamLife::Steer, false),
            ],
        ),
        (Vec::new(), Vec::new()),
    ];
    for (lanes, subscriptions) in cases {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let journal = authority_file_fixture(&path);
        plant_omitted_subscriptions(&journal, lanes.clone());
        let head_seq = |journal: &Journal| -> i64 {
            journal
                .connection
                .query_row("SELECT head_seq FROM residency_head", [], |row| row.get(0))
                .expect("residency position reads")
        };
        let position = head_seq(&journal);
        drop(journal);

        let journal = Journal::open(&path).expect("an omitted table crosses forward");
        assert_eq!(
            journal.migration().map(|migration| migration.from()),
            Some(SCHEMA_VERSION - 1)
        );
        let expected = ResidencyHead {
            lanes,
            subscriptions,
            ..residency_head()
        };
        let stored: Vec<u8> = journal
            .connection
            .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
            .expect("crossed residency reads");
        assert_eq!(
            stored,
            wire::encode(&expected).expect("expected head encodes"),
            "the crossed head is the canonical explicit table"
        );
        assert_eq!(head_seq(&journal), position, "the head keeps its position");
        journal.verify().expect("crossed authority verifies");
        drop(journal);

        let reopened = Journal::open(&path).expect("crossed authority reopens");
        assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
        let again: Vec<u8> = reopened
            .connection
            .query_row("SELECT body FROM residency_head", [], |row| row.get(0))
            .expect("reopened residency reads");
        assert_eq!(again, stored, "reopening leaves the explicit table put");
    }
}

#[test]
fn an_omitted_residency_table_without_one_closed_team_life_refuses_atomically() {
    const TEAM: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
    for lanes in [
        vec![
            residency_lane("018f82d4-6f59-7d06-92e3-bce913116311", TEAM, "drive"),
            residency_lane("018f82d4-6f59-7d06-92e3-bce913116312", TEAM, "steer"),
        ],
        vec![residency_lane(
            "018f82d4-6f59-7d06-92e3-bce913116311",
            TEAM,
            "wander",
        )],
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let journal = authority_file_fixture(&path);
        plant_omitted_subscriptions(&journal, lanes);
        drop(journal);
        let authority = || fs::read(&path).expect("authority bytes read");
        let before = authority();

        let error = Journal::open(&path).expect_err("an underivable table refuses");
        let OpenError::Migration(MigrationError::RetainedRow {
            table,
            row,
            release,
            ..
        }) = &error
        else {
            panic!("expected a residency refusal, got {error}");
        };
        assert_eq!(
            (*table, row.as_str(), *release),
            ("residency_head", "1", "4047a2a4f")
        );
        assert_eq!(authority(), before, "the refusal changes nothing");
    }
}

#[test]
fn writable_open_crosses_retired_gate_facts_into_inert_current_evidence_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retained-gate.sqlite3");
    let journal = authority_file_fixture(&path);
    let retired = json!({
        "kind": "gate::filed",
        "report": "a".repeat(64),
        "findings": 3,
        "created": 2,
        "reused": 1,
    });
    let envelope = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(3),
        at: instant(),
        actor: Actor::Spice,
        origin: "task:1kKRRsDP".parse::<Origin>().expect("fixture origin"),
        cause: Some(seq(2)),
        fact: retired.clone(),
    };
    let bytes = wire::encode(&envelope).expect("retired envelope encodes");
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes(
                seq, wire_version, instant, actor, origin, cause, kind, envelope
             ) VALUES (3, 1, ?1, 'spice', 'task:1kKRRsDP', 2, 'gate::filed', ?2)",
            params![envelope.at.to_string(), bytes],
        )
        .expect("retired fact is planted");
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (3, 'task:1kKRRsDP', 2, 1, 2)",
            [],
        )
        .expect("retired fact thread is planted");
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 3, wake = 3 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;",
        )
        .expect("planted cursor is indexed");
    restore_predecessor(&journal, 35);
    drop(journal);

    let journal = Journal::open(&path).expect("retained gate fact crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((35, SCHEMA_VERSION)),
    );
    assert_eq!(
        journal
            .connection
            .query_row("SELECT kind FROM fact_envelopes WHERE seq = 3", [], |row| {
                row.get::<_, String>(0)
            },)
            .expect("crossed kind reads"),
        "journal::retained",
    );
    let snapshot = journal.fact_snapshot().expect("crossed facts read");
    let retained = snapshot.facts()[2]
        .fact
        .read::<wire::body::JournalRetained>()
        .expect("retired payload became current inert evidence");
    assert_eq!(retained.release, "595da8d81");
    assert_eq!(retained.fact, retired);
    journal.verify().expect("crossed authority verifies");
    let crossed = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("crossed envelope reads");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(
        reopened
            .connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = 3",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .expect("reopened envelope reads"),
        crossed,
        "the retained crossing runs exactly once",
    );
}

const RETIRED_GUIDANCE: &str = "phase ownership: this advance released your claim; only the held plan phase was completed. Run spice task next for another assignment; it may select any eligible work and does not guarantee this task's next phase.";

/// A schema-36 authority, already carrying the room projection, whose
/// completion fact and its receipt carry guidance, with the canonical current
/// bytes of both rows before guidance was planted.
fn retained_guidance_fixture(path: &Path, guidance: &str) -> (Vec<u8>, Vec<u8>) {
    let journal = authority_file_fixture(path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::TaskDone,
            json!({
                "task": "1kKRRsDP",
                "validation": "fixture",
                "terminal": {
                    "operation": "018f82d4-6f59-7d06-92e3-bce9131163c4",
                    "status": "completed",
                    "next": "spice task next",
                },
            }),
        ),
    );
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (3, 'task:1kKRRsDP', 2, 1, 2);
             UPDATE cursor_head SET seq = 3, wake = 3 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;",
        )
        .expect("completion fact thread is planted");
    let canonical = guidance_rows(&journal);
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.terminal.guidance', ?1) AS BLOB) WHERE seq = 3",
            [guidance],
        )
        .expect("completion guidance is planted");
    journal
        .connection
        .execute(
            "UPDATE command_receipts SET response = CAST(json_set(CAST(response AS TEXT), \
             '$.outcome.body.guidance', ?1) AS BLOB)",
            [guidance],
        )
        .expect("receipt guidance is planted");
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 36 WHERE singleton = 1;
             PRAGMA user_version = 36;",
        )
        .expect("predecessor identity is restored");
    drop(journal);
    canonical
}

fn guidance_rows(journal: &Journal) -> (Vec<u8>, Vec<u8>) {
    let row = |sql: &str| {
        journal
            .connection
            .query_row(sql, [], |row| row.get::<_, Vec<u8>>(0))
            .expect("guidance-bearing row reads")
    };
    (
        row("SELECT envelope FROM fact_envelopes WHERE seq = 3"),
        row("SELECT response FROM command_receipts"),
    )
}

#[test]
fn writable_open_drops_retired_task_guidance_from_completions_and_receipts_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retained-guidance.sqlite3");
    let canonical = retained_guidance_fixture(&path, RETIRED_GUIDANCE);

    let journal = Journal::open(&path).expect("retained guidance crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((36, SCHEMA_VERSION)),
    );
    assert_eq!(
        guidance_rows(&journal),
        canonical,
        "each row keeps every other field in its canonical current encoding",
    );
    journal.verify().expect("crossed authority verifies");
    let (_, receipt) = guidance_rows(&journal);
    let response: Response<serde_json::Value> =
        wire::decode(&receipt).expect("the crossed response decodes");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("a completion receipt holds a reply");
    };
    let answer = wire::encode(&reply.body)
        .and_then(|body| CommandKind::TaskDone.binding().answer.decode(&body))
        .expect("the crossed answer decodes as an exact retry would");
    let wire::intent::CommandAnswer::Task(answer) = answer else {
        panic!("a completion receipt replays a task answer");
    };
    assert_eq!(answer.status, "review");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(guidance_rows(&reopened), canonical);
}

#[test]
fn guidance_other_than_the_retired_spelling_refuses_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("foreign-guidance.sqlite3");
    retained_guidance_fixture(&path, "same-task-next-phase");
    let before = fs::read(&path).expect("authority bytes read");

    let error = Journal::open(&path).expect_err("unknown guidance refuses the crossing");
    assert!(
        matches!(
            &error,
            OpenError::Migration(MigrationError::RetainedRow {
                table: "fact_envelopes",
                row,
                ..
            }) if row == "3"
        ),
        "{error}"
    );
    assert!(error.to_string().contains("979b72704"), "{error}");
    assert_eq!(fs::read(&path).expect("authority bytes reread"), before);
}

const FIXTURE_LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";

/// A predecessor authority whose task edit carries a retired claimant notice
/// and whose fast retune omits its prior flag, with the canonical current bytes
/// both rows cross to.
fn retained_residual_fixture(path: &Path) -> (Vec<u8>, Vec<u8>) {
    let journal = authority_file_fixture(path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::TaskEdited,
            json!({"task": "1kKRRsDP", "revision": 2}),
        ),
    );
    insert_fact(
        &journal,
        fact(
            4,
            Some(3),
            FactKind::LaneFastRetuned,
            json!({"lane": FIXTURE_LANE, "from": false, "fast_mode": true}),
        ),
    );
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (3, 'task:1kKRRsDP', 2, 1, 2), (4, 'task:1kKRRsDP', 3, 1, 3);
             UPDATE cursor_head SET seq = 4, wake = 4 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 4 WHERE singleton = 1;",
        )
        .expect("residual fact threads are planted");
    let canonical = residual_rows(&journal);
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.notice', json('{{\"lane\": \"{FIXTURE_LANE}\", \"claim\": 1}}')) AS BLOB) \
             WHERE seq = 3;
             UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.from') AS BLOB) WHERE seq = 4;"
        ))
        .expect("retired shapes are planted");
    restore_predecessor(&journal, SCHEMA_VERSION - 1);
    canonical
}

fn residual_rows(journal: &Journal) -> (Vec<u8>, Vec<u8>) {
    let row = |seq: i64| {
        journal
            .connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
                [seq],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .expect("residual fact reads")
    };
    (row(3), row(4))
}

#[test]
fn writable_open_drops_task_edit_notices_and_spells_implicit_fast_flags_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retained-residual.sqlite3");
    let canonical = retained_residual_fixture(&path);

    let journal = Journal::open(&path).expect("retained shapes cross on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((SCHEMA_VERSION - 1, SCHEMA_VERSION)),
    );
    assert_eq!(
        residual_rows(&journal),
        canonical,
        "the notice is gone, the absent flag is an explicit false, and every other \
         field keeps its canonical current encoding",
    );
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None);
    assert_eq!(residual_rows(&reopened), canonical);
}

type RetainedPlant = fn(&Journal) -> String;
type RetainedCase = (&'static str, &'static str, &'static str, RetainedPlant);

fn plant_claim_without_current_spelling(journal: &Journal) -> String {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::ClaimWon,
            json!({
                "task": "1kKRRsDP",
                "actor": "operator",
                "lane": FIXTURE_LANE,
                "occupant": "018f82d4-6f59-7d06-92e3-bce913116304",
                "epoch": "018f82d4-6f59-7d06-92e3-bce913116305",
                "cell": {"project": "journal", "phase": "todo"},
                "until": "2030-04-12T18:44:09.271Z",
                "source": {"proven": {"heads": []}},
            }),
        ),
    );
    "3".to_owned()
}

fn plant_release_without_current_spelling(journal: &Journal) -> String {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::ReleasePublished,
            json!({
                "manifest": "4a1b9c2d3e5f60718293a4b5c6d7e8f90123456789abcdef0123456789abcdef",
                "notes": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "targets": [],
            }),
        ),
    );
    "3".to_owned()
}

fn retained_receipt(journal: &Journal) -> String {
    journal
        .connection
        .query_row("SELECT command_id FROM command_receipts", [], |row| {
            row.get(0)
        })
        .expect("fixture receipt reads")
}

fn plant_vendor_hold(journal: &Journal) -> String {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::LaneHeld,
            json!({
                "lane": FIXTURE_LANE,
                "evidence": "fixture ended on a vendor error: refused",
                "until": "2030-04-12T18:44:09.271Z",
                "vendor_failure": {
                    "session": "fixture-session",
                    "directives": [FIXTURE_LANE],
                    "renewal": 1,
                },
            }),
        ),
    );
    "3".to_owned()
}

fn plant_lane_session(journal: &Journal) -> String {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::LaneSessionDeclared,
            json!({
                "lane": FIXTURE_LANE,
                "occupant": "018f82d4-6f59-7d06-92e3-bce913116304",
                "worktree": "/fixture/worktree",
                "vendor": "omp",
                "model": "sonnet",
                "effort": "high",
                "epoch": "018f82d4-6f59-7d06-92e3-bce913116305",
                "stream": "session",
                "source": "018f82d4-6f59-7d06-92e3-bce913116306",
                "identity": "fixture-session",
                "member": 1,
                "offset": 10,
                "length": 20,
            }),
        ),
    );
    "3".to_owned()
}

fn plant_residency_session(journal: &Journal) -> String {
    journal
        .connection
        .execute(
            r#"UPDATE residency_head SET body = CAST(json_set(
               CAST(body AS TEXT),
               '$.lanes[0].session',
               json('{
                 "lane":"018f82d4-6f59-7d06-92e3-bce913116302",
                 "occupant":"018f82d4-6f59-7d06-92e3-bce913116304",
                 "worktree":"/fixture/worktree",
                 "vendor":"omp",
                 "model":"sonnet",
                 "effort":"high",
                 "epoch":"018f82d4-6f59-7d06-92e3-bce913116305",
                 "stream":"session",
                 "identity":"fixture-session",
                 "locus":{"member":1,"offset":10,"length":20}
               }')
             ) AS BLOB)"#,
            [],
        )
        .expect("current resumable session is planted");
    "1".to_owned()
}

fn plant_steering_locus(journal: &Journal) -> String {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::SteerAcked,
            json!({
                "directive": "018f82d4-6f59-7d06-92e3-bce913116307",
                "key": "dFixture",
                "lane": FIXTURE_LANE,
                "epoch": "018f82d4-6f59-7d06-92e3-bce913116305",
                "locus": {
                    "source": "018f82d4-6f59-7d06-92e3-bce913116306",
                    "member": 1,
                    "offset": 10,
                    "length": 20,
                    "event": 0,
                    "line": 1,
                    "column": 1,
                },
            }),
        ),
    );
    "3".to_owned()
}

fn plant_millisecond_steering_head(journal: &Journal) -> String {
    journal
        .connection
        .execute(
            "INSERT INTO steering_heads(directive, steer_key, lane, disposition, \
             created_seq, minted_micros, head_seq, body) \
             VALUES (?1, 'fixture', ?2, 'pending', 1, NULL, 1, \
             CAST('{\"minted_millis\": 1}' AS BLOB))",
            [FIXTURE_LANE, FIXTURE_LANE],
        )
        .expect("steering head is planted");
    FIXTURE_LANE.to_owned()
}

fn assert_retained_refusals(cases: &[RetainedCase]) {
    for &(table, release, retire, place) in cases {
        let scratch = Scratch::new();
        let path = scratch.path().join("retained.sqlite3");
        let journal = authority_file_fixture(&path);
        let expected = place(&journal);
        journal
            .connection
            .execute_batch(retire)
            .expect("retired shape is planted");
        restore_predecessor(&journal, SCHEMA_VERSION - 1);
        drop(journal);
        let before = fs::read(&path).expect("authority bytes read");

        let error = Journal::open(&path).expect_err("a shape without a current spelling refuses");
        assert!(
            matches!(
                &error,
                OpenError::Migration(MigrationError::RetainedRow {
                    table: refused,
                    row,
                    release: owner,
                    ..
                }) if *refused == table && *row == expected && *owner == release
            ),
            "{table}: {error}"
        );
        assert_eq!(fs::read(&path).expect("authority bytes reread"), before);
    }
}

#[test]
fn retained_scalar_shapes_without_a_current_spelling_refuse_without_mutation() {
    let cases: [RetainedCase; 4] = [
        (
            "fact_envelopes",
            "ea3fe662a",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.cell') AS BLOB) WHERE seq = 3",
            plant_claim_without_current_spelling,
        ),
        (
            "fact_envelopes",
            "987f0d60b",
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.notes', json('null')) AS BLOB) WHERE seq = 3",
            plant_release_without_current_spelling,
        ),
        (
            "command_receipts",
            "987f0d60b",
            "UPDATE command_receipts SET command_kind = 'release::publish', \
             request = CAST(json_set(CAST(request AS TEXT), '$.body.notes', json('null')) AS BLOB)",
            retained_receipt,
        ),
        (
            "steering_heads",
            "d811711a3",
            "",
            plant_millisecond_steering_head,
        ),
    ];
    assert_retained_refusals(&cases);
}

#[test]
fn retained_runtime_authority_without_a_current_spelling_refuses_without_mutation() {
    let cases: [RetainedCase; 5] = [
        (
            "fact_envelopes",
            "3903331e5",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.vendor_failure') AS BLOB) WHERE seq = 3",
            plant_vendor_hold,
        ),
        (
            "fact_envelopes",
            "a4abe071f",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.source') AS BLOB) WHERE seq = 3",
            plant_lane_session,
        ),
        (
            "fact_envelopes",
            "9f2487d5a",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.model') AS BLOB) WHERE seq = 3",
            plant_lane_session,
        ),
        (
            "residency_head",
            "9f2487d5a",
            "UPDATE residency_head SET body = CAST(json_remove(CAST(body AS TEXT), \
             '$.lanes[0].session.model') AS BLOB)",
            plant_residency_session,
        ),
        (
            "fact_envelopes",
            "cb609bd56",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.locus.source') AS BLOB) WHERE seq = 3",
            plant_steering_locus,
        ),
    ];
    assert_retained_refusals(&cases);
}

#[test]
fn schema_admission_requires_trusted_schema_to_be_disabled_before_inspection() {
    let mut connection = Connection::open_in_memory().expect("open empty fixture");
    let trusted: bool = connection
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .expect("default trust posture reads");
    assert!(trusted);

    let error = crate::migrate::admit(&mut connection)
        .expect_err("unhardened connection refuses schema inspection");

    assert!(matches!(error, OpenError::TrustedSchemaEnabled));
    let objects: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("empty fixture remains inspectable");
    assert_eq!(objects, 0, "refusal initialized no schema objects");

    connection
        .pragma_update(None, "trusted_schema", false)
        .expect("disable trusted schema");
    let migration = crate::migrate::admit(&mut connection)
        .expect("hardened empty connection initializes")
        .expect("initialization is recorded");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
}

#[test]
fn every_retired_schema_refuses_and_names_its_last_migration_owner() {
    let owners = [
        (1, "122d2a842"),
        (2, "IDENTIT-1kLGl2S5"),
        (3, "IDENTIT-1kLGl2S5"),
        (4, "b6acb226d"),
        (5, "d0c14dcc4"),
        (6, "SESSION-1kLLCyqw"),
        (7, "MINING-1kLNTy1C"),
        (8, "MINING-1kLNTy1C"),
        (9, "87636d7af"),
        (10, "1kRNj6X7"),
        (11, "257200978"),
        (12, "6ad57c713"),
        (13, "d811711a3"),
        (14, "ENVIRON-dRyzKgHJw"),
        (15, "SCHEMAR-dRyzNKsdp"),
        (16, "b9a45a6ec"),
        (17, "37cc1297f"),
        (18, "ALLOCAT-dS8cvNplX"),
        (19, "BOUNDS-dSHsMK0kz"),
        (20, "eab8365a6"),
        (21, "d431530eb"),
        (22, "c64c48cac"),
        (23, "TASKS-dSPJyjNSH"),
        (24, "TASKS-dSPW61MXd"),
        (25, "fb96c12a7"),
        (26, "966d33bef"),
        (27, "916081f8a"),
        (28, "02715f9e1"),
        (29, "SPEECH-dTQBpHHJq"),
        (30, "a4abe071f"),
        (31, "55ad00080"),
        (32, "a15ed13f4"),
    ];

    for (version, owner) in owners {
        let error =
            Journal::from_connection(schema_fixture(version, "finalized", None, None, None))
                .expect_err("retired schema refuses write open");
        assert!(matches!(
            error,
            OpenError::Migration(MigrationError::UnknownVersion { found }) if found == version
        ));
        let refusal = error.to_string();
        assert!(
            refusal.contains(&format!(
                "schema {version} was last migrated by release {owner}"
            )),
            "retired schema {version} did not name {owner}: {refusal}"
        );
        assert!(
            refusal.contains(&format!("reach schema {SCHEMA_VERSION}")),
            "retired schema {version} did not name the current cutoff: {refusal}"
        );
    }
}
#[test]
fn retired_file_refuses_before_wal_or_any_storage_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retired.sqlite3");
    drop(schema_file_fixture(
        &path,
        11,
        "finalized",
        None,
        None,
        None,
    ));
    let connection = Connection::open(&path).expect("retired fixture opens directly");
    let mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("retired fixture mode reads");
    assert_eq!(mode, "delete");
    drop(connection);
    let before_file = FileState::read(&path);
    let before_entries = directory_entries(scratch.path());
    let before_directory = directory_bytes(scratch.path());

    let error = Journal::open(&path).expect_err("retired file refuses write open");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 11 })
    ));
    assert_eq!(FileState::read(&path), before_file);
    assert_eq!(directory_entries(scratch.path()), before_entries);
    assert_eq!(directory_bytes(scratch.path()), before_directory);
}

#[test]
fn prepared_and_forward_schema_states_are_refused_by_name() {
    let prepared = schema_fixture(1, "prepared", Some(0), Some(1), Some("plan:baseline"));
    let error = Journal::from_connection(prepared).expect_err("prepared migration refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::Unfinalized {
            from: Some(0),
            to: Some(1)
        })
    ));

    let forward = schema_fixture(SCHEMA_VERSION + 1, "finalized", None, None, None);
    let error = Journal::from_connection(forward).expect_err("forward schema refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::ForwardSchema {
            found,
            current: SCHEMA_VERSION
        })
        if found == SCHEMA_VERSION + 1
    ));
}

#[test]
fn unknown_states_and_occupied_databases_are_refused_by_name() {
    // These two refusals exist for databases this build did not write, so the
    // baseline table's own CHECK constraints cannot stand in for them: a
    // foreign file reaches `finalized_version` carrying whatever it likes.
    let unknown = schema_fixture(1, "migrating", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown state refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownState { .. })
    ));

    let occupied = Connection::open_in_memory().expect("open occupied fixture");
    occupied
        .execute_batch("CREATE TABLE somebody_elses_table(id INTEGER PRIMARY KEY) STRICT;")
        .expect("occupy the fixture database");
    let error =
        Journal::from_connection(occupied).expect_err("an occupied database refuses baselining");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnversionedAuthority { object_count: 1 })
    ));
}

#[test]
fn malformed_schema_identity_is_refused_by_name() {
    let duplicate = schema_fixture(1, "finalized", None, None, None);
    duplicate
        .execute(
            "INSERT INTO journal_schema(singleton, version, state) VALUES (2, 1, 'finalized')",
            [],
        )
        .expect("insert duplicate schema row");
    let error = Journal::from_connection(duplicate).expect_err("two schema rows refuse open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::SchemaRowCount { count: 2 })
    ));

    let missing = schema_fixture(1, "finalized", None, None, None);
    missing
        .execute("UPDATE journal_schema SET singleton = 2", [])
        .expect("move schema row away from singleton one");
    let error = Journal::from_connection(missing).expect_err("missing singleton refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSingleton)
    ));

    let unknown = schema_fixture(0, "finalized", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown version refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 0 })
    ));

    let disagreement = schema_fixture(SCHEMA_VERSION, "finalized", None, None, None);
    disagreement
        .pragma_update(None, "user_version", 0)
        .expect("make pragma disagree with schema row");
    let error =
        Journal::from_connection(disagreement).expect_err("version disagreement refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::VersionDisagreement {
            row: SCHEMA_VERSION,
            pragma: 0
        })
    ));
    assert!(error.to_string().contains(&format!(
        "row names version {SCHEMA_VERSION}, but SQLite user_version names 0"
    )));
}

#[test]
fn read_only_handle_reports_schema_and_cursor_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("journal.sqlite3");
    drop(authority_file_fixture(&path));
    let before = FileState::read(&path);

    let reader = JournalReader::open(&path).expect("finalized authority opens read-only");

    assert_eq!(
        reader.schema_version().expect("read schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(reader.cursor().expect("read cursor"), Seq::new(2));
    let query_only: bool = reader
        .connection
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .expect("query-only posture");
    assert!(query_only);
    let write = reader
        .connection
        .execute("UPDATE cursor_head SET wake = seq WHERE singleton = 1", [])
        .expect_err("SQLite rejects a write through the reader");
    assert_eq!(
        write.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ReadOnly)
    );
    drop(reader);

    assert_eq!(FileState::read(&path), before);

    let empty = scratch.path().join("empty.sqlite3");
    drop(Journal::open(&empty).expect("create empty finalized authority"));
    let before = FileState::read(&empty);

    let reader = JournalReader::open(&empty).expect("empty finalized authority opens read-only");

    assert_eq!(reader.cursor().expect("read empty cursor"), None);
    drop(reader);
    assert_eq!(FileState::read(&empty), before);
}

#[test]
fn read_only_admission_refuses_schema_states_without_mutation() {
    let scratch = Scratch::new();
    let cases = [
        (
            "prepared.sqlite3",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
        ),
        (
            "newer.sqlite3",
            SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            None,
        ),
        ("predecessor.sqlite3", 33, "finalized", None, None, None),
        ("older.sqlite3", 0, "finalized", None, None, None),
    ];

    for (name, version, state, from, to, digest) in cases {
        let path = scratch.path().join(name);
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = FileState::read(&path);

        let error = JournalReader::open(&path).expect_err("schema state refuses read-only open");

        assert!(matches!(error, OpenError::Migration(_)));
        assert_eq!(FileState::read(&path), before, "{name} changed on refusal");
    }
}

#[test]
fn read_only_admission_never_creates_or_repairs_an_authority() {
    let scratch = Scratch::new();
    let absent = scratch.path().join("absent.sqlite3");
    let entries_before = directory_entries(scratch.path());

    let error = JournalReader::open(&absent).expect_err("absent authority refuses open");

    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &absent));
    assert!(!absent.exists());
    assert_eq!(directory_entries(scratch.path()), entries_before);

    let malformed = scratch.path().join("malformed.sqlite3");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = FileState::read(&malformed);

    let error = JournalReader::open(&malformed).expect_err("malformed authority refuses open");

    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(FileState::read(&malformed), before);
}

#[test]
fn schema_probe_reads_finalized_identity_without_changing_the_store() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.db");
    drop(Journal::open(&path).expect("create finalized authority"));
    let before = directory_bytes(scratch.path());

    assert_eq!(
        Journal::inspect_schema(&path).expect("inspect finalized authority"),
        SCHEMA_VERSION
    );
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn stored_schema_probe_observes_finalized_versions_without_admitting_them() {
    for version in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("schema-{version}.db"));
        drop(schema_file_fixture(
            &path,
            version,
            "finalized",
            None,
            None,
            None,
        ));
        let before = directory_bytes(scratch.path());

        assert_eq!(
            Journal::inspect_stored_schema(&path).expect("stored version observes"),
            version
        );
        assert_eq!(directory_bytes(scratch.path()), before);
    }
}

#[test]
fn stored_schema_probe_still_refuses_unfinalized_state_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("prepared.db");
    drop(schema_file_fixture(
        &path,
        SCHEMA_VERSION - 1,
        "prepared",
        Some(SCHEMA_VERSION - 1),
        Some(SCHEMA_VERSION),
        Some("plan:upgrade"),
    ));
    let before = directory_bytes(scratch.path());

    let error = Journal::inspect_stored_schema(&path).expect_err("prepared state refuses");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::Unfinalized { .. })
    ));
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn schema_probe_refuses_unfinalized_and_forward_state_without_changing_the_store() {
    for (name, version, state, from, to, digest, expected) in [
        (
            "prepared",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
            MigrationError::Unfinalized {
                from: Some(0),
                to: Some(1),
            },
        ),
        (
            "forward",
            SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            None,
            MigrationError::ForwardSchema {
                found: SCHEMA_VERSION + 1,
                current: SCHEMA_VERSION,
            },
        ),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("{name}.db"));
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = directory_bytes(scratch.path());

        let error = Journal::inspect_schema(&path).expect_err("schema state refuses");
        assert!(
            matches!(error, OpenError::Migration(ref found) if *found == expected),
            "unexpected schema refusal: {error}"
        );
        assert_eq!(directory_bytes(scratch.path()), before);
    }
}

#[test]
fn schema_probe_distinguishes_absence_from_sqlite_corruption_and_creates_nothing() {
    let scratch = Scratch::new();
    let missing = scratch.path().join("missing.db");

    let error = Journal::inspect_schema(&missing).expect_err("missing authority refuses");
    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &missing));
    assert!(directory_bytes(scratch.path()).is_empty());

    let empty = scratch.path().join("empty.db");
    drop(Connection::open(&empty).expect("create empty SQLite fixture"));
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&empty).expect_err("missing schema identity refuses");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSchema)
    ));
    assert_eq!(directory_bytes(scratch.path()), before);

    let malformed = scratch.path().join("malformed.db");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&malformed).expect_err("malformed SQLite refuses");
    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(directory_bytes(scratch.path()), before);
}
