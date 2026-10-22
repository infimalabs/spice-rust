use super::*;

const DESIGN: &str = "1kKRRsDQ";

/// Schema 88 reviews carried no workflow status. The crossing reads
/// `completed` only from a clean final review of a positively settled
/// non-Design task: an earlier clean review on the same task and a clean
/// review that advanced a Design to its design phase read `pending`.
#[test]
fn retained_reviews_take_the_workflow_status_their_verdict_left() {
    let scratch = Scratch::new();
    let path = scratch.path().join("review-status.sqlite3");
    let journal = authority_file_fixture(&path);
    restore_predecessor(&journal, 88);
    plant_design(&journal);
    settle_fixture_task(&journal);
    plant_review(&journal, 3, "1kKRRsDP", 2);
    plant_review(&journal, 4, "1kKRRsDP", 3);
    plant_review(&journal, 6, DESIGN, 5);
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 6, wake = 6 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 6 WHERE singleton = 1;",
        )
        .expect("planted cursor is indexed");
    drop(journal);

    let journal = Journal::open(&path).expect("retained reviews cross");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((88, SCHEMA_VERSION)),
    );
    assert_eq!(
        review_status(&journal, 3),
        wire::body::TaskCompletionStatus::Pending,
        "a clean review a later review follows did not settle the task"
    );
    assert_eq!(
        review_status(&journal, 4),
        wire::body::TaskCompletionStatus::Completed,
        "the clean final review of a settled non-Design task is its settlement"
    );
    assert_eq!(
        review_status(&journal, 6),
        wire::body::TaskCompletionStatus::Pending,
        "a clean review of a review,design Design advances it, never settles it"
    );
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}

fn review_status(journal: &Journal, seq: i64) -> wire::body::TaskCompletionStatus {
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq],
            |row| row.get(0),
        )
        .expect("crossed review reads");
    wire::decode::<Envelope<AnyFact>>(&bytes)
        .expect("crossed review decodes strictly")
        .fact
        .read::<wire::body::TaskReviewed>()
        .expect("the crossed fact is a review")
        .status
}

/// The fixture task's clean final review already settled it positively.
fn settle_fixture_task(journal: &Journal) {
    let settled = WorkflowHead {
        status: wire::WorkflowStatus::reviewed(instant()),
        ..workflow_head()
    };
    journal
        .connection
        .execute(
            "UPDATE workflow_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            params![wire::encode(&settled).expect("encode settled workflow")],
        )
        .expect("settle fixture workflow");
}

/// A `review,design` Design whose clean review moved it to its design phase.
fn plant_design(journal: &Journal) {
    insert_fact(
        journal,
        fact(
            5,
            None,
            FactKind::TaskAdded,
            json!({
                "task": DESIGN,
                "title": "Design fixture",
                "pitch": "A Design fixture needs a pitch.",
                "project": "journal",
                "intent": "design",
                "depends": [],
                "flow": ["review", "design"],
                "phase": "review",
                "revision": 1,
                "creation": {"kind": "unavailable", "reason": "operator-actor"}
            }),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (5, 'task:1kKRRsDP', NULL, 5, 0)",
            [],
        )
        .expect("insert design thread");
    let statement = TaskHead {
        born: DESIGN.parse().expect("design stamp"),
        intent: Some(wire::TaskIntent::Design),
        title: "Design fixture".to_owned(),
        queue_origin: Seq::new(5).expect("queue origin"),
        ..task_head()
    };
    insert_head(
        &journal.connection,
        "task_heads",
        "stamp",
        DESIGN,
        5,
        &statement,
    );
    let workflow = WorkflowHead {
        flow: vec![phase("review"), phase("design")],
        phase: phase("design"),
        ..workflow_head()
    };
    insert_head(
        &journal.connection,
        "workflow_heads",
        "stamp",
        DESIGN,
        5,
        &workflow,
    );
}

/// A schema-88 review: clean, with no workflow status member.
fn plant_review(journal: &Journal, seq_number: u64, task: &str, cause: u64) {
    let retained = json!({
        "kind": "task::reviewed",
        "task": task,
        "reviewer": "operator",
        "verdict": "clean",
        "source": {"proven": {"heads": []}}
    });
    let envelope = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(seq_number),
        at: instant(),
        actor: Actor::Operator,
        origin: "task:1kKRRsDP".parse::<Origin>().expect("fixture origin"),
        cause: Some(seq(cause)),
        fact: retained,
    };
    let bytes = wire::encode(&envelope).expect("retained review encodes");
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes(\
                seq, wire_version, instant, actor, origin, cause, kind, envelope\
             ) VALUES (?1, 1, ?2, 'operator', 'task:1kKRRsDP', ?3, 'task::reviewed', ?4)",
            params![
                i64::try_from(seq_number).expect("fixture seq fits SQLite"),
                envelope.at.to_string(),
                i64::try_from(cause).expect("fixture cause fits SQLite"),
                bytes
            ],
        )
        .expect("retained review is planted");
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (?1, 'task:1kKRRsDP', ?2, 1, ?3)",
            params![
                i64::try_from(seq_number).expect("fixture seq fits SQLite"),
                i64::try_from(cause).expect("fixture cause fits SQLite"),
                i64::try_from(seq_number - 1).expect("fixture depth fits SQLite")
            ],
        )
        .expect("retained review thread is planted");
}
