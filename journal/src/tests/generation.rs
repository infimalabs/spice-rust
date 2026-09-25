//! The task generation the journal settles at every commit, and the claim
//! acknowledgement settled beside it.

use super::claim_index::add_claimed_task;
use super::*;

use crate::{FactCause, JournalWrite};
use wire::body::{ClaimFreed, TaskNoted};
use wire::{Producer, TaskStamp};

const HOLDER: &str = "018f82d4-6f59-7d06-92e3-bce913116360";

#[test]
fn mutations_advance_once_per_transaction_and_only_the_holder_keeps_pace() {
    let scratch = Scratch::new();
    let path = scratch.path().join("generation.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let holder = Actor::Lane(uuid(HOLDER));
    let task = add_claimed_task(&mut journal, 1, holder.clone());
    let transition = workflow(&journal, &task).position();
    assert_eq!(settled(&journal, &task), (0, 0));

    note(&mut journal, &holder, &task, 1);
    assert_eq!(
        settled(&journal, &task),
        (1, 1),
        "a holder sees its own change"
    );

    let applied = note(&mut journal, &Actor::Operator, &task, 2);
    assert_eq!(
        settled(&journal, &task),
        (2, 1),
        "two notes, one transaction"
    );
    let changed = journal
        .read(|read| read.changed_task_heads(applied.first(), applied.last()))
        .expect("changed heads read");
    assert_eq!(
        changed
            .iter()
            .map(|heads| heads.statement().body().born.clone())
            .collect::<Vec<_>>(),
        vec![task.clone()],
        "an in-place revision is a change a position-following reader sees"
    );

    note(&mut journal, &holder, &task, 1);
    assert_eq!(
        settled(&journal, &task),
        (3, 1),
        "a trailing claim stays behind"
    );
    free(&mut journal, &task);
    assert_eq!(
        settled(&journal, &task),
        (3, 1),
        "claim maintenance is no mutation"
    );
    assert_eq!(
        workflow(&journal, &task).position(),
        transition,
        "no mutation here moved the workflow transition"
    );

    let before = heads(&journal, &task);
    drop(journal);
    let journal = Journal::open(&path).expect("generation authority reopens");
    assert_eq!(heads(&journal, &task), before);
    journal.verify().expect("generation authority verifies");
}

#[test]
fn a_schema_twenty_seven_authority_opens_at_generation_zero_unchanged() {
    let scratch = Scratch::new();
    let path = scratch.path().join("generation-migration.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let task = add_claimed_task(&mut journal, 1, Actor::Lane(uuid(HOLDER)));
    journal
        .connection
        .execute_batch(
            "DROP INDEX workflow_heads_revised_seq;
             ALTER TABLE workflow_heads DROP COLUMN revised_seq;
             UPDATE journal_schema SET version = 27 WHERE singleton = 1;
             PRAGMA user_version = 27;",
        )
        .expect("downgrade the fixture to schema 27");
    let stored = rows(&journal.connection);
    drop(journal);

    let mut journal = Journal::open(&path).expect("schema 27 authority migrates");
    assert_eq!(
        rows(&journal.connection),
        stored,
        "no head or fact is rewritten"
    );
    assert_eq!(
        settled(&journal, &task),
        (0, 0),
        "no retained claim starts stale"
    );
    journal.verify().expect("migrated authority verifies");

    note(&mut journal, &Actor::Operator, &task, 1);
    assert_eq!(settled(&journal, &task), (1, 0));
}

/// Commit `count` notes on `task` from `actor` in one transaction.
fn note(
    journal: &mut Journal,
    actor: &Actor,
    task: &TaskStamp,
    count: usize,
) -> crate::guard::Applied {
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:claim-index-peer".parse().expect("claim origin"),
        instant(),
        Producer::CommandDoor,
    );
    for index in 0..count {
        write
            .fact(
                TaskNoted {
                    task: task.clone(),
                    note: format!("note {index}"),
                },
                FactCause::Root,
            )
            .expect("note fact builds");
    }
    journal.apply(write).expect("notes commit")
}

fn free(journal: &mut Journal, task: &TaskStamp) {
    let mut head = journal
        .read(|read| read.claim_head(task))
        .expect("claim reads")
        .expect("claim exists")
        .into_body();
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:claim-index-peer".parse().expect("claim origin"),
        instant(),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            ClaimFreed {
                task: task.clone(),
                actor: Actor::Operator,
            },
            FactCause::Existing(head.claimed),
        )
        .expect("release fact builds");
    head.disposition = ClaimDisposition::Freed;
    write.claim_head(task.clone(), fact, move |position| {
        head.fact = position;
        head
    });
    journal.apply(write).expect("release commits");
}

fn workflow(journal: &Journal, task: &TaskStamp) -> crate::guard::JournalHead<WorkflowHead> {
    journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
}

/// The task generation and its claim's acknowledgement.
fn settled(journal: &Journal, task: &TaskStamp) -> (u64, u64) {
    let (_, workflow, claim, _) = heads(journal, task).into_bodies();
    (
        workflow.generation,
        claim.expect("claim exists").acknowledged,
    )
}

fn heads(journal: &Journal, task: &TaskStamp) -> crate::guard::TaskHeads {
    journal
        .read(|read| read.task_heads_for(task))
        .expect("task heads read")
        .expect("task exists")
}

/// Every workflow and claim head row, and every fact, as stored.
fn rows(connection: &Connection) -> Vec<(String, i64, Vec<u8>)> {
    let mut statement = connection
        .prepare(
            "SELECT 'workflow:' || stamp, head_seq, body FROM workflow_heads UNION ALL \
             SELECT 'claim:' || stamp, head_seq, body FROM claim_heads UNION ALL \
             SELECT 'fact', seq, envelope FROM fact_envelopes ORDER BY 1, 2",
        )
        .expect("stored rows prepare");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("stored rows read")
        .collect::<Result<_, _>>()
        .expect("stored rows decode")
}
