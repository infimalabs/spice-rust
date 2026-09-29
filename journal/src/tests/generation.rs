//! The task generation the journal settles at every commit, the claim
//! acknowledgement settled beside it, and the index of every advance.

use super::claim_index::{add_claimed_task, query_plan};
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

    let first = note(&mut journal, &holder, &task, 1);
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

    let third = note(&mut journal, &holder, &task, 1);
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
    let recorded = [(1, first.last()), (2, applied.last()), (3, third.last())];
    assert_eq!(advances(&journal, &task, 0, 8), recorded);
    assert_eq!(
        advances(&journal, &task, 1, 1),
        recorded[1..2],
        "the read resumes past a generation and honors its bound"
    );
    let plan = query_plan(
        &journal,
        "SELECT generation, seq FROM task_generations \
         WHERE stamp = 'x' AND generation > 0 ORDER BY generation LIMIT 8",
    );
    assert!(
        plan.contains("SEARCH task_generations USING PRIMARY KEY") && !plan.contains("SCAN"),
        "one task's advances are a key range, not a scan: {plan}"
    );

    let before = heads(&journal, &task);
    drop(journal);
    let journal = Journal::open(&path).expect("generation authority reopens");
    assert_eq!(heads(&journal, &task), before);
    assert_eq!(advances(&journal, &task, 0, 8), recorded);
    journal.verify().expect("generation authority verifies");
}

#[test]
fn verification_names_an_index_that_disagrees_with_its_task() {
    for (corruption, subject) in [
        (
            "DELETE FROM task_generations WHERE generation = 1",
            "task_generations:{task}",
        ),
        (
            "DELETE FROM task_generations WHERE generation = 2",
            "task_generations:{task}",
        ),
        (
            "UPDATE task_generations SET seq = (SELECT min(seq) FROM fact_envelopes) \
             WHERE generation = 2",
            "task_generations:{task}:2",
        ),
    ] {
        let mut journal = authority_fixture();
        let task = add_claimed_task(&mut journal, 1, Actor::Lane(uuid(HOLDER)));
        note(&mut journal, &Actor::Operator, &task, 1);
        note(&mut journal, &Actor::Operator, &task, 1);
        journal.verify().expect("an intact index verifies");
        journal
            .connection
            .execute_batch(corruption)
            .expect("corrupt the index");
        let Err(crate::VerifyError::Violations(violations)) = journal.verify() else {
            panic!("verification accepted {corruption}");
        };
        let subject = subject.replace("{task}", task.as_str());
        assert!(
            violations
                .iter()
                .any(|violation| violation.subject == subject),
            "{corruption} was not named at {subject}: {violations:?}"
        );
    }
}

#[test]
fn a_schema_twenty_eight_advance_gains_one_baseline_row() {
    let scratch = Scratch::new();
    let path = scratch.path().join("generation-index-migration.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let task = add_claimed_task(&mut journal, 1, Actor::Lane(uuid(HOLDER)));
    note(&mut journal, &Actor::Operator, &task, 1);
    let latest = note(&mut journal, &Actor::Operator, &task, 1);
    journal
        .connection
        .execute_batch(
            "DROP TABLE task_generations;
             UPDATE journal_schema SET version = 28 WHERE singleton = 1;
             PRAGMA user_version = 28;",
        )
        .expect("downgrade the fixture to schema 28");
    let stored = rows(&journal.connection);
    drop(journal);

    let mut journal = Journal::open(&path).expect("schema 28 authority migrates");
    assert_eq!(
        rows(&journal.connection),
        stored,
        "no head or fact is rewritten"
    );
    assert_eq!(advances(&journal, &task, 0, 8), [(2, latest.last())]);
    journal.verify().expect("a baseline run verifies");

    let next = note(&mut journal, &Actor::Operator, &task, 1);
    assert_eq!(
        advances(&journal, &task, 0, 8),
        [(2, latest.last()), (3, next.last())]
    );
    journal
        .verify()
        .expect("an advance past the baseline verifies");
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
            "DROP TABLE task_generations;
             DROP INDEX workflow_heads_revised_seq;
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
    assert!(advances(&journal, &task, 0, 8).is_empty());
    journal.verify().expect("migrated authority verifies");

    let applied = note(&mut journal, &Actor::Operator, &task, 1);
    assert_eq!(settled(&journal, &task), (1, 0));
    assert_eq!(advances(&journal, &task, 0, 8), [(1, applied.last())]);
}

/// One task's recorded advances past `after`, at most `limit`.
fn advances(journal: &Journal, task: &TaskStamp, after: u64, limit: usize) -> Vec<(u64, Seq)> {
    journal
        .read(|read| read.task_generations_after(task, after, limit))
        .expect("generation index reads")
        .into_iter()
        .map(|advance| (advance.generation, advance.position))
        .collect()
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
                parked: None,
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
