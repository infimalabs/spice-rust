//! Bounded current-claim authority and schema-8 index admission.

use super::*;

use crate::{FactCause, JournalWrite};
use wire::body::{ClaimFreed, ClaimWon, SourceBoundary, TaskAdded, TaskNoted};
use wire::{ClaimCell, Producer};

#[test]
fn exact_actor_and_peer_count_work_stay_constant_as_board_and_history_grow() {
    let mut journal = authority_fixture();
    let before = measured_claim_authority(&journal);

    let mut notes = JournalWrite::new(
        Actor::Operator,
        "task:claim-index-scale".parse().expect("scale origin"),
        instant(),
        Producer::CommandDoor,
    );
    for offset in 0..256 {
        notes
            .fact(
                TaskNoted {
                    task: task_head().incepted,
                    note: format!("unrelated history {offset}"),
                },
                FactCause::Root,
            )
            .expect("unrelated fact builds");
    }
    journal.apply(notes).expect("unrelated facts commit");

    let mut tasks = JournalWrite::new(
        Actor::Operator,
        "task:claim-index-scale".parse().expect("scale origin"),
        instant(),
        Producer::CommandDoor,
    );
    for offset in 1..=256_u64 {
        add_unclaimed_task(&mut tasks, offset);
    }
    journal.apply(tasks).expect("unrelated board tasks commit");

    let after = measured_claim_authority(&journal);
    assert_eq!(after, before);
    assert_eq!(
        before.queries, 5,
        "claim, three exact heads, and peer count"
    );
    assert_eq!(before.rows, 5, "only the selected bundle and aggregate row");
    assert!(before.decoded_bytes > 0, "the selected bundle was decoded");

    let actor_plan = query_plan(
        &journal,
        "SELECT indexed.stamp, indexed.actor, indexed.disposition, \
                indexed.head_seq, heads.head_seq, heads.body \
         FROM claim_index AS indexed \
         JOIN claim_heads AS heads ON heads.stamp = indexed.stamp \
         WHERE indexed.actor = 'spice' AND indexed.disposition = 'active' \
         ORDER BY indexed.stamp LIMIT 2",
    );
    assert!(
        actor_plan.contains("claim_index_actor_disposition_stamp"),
        "exact actor lookup must use its composite index: {actor_plan}"
    );
    let peer_plan = query_plan(
        &journal,
        "SELECT count(*) FROM claim_index \
         WHERE disposition = 'active' AND actor <> 'spice'",
    );
    assert!(
        peer_plan.contains("claim_index_disposition_actor"),
        "peer count must use the active-disposition index: {peer_plan}"
    );
}

#[test]
fn corruption_outside_the_exact_actor_locus_does_not_poison_the_probe() {
    let mut journal = authority_fixture();
    let peer = add_claimed_task(&mut journal, 900, Actor::Operator);
    journal
        .connection
        .execute(
            "UPDATE claim_heads SET body = ?1 WHERE stamp = ?2",
            params![b"corrupt unrelated claim".to_vec(), peer.as_str()],
        )
        .expect("corrupt only the peer claim body");

    let (own, peers) = journal
        .read(|read| {
            Ok((
                read.active_claim_for_actor(&Actor::Spice)?,
                read.active_claim_count_excluding(&Actor::Spice)?,
            ))
        })
        .expect("out-of-locus corruption is not decoded");
    assert_eq!(
        own.expect("fixture actor claim remains visible")
            .statement()
            .body()
            .title,
        task_head().title
    );
    assert_eq!(peers, 1);
}

#[test]
fn claim_disposition_update_replaces_the_index_predicate_atomically() {
    let mut journal = authority_fixture();
    let peer = add_claimed_task(&mut journal, 901, Actor::Operator);
    assert_eq!(
        journal
            .read(|read| read.active_claim_count_excluding(&Actor::Spice))
            .expect("active peer count reads"),
        1
    );
    let observed = journal
        .read(|read| read.claim_head(&peer))
        .expect("peer claim head reads")
        .expect("peer claim exists")
        .into_body();
    let mut free = JournalWrite::new(
        Actor::Operator,
        "task:claim-index-peer".parse().expect("peer origin"),
        instant(),
        Producer::CommandDoor,
    );
    let freed = free
        .fact(
            ClaimFreed {
                task: peer.clone(),
                actor: Actor::Operator,
            },
            FactCause::Existing(observed.claimed),
        )
        .expect("peer release fact builds");
    let mut inactive = observed;
    inactive.disposition = ClaimDisposition::Freed;
    free.claim_head(peer, freed, move |position| {
        inactive.fact = position;
        inactive
    });
    journal.apply(free).expect("peer release commits");

    assert_eq!(
        journal
            .read(|read| read.active_claim_count_excluding(&Actor::Spice))
            .expect("inactive peer count reads"),
        0
    );
}

