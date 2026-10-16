//! Retained predecessor rows: shapes the crossing rewrites once into their
//! current spelling, and shapes with no current spelling that refuse by their
//! owning release without mutating the authority.

use super::*;

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
    restore_predecessor(&journal, SHAPE_PREDECESSOR_SCHEMA);
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
fn writable_open_binds_retained_design_notices_and_retires_provenance() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retained-design-notice.sqlite3");
    let journal = authority_file_fixture(&path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::DesignNoticed,
            json!({
                "design": "1kKRRsDP",
                "statement": 1,
                "lane": FIXTURE_LANE,
                "occupant": FIXTURE_LANE,
                "task": "1kKRRsDQ",
                "claim": "2",
                "terms": ["rooms", "state", "tasks"]
            }),
        ),
    );
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(
                 json_set(CAST(envelope AS TEXT), '$.fact.provenance', 'organic'),
                 '$.fact.statement'
             ) AS BLOB) WHERE seq = 3;
             UPDATE cursor_head SET seq = 3, wake = 3 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;",
        )
        .expect("retained notice cursor is planted");
    restore_predecessor(&journal, 62);
    drop(journal);

    let crossed = Journal::open(&path).expect("retained notice crosses");
    let envelope = crossed
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("crossed notice reads");
    let envelope: Envelope<AnyFact> = wire::decode(&envelope).expect("crossed notice decodes");
    let notice = envelope
        .fact
        .read::<wire::rooms::DesignNoticed>()
        .expect("crossed fact remains a notice");
    assert_eq!(notice.statement, seq(1));
    assert_eq!(notice.terms, ["rooms", "state", "tasks"]);
}

#[test]
fn writable_open_refuses_a_retained_notice_without_a_provable_statement() {
    let scratch = Scratch::new();
    let path = scratch.path().join("unproved-design-notice.sqlite3");
    let journal = authority_file_fixture(&path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::DesignNoticed,
            json!({
                "design": "1kKRRsDQ",
                "statement": 1,
                "lane": FIXTURE_LANE,
                "occupant": FIXTURE_LANE,
                "task": "1kKRRsDP",
                "claim": "2",
                "terms": ["rooms", "state", "tasks"]
            }),
        ),
    );
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(
                 json_set(CAST(envelope AS TEXT), '$.fact.provenance', 'organic'),
                 '$.fact.statement'
             ) AS BLOB) WHERE seq = 3",
            [],
        )
        .expect("unproved retained notice shape is planted");
    restore_predecessor(&journal, 62);
    drop(journal);

    let error = Journal::open(&path).expect_err("an unproved notice must refuse");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::RetainedRow {
            table: "fact_envelopes",
            row,
            release: "ROOMS-dW6GpYK65",
            ..
        }) if row == "3"
    ));
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
        Some((SHAPE_PREDECESSOR_SCHEMA, SCHEMA_VERSION)),
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
#[test]
fn writable_open_moves_hidden_oops_projects_into_explicit_filing_state() {
    for (name, retained, public) in [
        ("nested", ".oops.board.cli", "board.cli"),
        ("bare", ".oops", "task.oops"),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("retained-oops-{name}.sqlite3"));
        let journal = authority_file_fixture(&path);
        journal
            .connection
            .execute(
                "UPDATE fact_envelopes
                 SET envelope = CAST(json_set(
                     CAST(envelope AS TEXT), '$.fact.project', ?1
                 ) AS BLOB)
                 WHERE kind = 'task::added'",
                [retained],
            )
            .expect("retained OOPS fact is planted");
        journal
            .connection
            .execute(
                "UPDATE task_heads
                 SET body = CAST(json_set(
                     CAST(body AS TEXT), '$.project', ?1
                 ) AS BLOB)",
                [retained],
            )
            .expect("retained OOPS head is planted");
        restore_predecessor(&journal, SHAPE_PREDECESSOR_SCHEMA);
        drop(journal);

        let crossed = Journal::open(&path).expect("retained OOPS authority crosses");
        let head = crossed
            .connection
            .query_row(
                "SELECT json_extract(CAST(body AS TEXT), '$.project'),
                        json_extract(CAST(body AS TEXT), '$.filing')
                 FROM task_heads",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("crossed OOPS head reads");
        let fact = crossed
            .connection
            .query_row(
                "SELECT json_extract(CAST(envelope AS TEXT), '$.fact.project'),
                        json_extract(CAST(envelope AS TEXT), '$.fact.filing')
                 FROM fact_envelopes WHERE kind = 'task::added'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("crossed OOPS fact reads");
        assert_eq!(head, (public.to_owned(), "oops".to_owned()));
        assert_eq!(fact, head);
        crossed.verify().expect("crossed OOPS authority verifies");
    }
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

fn plant_room_packet(journal: &Journal) -> String {
    let packet = wire::rooms::RoomMessaged {
        room: "1kKRRsDP".parse().expect("fixture room"),
        publication: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("fixture publication"),
        hunk: wire::rooms::TranscriptHunk {
            head: transcript_source_head(),
            locus: wire::MessageLocus {
                member: 1,
                offset: 0,
                length: 64,
                event: 0,
            },
            text: wire::rooms::TextRange { start: 0, end: 8 },
        },
        author: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        directed: Vec::new(),
    };
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::RoomMessaged,
            serde_json::to_value(packet).expect("room packet encodes"),
        ),
    );
    "3".to_owned()
}

