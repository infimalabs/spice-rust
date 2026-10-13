//! Working-group engagement as execution authorization.
//!
//! An operator `group::stored` fact anchoring a Design is that Design's
//! activation. It authorizes every controlled output the Design already has,
//! and each output that becomes controlled while the Design stays anchored in
//! some working group. A statement records the authorizing fact as its
//! engagement: granted once, exclusive with a decision's execution
//! authorization, and never moved or dropped afterward, so a Design leaving
//! every group stops later grants without revoking earlier ones.

use rusqlite::{Connection, OptionalExtension, params};
use wire::rooms::WorkingGroupStored;
use wire::{Actor, AnyFact, Seq, TaskHead, TaskStamp};

use super::sql::sql;
use super::{Refusal, unsound};

/// Controller whose authorization gates `head`, when `head` is a controlled
/// output: preparation it constructs itself, without Plan or Design intent.
#[must_use]
pub(crate) fn gated_controller(head: &TaskHead) -> Option<&TaskStamp> {
    head.preparation
        .as_ref()
        .filter(|preparation| head.intent.is_none() && preparation.construction == head.born)
        .map(|preparation| &preparation.controller)
}

/// Whether `fact`, written by `actor`, is an operator working-group state
/// anchoring `controller`.
#[must_use]
pub(crate) fn engages(actor: &Actor, fact: &AnyFact, controller: &TaskStamp) -> bool {
    *actor == Actor::Operator
        && fact
            .read::<WorkingGroupStored>()
            .is_some_and(|stored| stored.group.anchors.contains(controller))
}

/// Admit one task statement row before it replaces the stored statement:
/// engagement is exclusive with decision authorization, survives every later
/// write unchanged, and is granted only to a controlled output whose
/// controller some working group currently anchors, naming an operator
/// working-group fact that anchors it.
pub(crate) fn require_transition(
    connection: &Connection,
    stamp: &str,
    body: &[u8],
) -> Result<(), Refusal> {
    let head = wire::decode::<TaskHead>(body)?;
    if head.authorization.is_some() && head.engagement.is_some() {
        return unsound(format!(
            "task {stamp} carries both a decision and an engagement authorization"
        ));
    }
    let prior = connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [stamp],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|body| wire::decode::<TaskHead>(&body))
        .transpose()?
        .and_then(|prior| prior.engagement);
    match (prior, head.engagement) {
        (prior, granted) if prior == granted => Ok(()),
        (None, Some(granted)) => require_grant(connection, stamp, &head, granted),
        _ => unsound(format!(
            "task {stamp} engagement authorization cannot move or drop"
        )),
    }
}

fn require_grant(
    connection: &Connection,
    stamp: &str,
    head: &TaskHead,
    granted: Seq,
) -> Result<(), Refusal> {
    let Some(controller) = gated_controller(head) else {
        return unsound(format!(
            "task {stamp} is not a controlled output an engagement can authorize"
        ));
    };
    if !anchored(connection, controller)? {
        return unsound(format!(
            "task {stamp} controller {controller} is in no working group to authorize it"
        ));
    }
    let envelope = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [sql(granted.get())],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|bytes| wire::decode::<wire::Envelope<AnyFact>>(&bytes))
        .transpose()?;
    if !envelope.is_some_and(|envelope| engages(&envelope.actor, &envelope.fact, controller)) {
        return unsound(format!(
            "task {stamp} engagement {granted} is no operator working group anchoring \
             {controller}"
        ));
    }
    Ok(())
}

/// After the write's heads land, every controlled output whose controller a
/// working group anchors carries an authorization: each statement written in
/// `first..=last`, and each output of every Design a working-group fact in the
/// write anchors.
pub(crate) fn require_complete<'a>(
    connection: &Connection,
    first: Seq,
    last: Seq,
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    let mut written = connection
        .prepare("SELECT stamp, body FROM task_heads WHERE head_seq BETWEEN ?1 AND ?2")?;
    let mut rows = written.query(params![sql(first.get()), sql(last.get())])?;
    while let Some(row) = rows.next()? {
        let head = wire::decode::<TaskHead>(&row.get::<_, Vec<u8>>(1)?)?;
        if let Some(controller) = gated_controller(&head)
            && anchored(connection, controller)?
        {
            require_authorized(&row.get::<_, String>(0)?, &head)?;
        }
    }
    let mut outputs = connection.prepare(
        "SELECT stamp, body FROM task_heads \
         WHERE json_extract(CAST(body AS TEXT), '$.preparation.controller') = ?1",
    )?;
    for fact in facts {
        let Some(WorkingGroupStored { group }) = fact.read() else {
            continue;
        };
        for anchor in &group.anchors {
            let mut rows = outputs.query([anchor.as_str()])?;
            while let Some(row) = rows.next()? {
                let head = wire::decode::<TaskHead>(&row.get::<_, Vec<u8>>(1)?)?;
                if gated_controller(&head).is_some() {
                    require_authorized(&row.get::<_, String>(0)?, &head)?;
                }
            }
        }
    }
    Ok(())
}

fn require_authorized(stamp: &str, head: &TaskHead) -> Result<(), Refusal> {
    if head.authorization.is_none() && head.engagement.is_none() {
        return unsound(format!(
            "controlled output {stamp} of an engaged Design carries no authorization"
        ));
    }
    Ok(())
}

fn anchored(connection: &Connection, controller: &TaskStamp) -> Result<bool, Refusal> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM group_anchors WHERE anchor = ?1 LIMIT 1",
            [controller.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
