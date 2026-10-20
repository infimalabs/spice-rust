//! Retire required question-result evidence from task statements.
//!
//! Required evidence tied positive completion to the retired ASK result
//! lifecycle. The crossing removes it before any current statement decoder
//! runs: the derived `task_evidence` index is dropped, each retained
//! `task::edited` evidence change and each statement head's required set lose
//! only that member, and each `task::edit` receipt whose request carried an
//! evidence change moves into historical receipt authority, where it stays
//! verifiable and its command identity never executes again. A retained
//! proposal or proposal request that names evidence refuses the crossing.

use rusqlite::{Transaction, params};
use serde::Deserialize;
use serde_json::Value;

use super::{OpenError, residual};

/// Last release whose wire decoded required question evidence.
const TASK_EVIDENCE_RELEASE: &str = "963a54c37";

/// The evidence change shapes that release could write; parsed only here.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
enum RetiredEvidenceEdit {
    Require {
        #[serde(rename = "result")]
        _result: String,
    },
    Replace {
        #[serde(rename = "result")]
        _result: String,
        #[serde(rename = "with")]
        _with: String,
    },
    Waive {
        #[serde(rename = "result")]
        _result: String,
    },
}

fn evidence_present(column: &str, path: &str) -> String {
    format!("json_type(CAST({column} AS TEXT), '{path}') IS NOT NULL")
}

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    refuse_proposal_evidence(transaction)?;
    transaction
        .execute_batch("DROP TABLE IF EXISTS task_evidence;")
        .map_err(OpenError::Sqlite)?;
    residual::rewrite_facts(
        transaction,
        wire::FactKind::TaskEdited,
        &evidence_present("envelope", "$.fact.evidence"),
        TASK_EVIDENCE_RELEASE,
        |_, fact| retired_edit(fact.remove("evidence")),
    )?;
    cross_heads(transaction)?;
    cross_receipts(transaction)
}

fn retired_edit(edit: Option<Value>) -> Result<(), String> {
    let edit = edit.ok_or("the edit carries no evidence change")?;
    serde_json::from_value::<RetiredEvidenceEdit>(edit)
        .map(drop)
        .map_err(|error| format!("the evidence change is not a retired shape: {error}"))
}

fn refuse_proposal_evidence(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::refuse_first(
        transaction,
        "fact_envelopes",
        &format!(
            "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 AND {} LIMIT 1",
            evidence_present("envelope", "$.fact.candidate.evidence")
        ),
        [wire::FactKind::DesignProposed.as_str()],
        TASK_EVIDENCE_RELEASE,
        "a retained proposal names question-result evidence",
    )?;
    residual::refuse_first(
        transaction,
        "command_receipts",
        &format!(
            "SELECT command_id FROM command_receipts WHERE command_kind = ?1 AND {} LIMIT 1",
            evidence_present("request", "$.body.candidate.evidence")
        ),
        [wire::CommandKind::DesignPropose.as_str()],
        TASK_EVIDENCE_RELEASE,
        "a retained proposal request names question-result evidence",
    )
}

/// Remove each statement head's required set, keeping every other member.
fn cross_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT stamp, head_seq, body FROM task_heads WHERE {} ORDER BY stamp",
                evidence_present("body", "$.evidence")
            ))
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| {
            residual::retained("task_heads", stamp.clone(), TASK_EVIDENCE_RELEASE, detail)
        };
        let mut body: Value = serde_json::from_slice(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        let required = body
            .as_object_mut()
            .and_then(|statement| statement.remove("evidence"))
            .ok_or_else(|| refuse("the statement carries no evidence member".to_owned()))?;
        serde_json::from_value::<Vec<String>>(required).map_err(|error| {
            refuse(format!(
                "the required evidence is not a retired shape: {error}"
            ))
        })?;
        let bytes = wire::encode(&body)
            .map_err(|error| refuse(format!("the crossed statement cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                params![bytes, stamp, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Move each `task::edit` receipt whose request changed evidence into
/// historical receipt authority, byte for byte.
fn cross_receipts(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let kind = wire::CommandKind::TaskEdit.as_str();
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT command_id, request FROM command_receipts \
                 WHERE command_kind = ?1 AND {} ORDER BY command_id",
                evidence_present("request", "$.body.evidence")
            ))
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([kind], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    if rows.is_empty() {
        return Ok(());
    }
    crate::historical_receipts::create_schema(transaction)?;
    crate::historical_receipts::reserve_identities(transaction)?;
    for (command_id, request) in rows {
        let refuse = |detail: String| {
            residual::retained(
                "command_receipts",
                command_id.clone(),
                TASK_EVIDENCE_RELEASE,
                detail,
            )
        };
        let request: Value = serde_json::from_slice(&request)
            .map_err(|error| refuse(format!("the stored request does not decode: {error}")))?;
        retired_edit(request.pointer("/body/evidence").cloned()).map_err(refuse)?;
        let moved = transaction
            .execute(
                "INSERT INTO historical_command_receipts(command_id, actor, command_kind, \
                 request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, \
                 epoch) SELECT command_id, actor, command_kind, request, evaluated_witnesses, \
                 response, first_seq, last_seq, snapshot_seq, epoch FROM command_receipts \
                 WHERE command_id = ?1 AND command_kind = ?2",
                params![command_id, kind],
            )
            .map_err(OpenError::Sqlite)?;
        let removed = transaction
            .execute(
                "DELETE FROM command_receipts WHERE command_id = ?1 AND command_kind = ?2",
                params![command_id, kind],
            )
            .map_err(OpenError::Sqlite)?;
        if moved != 1 || removed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}