fn plant_room_coordination(journal: &Journal) -> String {
    let directed = wire::rooms::RoomCoordinationStarted {
        room: "1kKRRsDP".parse().expect("fixture room"),
        publication: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("fixture publication"),
        key: "dVsW5mRxg".to_owned(),
        target: "6SNy".parse().expect("fixture address"),
        rule: None,
        automated: None,
    };
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::RoomCoordinationStarted,
            serde_json::to_value(directed).expect("room coordination encodes"),
        ),
    );
    "3".to_owned()
}

fn plant_room_conversation(journal: &Journal) -> String {
    let posted = wire::rooms::RoomPosted {
        publication: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("fixture publication"),
        text: "heads up".to_owned(),
        operation: wire::rooms::RoomPublicationKind::Msg {},
        routes: vec![wire::rooms::RoomRoute {
            destination: wire::rooms::RoomDestination::Operator {},
            context: None,
            direction: None,
        }],
        directed: Vec::new(),
        expires: "2030-04-12T19:44:09.271Z".parse().expect("fixture expiry"),
    };
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::RoomPosted,
            serde_json::to_value(posted).expect("room post encodes"),
        ),
    );
    "3".to_owned()
}

#[test]
fn current_conversations_cross_a_predecessor_schema_without_changing_authority() {
    use wire::rooms::{
        QuestionResultRef, RoomAuthorityStamp, RoomDestination, RoomPosted, RoomPublicationKind,
        RoomRoute,
    };

    let mut journal = Journal::in_memory().expect("current journal opens");
    let result = QuestionResultRef::new(
        RoomAuthorityStamp::for_micros(1_760_000_000_000_000).expect("result identity"),
    );
    for (micros, operation) in [
        (1_760_000_000_000_001, RoomPublicationKind::Msg {}),
        (
            1_760_000_000_000_002,
            RoomPublicationKind::Ask {
                result,
                deadline: "2030-04-12T19:44:09.271Z".parse().expect("deadline"),
            },
        ),
    ] {
        let mut write = crate::JournalWrite::new(
            Actor::Operator,
            "task:rooms".parse().expect("origin"),
            instant(),
            wire::Producer::CommandDoor,
        );
        write
            .fact(
                RoomPosted {
                    publication: RoomAuthorityStamp::for_micros(micros).expect("publication"),
                    text: "operator conversation".to_owned(),
                    operation,
                    routes: vec![RoomRoute {
                        destination: RoomDestination::Operator {},
                        context: None,
                        direction: None,
                    }],
                    directed: Vec::new(),
                    expires: "2030-04-12T19:44:09.271Z".parse().expect("expiry"),
                },
                crate::FactCause::Root,
            )
            .expect("publication builds");
        journal.apply(write).expect("current publication commits");
    }
    let rows = |journal: &Journal| {
        journal
            .connection
            .prepare(
                "SELECT envelope AS bytes FROM fact_envelopes \
                 UNION ALL SELECT body FROM room_conversations \
                 UNION ALL SELECT record FROM room_items \
                 UNION ALL SELECT head FROM room_items \
                 UNION ALL SELECT record FROM room_results \
                 UNION ALL SELECT head FROM room_results ORDER BY bytes",
            )
            .expect("conversation authority query prepares")
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("conversation authority reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("conversation authority decodes")
    };
    let before = rows(&journal);
    journal.verify().expect("current conversation verifies");
    restore_predecessor(&journal, SCHEMA_VERSION - 1);

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("current conversations cross")
        .expect("predecessor crosses");
    assert_eq!(
        (migration.from(), migration.to()),
        (SCHEMA_VERSION - 1, SCHEMA_VERSION)
    );
    assert_eq!(rows(&journal), before);
    journal.verify().expect("crossed conversation replays");
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).unwrap(),
        None
    );
    assert_eq!(rows(&journal), before);
}

