use super::*;

use wire::body::{
    JournalIngested, LaneSessionDeclared, LaneStarted, TaskAdded, TaskCreationContext,
    TaskCreationUnavailable,
};

#[test]
fn schema_twelve_reconstructs_creation_context_only_from_pre_creation_facts() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("creation-context-schema-twelve.sqlite3");
    let mut journal = Journal::open(&path).expect("current authority opens");
    let (lane, expected) = seed_proven_creation_history(&mut journal);
    let available = TaskStamp::for_micros(10).expect("available task stamp");
    let operator = TaskStamp::for_micros(11).expect("operator task stamp");
    let spice = TaskStamp::for_micros(12).expect("Spice task stamp");
    let unproved = TaskStamp::for_micros(13).expect("unproved lane task stamp");
    let unproved_lane = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    seed_task(
        &mut journal,
        Actor::Lane(lane),
        &available,
        expected.clone(),
    );
    seed_task(
        &mut journal,
        Actor::Operator,
        &operator,
        TaskCreationContext::unavailable(TaskCreationUnavailable::OperatorActor),
    );
    seed_task(
        &mut journal,
        Actor::Spice,
        &spice,
        TaskCreationContext::unavailable(TaskCreationUnavailable::SpiceActor),
    );
    seed_task(
        &mut journal,
        Actor::Lane(unproved_lane),
        &unproved,
        TaskCreationContext::unavailable(TaskCreationUnavailable::LegacyEvidenceMissing),
    );
    seed_post_creation_evidence(&mut journal, unproved_lane);
    remove_current_context(&journal.connection);
    downgrade_steering_index_to_millis(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 12 WHERE singleton = 1;
             PRAGMA user_version = 12;",
        )
        .expect("fixture downgrades to schema twelve");
    drop(journal);

    let journal = Journal::open(&path).expect("schema twelve creation context migrates");
    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((12, SCHEMA_VERSION))
    );
    assert_migrated_contexts(
        &journal,
        &[
            (available, expected),
            (
                operator,
                TaskCreationContext::unavailable(TaskCreationUnavailable::OperatorActor),
            ),
            (
                spice,
                TaskCreationContext::unavailable(TaskCreationUnavailable::SpiceActor),
            ),
            (
                unproved,
                TaskCreationContext::unavailable(TaskCreationUnavailable::LegacyEvidenceMissing),
            ),
        ],
    );
}

#[test]
fn schema_twelve_marks_a_sole_unregistered_stream_unavailable() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("creation-context-unregistered-stream.sqlite3");
    let mut journal = Journal::open(&path).expect("current authority opens");
    let (lane, legacy_context) = seed_creation_history(
        &mut journal,
        uuid("018f82d4-6f59-7d06-92e3-bce9131163a1"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163a2"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163a3"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163a4"),
        "stdout",
        "unregistered-stream-session",
    );
    let task = TaskStamp::for_micros(14).expect("unregistered stream task stamp");
    seed_task(&mut journal, Actor::Lane(lane), &task, legacy_context);
    remove_current_context(&journal.connection);
    downgrade_steering_index_to_millis(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 12 WHERE singleton = 1;
             PRAGMA user_version = 12;",
        )
        .expect("fixture downgrades to schema twelve");
    drop(journal);

    let journal = Journal::open(&path).expect("schema twelve creation context migrates");
    assert_migrated_contexts(
        &journal,
        &[(
            task,
            TaskCreationContext::unavailable(TaskCreationUnavailable::LegacyStreamUnregistered),
        )],
    );
}

fn seed_proven_creation_history(journal: &mut Journal) -> (CanonicalUuid, TaskCreationContext) {
    seed_creation_history(
        journal,
        uuid("018f82d4-6f59-7d06-92e3-bce913116371"),
        uuid("018f82d4-6f59-7d06-92e3-bce913116372"),
        uuid("018f82d4-6f59-7d06-92e3-bce913116373"),
        uuid("018f82d4-6f59-7d06-92e3-bce913116374"),
        wire::body::TASK_CREATION_CONTEXT_STREAM,
        "session-migration",
    )
}

fn seed_creation_history(
    journal: &mut Journal,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    source: CanonicalUuid,
    stream: &str,
    identity: &str,
) -> (CanonicalUuid, TaskCreationContext) {
    let vendor: wire::Name = "codex".parse().expect("fixture vendor");
    let next = TranscriptPosition {
        member: 1,
        offset: 256,
    };
    let boundary = TranscriptSourceHead {
        lane,
        vendor: vendor.clone(),
        stream: stream.to_owned(),
        source,
        next,
    };
    let expected = TaskCreationContext::Transcript {
        session: identity.to_owned(),
        boundary: boundary.clone(),
        creator_team: None,
    };
    let mut history = crate::JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}").parse().expect("fixture origin"),
        instant(),
        wire::Producer::ResidentRecognition,
    );
    history
        .fact(
            LaneStarted {
                lane,
                occupant,
                epoch,
                group: 41,
            },
            crate::FactCause::Root,
        )
        .expect("lane start builds");
    history
        .fact(
            LaneSessionDeclared {
                lane,
                occupant,
                worktree: "/retained/worktree".to_owned(),
                vendor: vendor.clone(),
                launch: None,
                epoch,
                stream: "stdout".to_owned(),
                identity: identity.to_owned(),
                member: 1,
                offset: 0,
                length: 64,
            },
            crate::FactCause::Root,
        )
        .expect("session declaration builds");
    let ingested = history
        .fact(
            JournalIngested {
                lane,
                vendor: vendor.clone(),
                stream: stream.to_owned(),
                source,
                supersedes: None,
                from: TranscriptPosition {
                    member: 1,
                    offset: 0,
                },
                to: next,
                usage: Vec::new(),
                non_usage_records: 1,
                residue_records: 0,
            },
            crate::FactCause::Root,
        )
        .expect("transcript ingestion builds");
    history.transcript_source_head(ingested, move |_| boundary);
    journal.apply(history).expect("immutable history commits");
    (lane, expected)
}

