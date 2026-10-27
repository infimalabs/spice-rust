//! The task generation, assigned where every task write commits.
//!
//! A transaction holding any fact that changes a task's content or workflow
//! ([`wire::AnyFact::mutated_tasks`]) leaves that task's workflow generation
//! exactly one past where it stood, and a task created in the transaction
//! opens at zero. Every other workflow write preserves the stored generation.
//!
//! A claim won in the transaction acknowledges the generation the transaction
//! leaves. A mutation made by the active claim's own actor is one that holder
//! has seen, so it carries a claim that was current into the new generation;
//! a claim that already trailed stays behind until the holder explicitly
//! acknowledges it. Any other claim write keeps the acknowledgement its writer
//! carried, which may never exceed the task's generation. A completion or
//! review verdict commits only under a claim that acknowledged the generation
//! the task stood at when the transaction began.
//!
//! Writers never compute the generation. They build their heads from a
//! snapshot, and only this immediate transaction knows the stored value the
//! write lands on, so settling it here keeps it one canonical count. A
//! mutation that writes no workflow head of its own (a note, a statement
//! edit) advances the stored workflow body in place: the head's position keeps
//! naming the task's last workflow transition, which lifecycle deadlines,
//! witnesses, and recovery all read, and the generation counts mutations
//! through the journal cursor rather than through that position. The row's
//! `revised_seq` records where that revision landed, so a reader following
//! head changes by position still sees it. A holder's own acknowledgement
//! moves in place beside that revision for the same reason.
//!
//! Every advance also records `(task, generation, position)` in
//! `task_generations`, where the position is the transaction's last fact
//! mutating that task, so claimant delivery can name each generation a claim
//! has not acknowledged. Creation opens at zero and records nothing.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};

use super::commit::sqlite_fault;
use super::{CommitError, GuardRefusal, HeadKey, HeadRow, ProposedFact};

const WORKFLOW: &str = "workflow_heads";
const CLAIM: &str = "claim_heads";

/// What one task looked like before this transaction first touched it.
#[derive(Clone, Copy)]
struct Prior {
    /// Stored workflow generation; `None` for a task created here.
    generation: Option<u64>,
    /// Acknowledgement of the stored active claim, if there is one.
    acknowledged: Option<u64>,
    /// Whether that active claim is the writer's and was current.
    holder: bool,
}

/// One transaction's generation obligations, keyed by task stamp.
pub(super) struct Ledger {
    /// Actor committing this transaction.
    actor: wire::Actor,
    /// Tasks this transaction changes the content or workflow of, each with
    /// the position of its last such fact.
    mutated: BTreeMap<String, wire::Seq>,
    /// Tasks whose claim this transaction won.
    won: BTreeSet<String>,
    /// Tasks this transaction completes a phase of or records a verdict on.
    completed: BTreeSet<String>,
    /// Each touched task as it stood before this transaction.
    prior: BTreeMap<String, Prior>,
    /// Task heads this transaction wrote, as `(table, stamp)`.
    written: BTreeSet<(&'static str, String)>,
}

impl Ledger {
    pub(super) fn new(actor: &wire::Actor, facts: &[ProposedFact], assigned: &[wire::Seq]) -> Self {
        let mut ledger = Self {
            actor: actor.clone(),
            mutated: BTreeMap::new(),
            won: BTreeSet::new(),
            completed: BTreeSet::new(),
            prior: BTreeMap::new(),
            written: BTreeSet::new(),
        };
        for (ProposedFact { fact, .. }, seq) in facts.iter().zip(assigned) {
            for task in fact.mutated_tasks() {
                ledger.mutated.insert(task.to_string(), *seq);
            }
            if let Some(claim) = fact.read::<wire::body::ClaimWon>() {
                ledger.won.insert(claim.task.to_string());
            }
            if let Some(done) = fact.read::<wire::body::TaskDone>() {
                ledger.completed.insert(done.task.to_string());
            }
            if let Some(reviewed) = fact.read::<wire::body::TaskReviewed>() {
                ledger.completed.insert(reviewed.task.to_string());
            }
        }
        ledger
    }

    /// Settle the generation a task head row carries before it is stored.
    pub(super) fn settle(
        &mut self,
        transaction: &Transaction<'_>,
        row: &mut HeadRow,
    ) -> Result<(), CommitError> {
        let HeadKey::Text(stamp) = &row.key else {
            return Ok(());
        };
        let table = match row.table {
            WORKFLOW => WORKFLOW,
            CLAIM => CLAIM,
            _ => return Ok(()),
        };
        let (prior, generation) = self.touch(transaction, stamp)?;
        self.written.insert((table, stamp.clone()));
        if table == WORKFLOW {
            let mut head = decode::<wire::WorkflowHead>(WORKFLOW, stamp, &row.body)?;
            head.generation = generation;
            row.body = encode(&head)?;
            return Ok(());
        }
        let mut head = decode::<wire::ClaimHead>(CLAIM, stamp, &row.body)?;
        let held = prior.holder
            && self.mutated.contains_key(stamp)
            && head.disposition == wire::ClaimDisposition::Active
            && head.actor == self.actor;
        if self.won.contains(stamp) || held {
            head.acknowledged = generation;
        } else if head.acknowledged > generation {
            return Err(refused(
                stamp,
                "a claim cannot acknowledge a generation its task has not reached",
            ));
        }
        row.body = encode(&head)?;
        Ok(())
    }