/// Plant one per-group `group::stored` fact holding `anchors` designs, as a
/// release before the partition wrote it.
fn plant_stored_group(journal: &Journal, anchors: usize) -> String {
    let group = wire::rooms::WorkingGroup {
        group: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("fixture group"),
        anchors: (1..=anchors)
            .map(|micros| wire::TaskStamp::for_micros(micros as i64).expect("fixture anchor"))
            .collect(),
        stance: wire::rooms::WorkingGroupStance::Explore,
    };
    let moved = wire::rooms::WorkingGroupMoved {
        design: group.anchors[0].clone(),
        from: None,
        to: Some(group.group.clone()),
        partition: wire::rooms::WorkingGroupPartition {
            revision: 1,
            groups: vec![group.clone()],
            fast: Vec::new(),
        },
    };
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::WorkingGroupMoved,
            serde_json::to_value(moved).expect("move encodes"),
        ),
    );
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET kind = 'group::stored', envelope = CAST(json_set( \
             json_remove(CAST(envelope AS TEXT), '$.fact.design', '$.fact.from', '$.fact.to', \
             '$.fact.partition'), '$.fact.kind', 'group::stored', '$.fact.group', json(?1)) \
             AS BLOB) WHERE seq = 3",
            [serde_json::to_string(&group).expect("group encodes")],
        )
        .expect("per-group fact is planted");
    "3".to_owned()
}

fn plant_overfull_group(journal: &Journal) -> String {
    plant_stored_group(journal, wire::rooms::WORKING_GROUP_DESIGN_LIMIT + 1)
}

fn plant_group(journal: &Journal) -> String {
    plant_stored_group(journal, 1)
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
        restore_predecessor(&journal, SHAPE_PREDECESSOR_SCHEMA);
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
    let cases: [RetainedCase; 6] = [
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
            "command_receipts",
            "370793eca",
            "UPDATE command_receipts SET command_kind = 'group::remove'",
            retained_receipt,
        ),
        (
            "command_receipts",
            "45eb87f84",
            "UPDATE command_receipts SET command_kind = 'group::put'",
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
    let cases: [RetainedCase; 10] = [
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
        (
            "fact_envelopes",
            "98a3243fa",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.author', '$.fact.directed') AS BLOB) WHERE seq = 3",
            plant_room_packet,
        ),
        ("fact_envelopes", "09d471040", "", plant_room_coordination),
        (
            "fact_envelopes",
            "d4f4dba9d",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.directed') AS BLOB) WHERE seq = 3",
            plant_room_conversation,
        ),
        ("fact_envelopes", "577f4f602", "", plant_overfull_group),
        (
            "fact_envelopes",
            "370793eca",
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.group.name', 'Named') AS BLOB) WHERE seq = 3",
            plant_group,
        ),
        ("fact_envelopes", "45eb87f84", "", plant_group),
    ];
    assert_retained_refusals(&cases);
}

#[test]
fn retained_lifecycle_facts_without_a_proven_boundary_refuse_without_mutation() {
    let cases: [RetainedCase; 2] = [
        (
            "fact_envelopes",
            "b18f556c3",
            "UPDATE fact_envelopes SET envelope = CAST(json_remove(CAST(envelope AS TEXT), \
             '$.fact.source') AS BLOB) WHERE seq = 3",
            plant_claim_without_current_spelling,
        ),
        (
            "fact_envelopes",
            "b18f556c3",
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.source', json('{\"incomplete\": {\"reason\": \"unrecorded\"}}')) AS BLOB) \
             WHERE seq = 3",
            plant_claim_without_current_spelling,
        ),
    ];
    assert_retained_refusals(&cases);
}
