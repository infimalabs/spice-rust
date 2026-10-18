//! Cross retained derived preparation control into ordinary dependency edges.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, params};

use super::{OpenError, residual, table_exists};

/// Last schema whose statements carried a derived authorization field.
const DERIVED_CONTROL_RELEASE: &str = "schema-77";
const SELECTED: &str = "preparation_selected_outputs";

/// Remove the retired field before any current typed statement decoder runs,
/// retaining its positive identities in a transaction-local table.
pub(super) fn strip_authorization(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch(&format!(
            "CREATE TEMP TABLE {SELECTED} (output TEXT PRIMARY KEY) STRICT;"
        ))
        .map_err(OpenError::Sqlite)?;
    let rows = {
        let mut statement = transaction
            .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (stamp, bytes) in rows {
        let mut value: serde_json::Value = wire::decode(&bytes).map_err(|error| {
            retained(
                "task_heads",
                stamp.clone(),
                format!("statement does not decode: {error}"),
            )
        })?;
        let selected = value
            .as_object_mut()
            .and_then(|object| object.remove("authorization"))
            .is_some_and(|authorization| !authorization.is_null());
        if selected {
            transaction
                .execute(
                    &format!("INSERT INTO {SELECTED}(output) VALUES (?1)"),
                    [&stamp],
                )
                .map_err(OpenError::Sqlite)?;
        }
        let bytes = wire::encode(&value).map_err(|error| {
            retained(
                "task_heads",
                stamp.clone(),
                format!("statement does not encode: {error}"),
            )
        })?;
        transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
                params![bytes, stamp],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

/// Add each still-live gate edge and close every pending branch whose retained
/// controller can no longer release it.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let statements = read_statements(transaction)?;
    let workflows = read_workflows(transaction)?;
    let selected = selected_outputs(transaction)?;
    let mut close = BTreeSet::new();
    let mut prepared = BTreeMap::new();
    let mut gated = BTreeMap::new();

    for (stamp, statement) in &statements {
        let Some(preparation) = statement
            .preparation
            .as_ref()
            .filter(|preparation| preparation.construction.as_str() == stamp.as_str())
        else {
            continue;
        };
        prepared.insert(stamp.clone(), preparation.parent.clone());
        let Some(workflow) = workflows.get(stamp) else {
            return Err(retained(
                "workflow_heads",
                stamp.to_string(),
                "prepared statement has no workflow head".to_owned(),
            ));
        };
        if !workflow.status.is_pending() || selected.contains(stamp) {
            continue;
        }
        let Some(parent) = statements.get(&preparation.parent) else {
            close.insert(stamp.clone());
            continue;
        };
        let Some(parent_workflow) = workflows.get(&preparation.parent) else {
            close.insert(stamp.clone());
            continue;
        };
        match parent.intent {
            Some(intent) if intent.is_design() => {
                if !wire::intent::is_open_design(parent.intent, parent_workflow) {
                    close.insert(stamp.clone());
                } else {
                    gated.insert(stamp.clone(), preparation.parent.clone());
                }
            }
            Some(wire::TaskIntent::Plan) if parent_workflow.status.is_pending() => {
                gated.insert(stamp.clone(), preparation.parent.clone());
            }
            Some(wire::TaskIntent::Plan) if parent_workflow.concluded_positively() => {}
            Some(wire::TaskIntent::Plan) => {
                close.insert(stamp.clone());
            }
            _ => {}
        }
    }

    loop {
        let descendants = statements
            .iter()
            .filter(|(stamp, statement)| {
                workflows
                    .get(*stamp)
                    .is_some_and(|workflow| workflow.status.is_pending())
                    && (gated
                        .get(*stamp)
                        .is_some_and(|parent| close.contains(parent))
                        || statement
                            .dependencies
                            .iter()
                            .any(|dependency| close.contains(dependency)))
            })
            .map(|(stamp, _)| stamp.clone())
            .collect::<Vec<_>>();
        let before = close.len();
        close.extend(descendants);
        if close.len() == before {
            break;
        }
    }

    for (stamp, mut statement) in statements {
        statement
            .dependencies
            .retain(|dependency| prepared.get(&stamp) != Some(dependency));
        if let Some(parent) = gated.get(&stamp)
            && !close.contains(&stamp)
            && !statement.dependencies.contains(parent)
        {
            statement.dependencies.push(parent.clone());
        }
        let bytes = wire::encode(&statement).map_err(|error| {
            retained(
                "task_heads",
                stamp.to_string(),
                format!("statement does not encode: {error}"),
            )
        })?;
        transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
                params![bytes, stamp.as_str()],
            )
            .map_err(OpenError::Sqlite)?;
    }

    for stamp in close {
        append_closure(transaction, &stamp)?;
    }
    transaction
        .execute_batch(&format!("DROP TABLE {SELECTED};"))
        .map_err(OpenError::Sqlite)
}

fn read_statements(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<wire::TaskStamp, wire::TaskHead>, OpenError> {
    let mut statement = transaction
        .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?
        .map(|row| {
            let (stamp, bytes) = row.map_err(OpenError::Sqlite)?;
            let head = wire::decode::<wire::TaskHead>(&bytes).map_err(|error| {
                retained(
                    "task_heads",
                    stamp,
                    format!("current statement does not decode: {error}"),
                )
            })?;
            Ok((head.born.clone(), head))
        })
        .collect()
}

fn read_workflows(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<wire::TaskStamp, wire::WorkflowHead>, OpenError> {
    let mut statement = transaction
        .prepare("SELECT stamp, body FROM workflow_heads ORDER BY stamp")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?
        .map(|row| {
            let (stamp, bytes) = row.map_err(OpenError::Sqlite)?;
            let task: wire::TaskStamp = stamp.parse().map_err(|error| {
                retained(
                    "workflow_heads",
                    stamp.clone(),
                    format!("invalid task stamp: {error}"),
                )
            })?;
            let head = wire::decode::<wire::WorkflowHead>(&bytes).map_err(|error| {
                retained(
                    "workflow_heads",
                    stamp,
                    format!("current workflow does not decode: {error}"),
                )
            })?;
            Ok((task, head))
        })
        .collect()
}

fn selected_outputs(transaction: &Transaction<'_>) -> Result<BTreeSet<wire::TaskStamp>, OpenError> {
    let mut outputs = BTreeSet::new();
    let mut selected = transaction
        .prepare(&format!("SELECT output FROM {SELECTED} ORDER BY output"))
        .map_err(OpenError::Sqlite)?;
    for output in selected
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(OpenError::Sqlite)?
    {
        let output = output.map_err(OpenError::Sqlite)?;
        outputs.insert(output.parse().map_err(|error| {
            retained(
                "task_heads",
                output.clone(),
                format!("invalid selected output: {error}"),
            )
        })?);
    }
    if table_exists(transaction, "authorization_heads")? {
        let mut authorizations = transaction
            .prepare("SELECT output FROM authorization_heads ORDER BY output")
            .map_err(OpenError::Sqlite)?;
        for output in authorizations
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(OpenError::Sqlite)?
        {
            let output = output.map_err(OpenError::Sqlite)?;
            outputs.insert(output.parse().map_err(|error| {
                retained(
                    "authorization_heads",
                    output.clone(),
                    format!("invalid selected output: {error}"),
                )
            })?);
        }
    }
    Ok(outputs)
}

struct ClosureState {
    workflow: wire::WorkflowHead,
    phase: wire::Phase,
    next: i64,
    sequence: wire::Seq,
    at: wire::Instant,
    claim: Option<wire::ClaimHead>,
}

fn append_closure(transaction: &Transaction<'_>, task: &wire::TaskStamp) -> Result<(), OpenError> {
    let Some(state) = read_closure_state(transaction, task)? else {
        return Ok(());
    };
    let (origin, envelope) = closure_envelope(task, &state)?;
    transaction
        .execute(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'task::closed', ?6)",
            params![
                state.next,
                wire::WireVersion::CURRENT.0,
                state.at.to_string(),
                wire::Actor::Spice.to_string(),
                origin.as_str(),
                envelope
            ],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, ?2, NULL, ?1, 0)",
            params![state.next, origin.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    advance_workflow(transaction, task, &state)?;
    free_claim(transaction, task, &state)?;
    advance_cursors(transaction, state.next)
}

fn read_closure_state(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
) -> Result<Option<ClosureState>, OpenError> {
    let (workflow_bytes, phase) = transaction
        .query_row(
            "SELECT body, json_extract(CAST(body AS TEXT), '$.phase') FROM workflow_heads WHERE stamp = ?1",
            [task.as_str()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(OpenError::Sqlite)?;
    let workflow: wire::WorkflowHead = wire::decode(&workflow_bytes).map_err(|error| {
        retained(
            "workflow_heads",
            task.to_string(),
            format!("workflow does not decode: {error}"),
        )
    })?;
    if !workflow.status.is_pending() {
        return Ok(None);
    }
    let (next, instant): (i64, String) = transaction
        .query_row(
            "SELECT coalesce(max(seq), 0) + 1, coalesce((SELECT instant FROM fact_envelopes ORDER BY seq DESC LIMIT 1), '1970-01-01T00:00:00Z') FROM fact_envelopes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(OpenError::Sqlite)?;
    let sequence = wire::Seq::new(u64::try_from(next).map_err(|error| {
        retained(
            "fact_envelopes",
            next.to_string(),
            format!("invalid next sequence: {error}"),
        )
    })?)
    .ok_or_else(|| {
        retained(
            "fact_envelopes",
            next.to_string(),
            "zero sequence".to_owned(),
        )
    })?;
    let at = instant.parse().map_err(|error| {
        retained(
            "fact_envelopes",
            next.to_string(),
            format!("invalid retained instant: {error}"),
        )
    })?;
    let phase = phase.parse().map_err(|error| {
        retained(
            "workflow_heads",
            task.to_string(),
            format!("invalid phase: {error}"),
        )
    })?;
    let claim = read_claim(transaction, task)?;
    Ok(Some(ClosureState {
        workflow,
        phase,
        next,
        sequence,
        at,
        claim,
    }))
}

fn read_claim(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
) -> Result<Option<wire::ClaimHead>, OpenError> {
    transaction
        .query_row(
            "SELECT body FROM claim_heads WHERE stamp = ?1",
            [task.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .map(|bytes| wire::decode::<wire::ClaimHead>(&bytes))
        .transpose()
        .map_err(|error| {
            retained(
                "claim_heads",
                task.to_string(),
                format!("claim does not decode: {error}"),
            )
        })
}

fn closure_envelope(
    task: &wire::TaskStamp,
    state: &ClosureState,
) -> Result<(wire::Origin, Vec<u8>), OpenError> {
    let claimed = state
        .claim
        .as_ref()
        .filter(|claim| claim.is_active())
        .map(|claim| claim.actor.clone());
    let closed = wire::body::TaskClosed {
        task: task.clone(),
        validation: "Retained preparation controller could no longer release this task during the schema-77 crossing".to_owned(),
        phase: state.phase.clone(),
        claimed,
    };
    let fact: wire::AnyFact =
        wire::decode(&wire::encode(&wire::Fact::new(closed)).map_err(|error| {
            retained(
                "fact_envelopes",
                state.next.to_string(),
                format!("closure does not encode: {error}"),
            )
        })?)
        .map_err(|error| {
            retained(
                "fact_envelopes",
                state.next.to_string(),
                format!("closure does not decode: {error}"),
            )
        })?;
    let origin: wire::Origin = format!("task:{task}").parse().map_err(|error| {
        retained(
            "fact_envelopes",
            state.next.to_string(),
            format!("invalid task origin: {error}"),
        )
    })?;
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: state.sequence,
        at: state.at,
        actor: wire::Actor::Spice,
        origin: origin.clone(),
        cause: None,
        fact,
    };
    let bytes = wire::encode(&envelope).map_err(|error| {
        retained(
            "fact_envelopes",
            state.next.to_string(),
            format!("closure envelope does not encode: {error}"),
        )
    })?;
    Ok((origin, bytes))
}

fn advance_workflow(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
    state: &ClosureState,
) -> Result<(), OpenError> {
    let mut workflow = state.workflow.clone();
    workflow.status = wire::WorkflowStatus::closed(state.at);
    workflow.deferral = None;
    workflow.generation = workflow.generation.checked_add(1).ok_or_else(|| {
        retained(
            "workflow_heads",
            task.to_string(),
            "workflow generation exhausted".to_owned(),
        )
    })?;
    let body = wire::encode(&workflow).map_err(|error| {
        retained(
            "workflow_heads",
            task.to_string(),
            format!("workflow does not encode: {error}"),
        )
    })?;
    transaction
        .execute(
            "UPDATE workflow_heads SET head_seq = ?1, body = ?2, revised_seq = NULL WHERE stamp = ?3",
            params![state.next, body, task.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    let generation = i64::try_from(workflow.generation).map_err(|error| {
        retained(
            "workflow_heads",
            task.to_string(),
            format!("workflow generation exceeds SQLite: {error}"),
        )
    })?;
    transaction
        .execute(
            "INSERT INTO task_generations(stamp, generation, seq, baseline) VALUES (?1, ?2, ?3, 0)",
            params![task.as_str(), generation, state.next],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn free_claim(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
    state: &ClosureState,
) -> Result<(), OpenError> {
    let Some(mut claim) = state.claim.clone().filter(wire::ClaimHead::is_active) else {
        return Ok(());
    };
    claim.fact = state.sequence;
    claim.disposition = wire::ClaimDisposition::Freed;
    let body = wire::encode(&claim).map_err(|error| {
        retained(
            "claim_heads",
            task.to_string(),
            format!("freed claim does not encode: {error}"),
        )
    })?;
    transaction
        .execute(
            "UPDATE claim_heads SET head_seq = ?1, body = ?2 WHERE stamp = ?3",
            params![state.next, body, task.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn advance_cursors(transaction: &Transaction<'_>, next: i64) -> Result<(), OpenError> {
    transaction
        .execute(
            "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE steering_index SET indexed_seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn retained(table: &'static str, row: String, detail: String) -> OpenError {
    residual::retained(table, row, DERIVED_CONTROL_RELEASE, detail)
}