fn assert_migrated_contexts(
    journal: &Journal,
    expected_by_task: &[(TaskStamp, TaskCreationContext)],
) {
    for (task, expected) in expected_by_task {
        let head = journal
            .read(|read| read.task_head(task))
            .expect("migrated task head reads")
            .expect("migrated task head exists");
        assert_eq!(&head.body().creation, expected);
        let fact = journal
            .fact_snapshot()
            .expect("migrated facts read")
            .facts()
            .iter()
            .find_map(|envelope| {
                envelope
                    .fact
                    .read::<TaskAdded>()
                    .filter(|added| &added.task == task)
            })
            .expect("migrated creation fact exists");
        assert_eq!(&fact.creation, expected);
    }
}

fn seed_post_creation_evidence(journal: &mut Journal, lane: CanonicalUuid) {
    let occupant = uuid("018f82d4-6f59-7d06-92e3-bce913116391");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116392");
    let source = uuid("018f82d4-6f59-7d06-92e3-bce913116393");
    let vendor: wire::Name = "codex".parse().expect("fixture vendor");
    let next = TranscriptPosition {
        member: 1,
        offset: 512,
    };
    let mut write = crate::JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}").parse().expect("fixture origin"),
        instant(),
        wire::Producer::ResidentRecognition,
    );
    let started = write
        .fact(
            LaneStarted {
                lane,
                occupant,
                epoch,
                group: 42,
            },
            crate::FactCause::Root,
        )
        .expect("post-creation lane start builds");
    let declared = write
        .fact(
            LaneSessionDeclared {
                lane,
                occupant,
                worktree: "/late/worktree".to_owned(),
                vendor: vendor.clone(),
                launch: None,
                epoch,
                stream: "stdout".to_owned(),
                identity: "too-late-session".to_owned(),
                member: 1,
                offset: 0,
                length: 64,
            },
            crate::FactCause::Pending(started),
        )
        .expect("post-creation session declaration builds");
    let ingested = write
        .fact(
            JournalIngested {
                lane,
                vendor: vendor.clone(),
                stream: "session".to_owned(),
                source,
                supersedes: None,
                from: TranscriptPosition {
                    member: 1,
                    offset: 0,
                },
                to: next,
                usage: Vec::new(),
                non_usage_records: 1,
                residue_records: 0,
            },
            crate::FactCause::Pending(declared),
        )
        .expect("post-creation transcript ingestion builds");
    write.transcript_source_head(ingested, move |_| TranscriptSourceHead {
        lane,
        vendor,
        stream: "session".to_owned(),
        source,
        next,
    });
    journal
        .apply(write)
        .expect("post-creation evidence commits");
}

fn seed_task(journal: &mut Journal, actor: Actor, task: &TaskStamp, creation: TaskCreationContext) {
    let phase = phase("todo");
    let origin: Origin = format!("task:{task}").parse().expect("fixture origin");
    let mut write = crate::JournalWrite::new(
        actor,
        origin.clone(),
        instant(),
        wire::Producer::CommandDoor,
    );
    let added = write
        .fact(
            TaskAdded {
                task: task.clone(),
                title: format!("Migrate task {task}"),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "task.migration".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
                creation: creation.clone(),
            },
            crate::FactCause::Root,
        )
        .expect("task creation fact builds");
    let born = task.clone();
    write.task_head(added, move |position| TaskHead {
        born,
        queue_origin: position,
        title: "Migrated task".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "task.migration".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        creation,
        revision: position,
    });
    let born = task.clone();
    write.workflow_head(born, added, move |_| WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    journal.apply(write).expect("task commits");
}

fn remove_current_context(connection: &Connection) {
    let rows = {
        let mut statement = connection
            .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind = 'task::added'")
            .expect("task facts select prepares");
        statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("task facts query")
            .collect::<Result<Vec<_>, _>>()
            .expect("task facts collect")
    };
    for (seq, bytes) in rows {
        let mut value: Value = serde_json::from_slice(&bytes).expect("task envelope decodes");
        assert!(
            value
                .pointer_mut("/fact")
                .and_then(Value::as_object_mut)
                .expect("task fact is an object")
                .remove("creation")
                .is_some()
        );
        connection
            .execute(
                "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                params![
                    serde_json::to_vec(&value).expect("legacy fact encodes"),
                    seq
                ],
            )
            .expect("legacy task fact updates");
    }
    let rows = {
        let mut statement = connection
            .prepare("SELECT stamp, body FROM task_heads")
            .expect("task heads select prepares");
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("task heads query")
            .collect::<Result<Vec<_>, _>>()
            .expect("task heads collect")
    };
    for (stamp, bytes) in rows {
        let mut value: Value = serde_json::from_slice(&bytes).expect("task head decodes");
        assert!(
            value
                .as_object_mut()
                .expect("task head is an object")
                .remove("creation")
                .is_some()
        );
        connection
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
                params![
                    serde_json::to_vec(&value).expect("legacy head encodes"),
                    stamp
                ],
            )
            .expect("legacy task head updates");
    }
}