#[test]
fn verification_names_claim_index_predicate_disagreement() {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE claim_index SET actor = 'operator' WHERE stamp = '1kKRRsDP'",
            [],
        )
        .expect("corrupt indexed actor fixture");

    let error = journal
        .verify()
        .expect_err("claim index disagreement must fail verification");
    assert!(error.violations().iter().any(|violation| {
        violation.class == ViolationClass::Head
            && violation.subject == "claim_index:1kKRRsDP"
            && violation.detail.contains("predicate columns disagree")
    }));
}

#[test]
fn schema_seven_claim_heads_backfill_the_index_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-seven.sqlite3");
    let old = authority_file_fixture(&path);
    old.connection
        .execute_batch(
            "DROP TABLE claim_index;
             UPDATE journal_schema SET version = 7 WHERE singleton = 1;
             PRAGMA user_version = 7;",
        )
        .expect("downgrade schema-seven fixture");
    drop(old);

    let migrated = Journal::open(&path).expect("schema seven migrates");
    assert_eq!(
        migrated.migration().map(|step| (step.from(), step.to())),
        Some((7, SCHEMA_VERSION))
    );
    assert!(
        migrated
            .read(|read| read.active_claim_for_actor(&Actor::Spice))
            .expect("backfilled actor lookup reads")
            .is_some()
    );
}

fn measured_claim_authority(journal: &Journal) -> crate::guard::IndexedReadMetrics {
    crate::guard::reset_indexed_read_metrics();
    let (own, peers) = journal
        .read(|read| {
            Ok((
                read.active_claim_for_actor(&Actor::Spice)?,
                read.active_claim_count_excluding(&Actor::Spice)?,
            ))
        })
        .expect("bounded claim authority reads");
    assert_eq!(
        own.expect("fixture claim exists")
            .statement()
            .body()
            .incepted,
        task_head().incepted
    );
    assert_eq!(peers, 0);
    crate::guard::indexed_read_metrics()
}

fn add_unclaimed_task(write: &mut JournalWrite, offset: u64) -> TaskStamp {
    let millis = i64::try_from(10_000 + offset).expect("scale task offset fits i64");
    let task = TaskStamp::for_millis(millis).expect("scale task stamp");
    let title = format!("Unrelated board task {offset}");
    let flow = vec![phase("todo"), phase("review")];
    let fact = write
        .fact(
            TaskAdded {
                task: task.clone(),
                title: title.clone(),
                description: None,
                acceptance: Vec::new(),
                project: "journal".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: phase("todo"),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("unrelated task fact builds");
    let statement_task = task.clone();
    write.task_head(fact, move |revision| TaskHead {
        incepted: statement_task,
        queue_origin: revision,
        title,
        description: None,
        acceptance: Vec::new(),
        project: "journal".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:claim-index-scale".parse().expect("scale provenance"),
        revision,
    });
    let workflow_task = task.clone();
    write.workflow_head(workflow_task, fact, move |_| WorkflowHead {
        flow,
        phase: phase("todo"),
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    task
}

fn add_claimed_task(journal: &mut Journal, offset: u64, actor: Actor) -> TaskStamp {
    let mut add = JournalWrite::new(
        Actor::Operator,
        "task:claim-index-peer".parse().expect("peer origin"),
        instant(),
        Producer::CommandDoor,
    );
    let task = add_unclaimed_task(&mut add, offset);
    journal.apply(add).expect("peer task commits");

    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    let occupant = uuid("018f82d4-6f59-7d06-92e3-bce9131163d2");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163d3");
    let mut claim = JournalWrite::new(
        actor.clone(),
        "task:claim-index-peer".parse().expect("peer origin"),
        instant(),
        Producer::CommandDoor,
    );
    let won = claim
        .fact(
            ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(ClaimCell {
                    project: "journal".to_owned(),
                    phase: phase("todo"),
                }),
                until: instant(),
                git: None,
                source: SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("peer claim fact builds");
    let claim_task = task.clone();
    claim.claim_head(claim_task, won, move |position| ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell: ClaimCell {
            project: "journal".to_owned(),
            phase: phase("todo"),
        },
        lane,
        occupant,
        epoch,
        disposition: ClaimDisposition::Active,
    });
    let lease_task = task.clone();
    claim.lease_head(lease_task, won, move |_| LeaseHead {
        start: instant(),
        renewal: instant(),
        maximum: instant(),
        expiry: instant(),
        epoch,
    });
    journal.apply(claim).expect("peer claim commits");
    task
}

fn query_plan(journal: &Journal, sql: &str) -> String {
    journal
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("query plan prepares")
        .query_map([], |row| row.get::<_, String>(3))
        .expect("query plan rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("query plan decodes")
        .join("\n")
}