    /// Refuse a completion under a claim that trailed its task, then advance
    /// every mutated task and current holder no writer rewrote, recording
    /// each advance in the generation index.
    pub(super) fn finish(mut self, transaction: &Transaction<'_>) -> Result<(), CommitError> {
        for stamp in std::mem::take(&mut self.completed) {
            let (prior, _) = self.touch(transaction, &stamp)?;
            if let (Some(generation), Some(acknowledged)) = (prior.generation, prior.acknowledged)
                && acknowledged < generation
            {
                return Err(refused(
                    &stamp,
                    "the claim has not acknowledged the task's generation; reclaim before completing or reviewing",
                ));
            }
        }
        for (stamp, revised) in self.mutated.clone() {
            let (prior, generation) = self.touch(transaction, &stamp)?;
            if prior.generation.is_some() {
                index(transaction, &stamp, generation, revised)?;
            }
            if !self.written.contains(&(WORKFLOW, stamp.clone()))
                && let Some(mut head) = stored::<wire::WorkflowHead>(transaction, WORKFLOW, &stamp)?
            {
                head.generation = generation;
                revise(transaction, &stamp, &encode(&head)?, revised)?;
            }
            if prior.holder
                && !self.written.contains(&(CLAIM, stamp.clone()))
                && let Some(mut head) = stored::<wire::ClaimHead>(transaction, CLAIM, &stamp)?
            {
                head.acknowledged = generation;
                acknowledge(transaction, &stamp, &encode(&head)?)?;
            }
        }
        Ok(())
    }

    /// The task as it stood before this transaction, and the generation this
    /// transaction leaves it at.
    ///
    /// The stored heads are read on first touch, before this transaction has
    /// written either, and remembered after.
    fn touch(
        &mut self,
        transaction: &Transaction<'_>,
        stamp: &str,
    ) -> Result<(Prior, u64), CommitError> {
        let prior = match self.prior.get(stamp) {
            Some(prior) => *prior,
            None => {
                let generation = stored::<wire::WorkflowHead>(transaction, WORKFLOW, stamp)?
                    .map(|head| head.generation);
                let claim = stored::<wire::ClaimHead>(transaction, CLAIM, stamp)?
                    .filter(|claim| claim.disposition == wire::ClaimDisposition::Active);
                let prior = Prior {
                    generation,
                    acknowledged: claim.as_ref().map(|claim| claim.acknowledged),
                    holder: claim.is_some_and(|claim| {
                        claim.actor == self.actor
                            && claim.acknowledged >= generation.unwrap_or_default()
                    }),
                };
                self.prior.insert(stamp.to_owned(), prior);
                prior
            }
        };
        let generation = match prior.generation {
            None => 0,
            Some(generation) if self.mutated.contains_key(stamp) => generation + 1,
            Some(generation) => generation,
        };
        Ok((prior, generation))
    }
}

/// The head stored for `stamp` in `table`, as this transaction sees it.
fn stored<T: serde::de::DeserializeOwned>(
    transaction: &Transaction<'_>,
    table: &'static str,
    stamp: &str,
) -> Result<Option<T>, CommitError> {
    transaction
        .query_row(
            &format!("SELECT body FROM {table} WHERE stamp = ?1"),
            [stamp],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(sqlite_fault)?
        .map(|bytes| decode(table, stamp, &bytes))
        .transpose()
}

/// Record where one task advance landed: the transaction's last fact
/// mutating that task.
fn index(
    transaction: &Transaction<'_>,
    stamp: &str,
    generation: u64,
    position: wire::Seq,
) -> Result<(), CommitError> {
    let generation = sqlite_int(stamp, generation)?;
    let position = sqlite_int(stamp, position.get())?;
    transaction
        .execute(
            "INSERT INTO task_generations(stamp, generation, seq) VALUES (?1, ?2, ?3)",
            params![stamp, generation, position],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

/// Advance a workflow body in place, keeping the transition position it is
/// stored at and recording where the revision landed.
fn revise(
    transaction: &Transaction<'_>,
    stamp: &str,
    body: &[u8],
    revised: wire::Seq,
) -> Result<(), CommitError> {
    let revised = sqlite_int(stamp, revised.get())?;
    transaction
        .execute(
            "UPDATE workflow_heads SET body = ?1, revised_seq = ?2 WHERE stamp = ?3",
            params![body, revised, stamp],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn sqlite_int(stamp: &str, value: u64) -> Result<i64, CommitError> {
    i64::try_from(value).map_err(|_| refused(stamp, "value exceeds SQLite integer range"))
}

/// Move a holder's acknowledgement in place; the workflow revision it rides
/// on names the task as changed.
fn acknowledge(transaction: &Transaction<'_>, stamp: &str, body: &[u8]) -> Result<(), CommitError> {
    transaction
        .execute(
            "UPDATE claim_heads SET body = ?1 WHERE stamp = ?2",
            params![body, stamp],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(
    table: &str,
    stamp: &str,
    bytes: &[u8],
) -> Result<T, CommitError> {
    wire::decode(bytes)
        .map_err(|error| refused(stamp, &format!("{table} head is invalid: {error}")))
}

fn encode<T: serde::Serialize>(head: &T) -> Result<Vec<u8>, CommitError> {
    wire::encode(head).map_err(|error| refused("head", &error.to_string()))
}

fn refused(stamp: &str, detail: &str) -> CommitError {
    CommitError::Refused(GuardRefusal::new(format!(
        "task generation refused for {stamp}: {detail}"
    )))
}
