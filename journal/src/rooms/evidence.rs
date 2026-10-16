//! Required question evidence.
//!
//! A task requires results only through explicit statement edits, so its
//! required set is a projection only those edits move. Positive task
//! completion reads required results against their current state: answered
//! makes a result available for assessment, and every other state leaves the
//! requirement unsatisfied. Candidate references may cite review facts, but
//! neither task evidence nor candidate coverage grants execution authority.
//! Commit admission and history replay must both use [`fold`], leaving
//! `task_evidence` an exactly rebuildable and independently verified index.

use rusqlite::{Connection, params};
use wire::rooms::{
    DesignPassProposal, QuestionResultHead, QuestionResultRecord, QuestionResultRef,
};
use wire::{Actor, AnyFact, Seq, TaskEvidenceEdit, TaskStamp};

use super::{Authority, Change, Refusal, require_operator, unsound};

/// Required-evidence projection, created with the current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE task_evidence (
    task TEXT NOT NULL,
    result TEXT NOT NULL,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    PRIMARY KEY (task, result)
) STRICT;
"#;

/// Admit an explicit evidence change, or a positive completion it gates.
pub(crate) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Result<(), Refusal> {
    if let Some(edited) = fact.read::<wire::body::TaskEdited>() {
        return match &edited.evidence {
            Some(edit) => change(authority, seq, actor, &edited.task, edit),
            None => Ok(()),
        };
    }
    if let Some(done) = fact.read::<wire::body::TaskDone>() {
        return require_answered(authority, &done.task);
    }
    match fact.read::<wire::body::TaskReviewed>() {
        Some(reviewed) if matches!(reviewed.verdict, wire::body::Verdict::Clean) => {
            require_answered(authority, &reviewed.task)
        }
        _ => Ok(()),
    }
}

/// Requiring binds a result asked in the task's own room; replacing or
/// waiving a required result is the operator's explicit act.
fn change(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    task: &TaskStamp,
    edit: &TaskEvidenceEdit,
) -> Result<(), Refusal> {
    if edit.released().is_some() {
        require_operator(actor, "replacing or waiving required evidence")?;
    }
    edit.apply(&authority.evidence(task)?)
        .or_else(|detail| unsound(format!("task {task} evidence: {detail}")))?;
    if let Some(required) = edit.required() {
        require_asked_in(authority, required, task)?;
    }
    authority.change(Change::Evidence {
        task: task.to_string(),
        released: edit.released().map(ToString::to_string),
        required: edit
            .required()
            .map(|result| (result.to_string(), seq.get())),
    })
}

/// A task with required evidence completes a phase positively only while
/// every required result is answered.
fn require_answered(authority: &impl Authority, task: &TaskStamp) -> Result<(), Refusal> {
    let unsatisfied = unsatisfied(authority, &authority.evidence(task)?)?;
    if unsatisfied.is_empty() {
        return Ok(());
    }
    unsound(format!(
        "task {task} requires answered evidence before positive completion: {}",
        unsatisfied.join(", ")
    ))
}

/// Every candidate evidence result must be asked in its controller's room.
pub(super) fn require_candidate_evidence(
    authority: &impl Authority,
    candidate: &DesignPassProposal,
) -> Result<(), Refusal> {
    for result in &candidate.evidence {
        require_asked_in(authority, result, &candidate.candidate.controller)?;
    }
    Ok(())
}

/// Each required result that is not answered, with its current state.
fn unsatisfied(
    authority: &impl Authority,
    results: &[QuestionResultRef],
) -> Result<Vec<String>, Refusal> {
    let mut unsatisfied = Vec::new();
    for result in results {
        let head: QuestionResultHead = wire::decode(&row(authority, result)?.head)?;
        if !matches!(
            head.state(),
            wire::rooms::QuestionResultState::Answered { .. }
        ) {
            unsatisfied.push(format!("{result} is {}", head.state().name()));
        }
    }
    Ok(unsatisfied)
}

fn require_asked_in(
    authority: &impl Authority,
    result: &QuestionResultRef,
    room: &TaskStamp,
) -> Result<(), Refusal> {
    let record: QuestionResultRecord = wire::decode(&row(authority, result)?.record)?;
    if record.context() != Some(room) {
        return unsound(format!("result {result} was not asked in room {room}"));
    }
    Ok(())
}

fn row(
    authority: &impl Authority,
    result: &QuestionResultRef,
) -> Result<super::conversation::ResultRow, Refusal> {
    match authority.question_result(result.as_str())? {
        Some(row) => Ok(row),
        None => unsound(format!("question result {result} does not exist")),
    }
}

/// Decode stored result keys in canonical order.
pub(super) fn parse(
    keys: impl IntoIterator<Item = String>,
) -> Result<Vec<QuestionResultRef>, Refusal> {
    keys.into_iter()
        .map(|key| {
            key.parse()
                .map(QuestionResultRef::new)
                .or_else(|_| unsound(format!("required evidence key {key} is not a result")))
        })
        .collect()
}

/// Apply one evidence change to the stored projection.
pub(super) fn apply_sql(
    connection: &Connection,
    task: &str,
    released: Option<&str>,
    required: Option<(&str, u64)>,
) -> Result<(), rusqlite::Error> {
    if let Some(result) = released {
        connection.execute(
            "DELETE FROM task_evidence WHERE task = ?1 AND result = ?2",
            params![task, result],
        )?;
    }
    if let Some((result, seq)) = required {
        insert(connection, task, result, seq)?;
    }
    Ok(())
}

pub(super) fn insert(
    connection: &Connection,
    task: &str,
    result: &str,
    seq: u64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO task_evidence(task, result, head_seq) VALUES (?1, ?2, ?3)",
        params![task, result, super::sql::sql(seq)],
    )?;
    Ok(())
}
