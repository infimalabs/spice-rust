use serde_json::Value;

use super::*;

const STAMP: &str = "1kKRRsDP";
const HANDLE: &str = "JOURNAL-1kKRRsDP";

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero sequence")
}

#[test]
fn schema_three_heads_gain_durable_queue_and_claim_origins_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("allocator-schema-three.sqlite3");
    schema_three_authority(&path);

    let mut journal = Journal::open(&path).expect("schema three allocator evidence migrates");

    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((3, SCHEMA_VERSION)),
    );
    let task = journal
        .read(|read| read.task_heads_for(&STAMP.parse().expect("task stamp")))
        .expect("migrated heads read")
        .expect("one task");
    assert_eq!(task.statement().body().born.as_str(), STAMP);
    assert_eq!(task.statement().body().queue_origin, seq(1));
    let claim = task.claim().expect("one claim").body();
    assert_eq!(claim.fact, seq(3));
    assert_eq!(claim.claimed, seq(2));
    assert_eq!(
        claim.cell,
        wire::ClaimCell {
            project: "journal.allocate".to_owned(),
            phase: phase("todo"),
        }
    );
    admit_empty_steering_fold(&mut journal);
    journal.verify().expect("migrated authority verifies");
}

struct AllocationFixture {
    task: TaskStamp,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    origin: Origin,
    cell: wire::ClaimCell,
}

fn allocation_fixture() -> AllocationFixture {
    AllocationFixture {
        task: STAMP.parse().expect("task stamp"),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116371"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116372"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116373"),
        origin: format!("task:{STAMP}").parse().expect("origin"),
        cell: wire::ClaimCell {
            project: "journal.allocate".to_owned(),
            phase: phase("todo"),
        },
    }
}

fn schema_three_authority(path: &Path) {
    let mut journal = Journal::open(path).expect("create current authority");
    let fixture = allocation_fixture();
    seed_task(&mut journal, &fixture);
    seed_claim(&mut journal, &fixture);
    seed_free(&mut journal, &fixture);
    downgrade(&journal.connection);
}

fn seed_task(journal: &mut Journal, fixture: &AllocationFixture) {
    let mut add = crate::JournalWrite::new(
        Actor::Spice,
        fixture.origin.clone(),
        instant(),
        wire::Producer::CommandDoor,
    );
    let added = add
        .fact(
            wire::body::TaskAdded {
                task: fixture.task.clone(),
                title: "Preserve queue age".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: fixture.cell.project.clone(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![fixture.cell.phase.clone()],
                phase: fixture.cell.phase.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            crate::FactCause::Root,
        )
        .expect("task fact");
    let statement_task = fixture.task.clone();
    let statement_project = fixture.cell.project.clone();
    let origin = fixture.origin.clone();
    add.task_head(added, move |position| TaskHead {
        born: statement_task,
        queue_origin: position,
        title: "Preserve queue age".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: statement_project,
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: position,
    });
    let workflow_task = fixture.task.clone();
    let workflow_phase = fixture.cell.phase.clone();
    add.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![workflow_phase.clone()],
        phase: workflow_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    journal.apply(add).expect("task commits");
}

fn seed_claim(journal: &mut Journal, fixture: &AllocationFixture) {
    let actor = Actor::Lane(fixture.lane);
    let mut claim = crate::JournalWrite::new(
        actor.clone(),
        format!("task:{STAMP}").parse().expect("claim origin"),
        instant(),
        wire::Producer::CommandDoor,
    );
    let won = claim
        .fact(
            wire::body::ClaimWon {
                task: fixture.task.clone(),
                actor: actor.clone(),
                lane: fixture.lane,
                occupant: fixture.occupant,
                epoch: fixture.epoch,
                cell: Some(fixture.cell.clone()),
                until: instant(),
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            crate::FactCause::Root,
        )
        .expect("claim fact");
    let claim_task = fixture.task.clone();
    let claim_cell = fixture.cell.clone();
    let lane = fixture.lane;
    let occupant = fixture.occupant;
    let epoch = fixture.epoch;
    claim.claim_head(claim_task, won, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell: claim_cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    let lease_task = fixture.task.clone();
    claim.lease_head(lease_task, won, move |_| LeaseHead {
        start: instant(),
        renewal: instant(),
        maximum: instant(),
        expiry: instant(),
        epoch,
    });
    journal.apply(claim).expect("claim commits");
}

fn seed_free(journal: &mut Journal, fixture: &AllocationFixture) {
    let lane = fixture.lane;
    let mut free = crate::JournalWrite::new(
        Actor::Lane(lane),
        format!("task:{STAMP}").parse().expect("free origin"),
        instant(),
        wire::Producer::CommandDoor,
    );
    let freed = free
        .fact(
            wire::body::ClaimFreed {
                task: fixture.task.clone(),
                actor: Actor::Lane(lane),
            },
            crate::FactCause::Root,
        )
        .expect("free fact");
    let task = fixture.task.clone();
    let cell = fixture.cell.clone();
    let occupant = fixture.occupant;
    let epoch = fixture.epoch;
    free.claim_head(task, freed, move |position| ClaimHead {
        actor: Actor::Lane(lane),
        fact: position,
        claimed: seq(2),
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Freed,
    });
    journal.apply(free).expect("free commits");
}

fn downgrade(connection: &Connection) {
    drop_steering_index(connection);
    downgrade_task_identity_to_five(connection);
    remove_fields(
        connection,
        &format!("SELECT body FROM task_heads WHERE handle = '{HANDLE}'"),
        &format!("UPDATE task_heads SET body = ?1 WHERE handle = '{HANDLE}'"),
        &["queue_origin"],
    );
    remove_fields(
        connection,
        &format!("SELECT body FROM claim_heads WHERE handle = '{HANDLE}'"),
        &format!("UPDATE claim_heads SET body = ?1 WHERE handle = '{HANDLE}'"),
        &["claimed", "cell"],
    );
    remove_pointer(
        connection,
        "SELECT envelope FROM fact_envelopes WHERE seq = 2",
        "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 2",
        "/fact/cell",
    );
    connection
        .execute_batch(
            "UPDATE journal_schema SET version = 3 WHERE singleton = 1;
             PRAGMA user_version = 3;",
        )
        .expect("downgrade allocator fixture");
}

fn remove_fields(connection: &Connection, select: &str, update: &str, fields: &[&str]) {
    let bytes = connection
        .query_row(select, [], |row| row.get::<_, Vec<u8>>(0))
        .expect("read carrier");
    let mut value: Value = serde_json::from_slice(&bytes).expect("decode carrier");
    let object = value.as_object_mut().expect("head object");
    for field in fields {
        assert!(object.remove(*field).is_some(), "fixture carries {field}");
    }
    connection
        .execute(
            update,
            params![serde_json::to_vec(&value).expect("encode carrier")],
        )
        .expect("store carrier");
}

fn remove_pointer(connection: &Connection, select: &str, update: &str, pointer: &str) {
    let bytes = connection
        .query_row(select, [], |row| row.get::<_, Vec<u8>>(0))
        .expect("read envelope");
    let mut value: Value = serde_json::from_slice(&bytes).expect("decode envelope");
    let (parent, field) = pointer.rsplit_once('/').expect("field pointer");
    assert!(
        value
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .and_then(|object| object.remove(field))
            .is_some(),
        "fixture carries pointer",
    );
    connection
        .execute(
            update,
            params![serde_json::to_vec(&value).expect("encode legacy envelope")],
        )
        .expect("store envelope");
}
