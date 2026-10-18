//! Bind retained claim-backed Design notices to exact sessions and sources.

use std::collections::BTreeMap;

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::{
    AnyFact, CanonicalUuid, Envelope, FactKind, Seq, SequenceCode, TaskStamp,
    rooms::{DesignNoticeEvidence, DesignNoticeOpportunity, DesignSession},
};

use super::{MigrationError, OpenError};

/// Release that made exact Design statement evidence the notice authority.
const NOTICE_STATEMENT_RELEASE: &str = "ROOMS-dW6GpYK65";
/// Release that separated the invitation opportunity from its supporting card.
const DESIGN_INVITATION_RELEASE: &str = "ROOMS-dWKcWx0xN";
/// Retained envelopes decoded in one bounded crossing step.
const FACT_PAGE_ROWS: i64 = 256;

#[derive(Clone)]
struct Claim {
    task: TaskStamp,
    lane: CanonicalUuid,
}

#[derive(Clone, Copy)]
struct Terminal {
    at: Seq,
    completed: bool,
}

#[derive(Default)]
struct Witnesses {
    statements: BTreeMap<TaskStamp, Seq>,
    claims: BTreeMap<Seq, Claim>,
    terminals: BTreeMap<TaskStamp, Terminal>,
    sessions: BTreeMap<(CanonicalUuid, CanonicalUuid), DesignSession>,
}

struct Crossing {
    session: DesignSession,
    opportunity: DesignNoticeOpportunity,
    evidence: DesignNoticeEvidence,
}

/// Rewrite every retained claim-backed notice to the session-targeted shape.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let mut witnesses = Witnesses::default();
    let mut after = 0_i64;
    loop {
        let page = fact_page(transaction, after)?;
        let Some((last, ..)) = page.last() else {
            return Ok(());
        };
        after = *last;
        for (seq, kind, bytes) in page {
            if kind == FactKind::DesignNoticed.as_str() {
                cross_notice(transaction, seq, &bytes, &witnesses)?;
            } else {
                observe(seq, &bytes, &mut witnesses)?;
            }
        }
    }
}

fn observe(seq: i64, bytes: &[u8], witnesses: &mut Witnesses) -> Result<(), OpenError> {
    let envelope: Envelope<AnyFact> = wire::decode(bytes).map_err(|error| {
        refusal(
            seq,
            &format!("a retained invitation witness does not decode: {error}"),
        )
    })?;
    for task in envelope.fact.revised_tasks() {
        witnesses.statements.insert(task.clone(), envelope.seq);
    }
    if let Some(claim) = envelope.fact.read::<wire::body::ClaimWon>() {
        witnesses.claims.insert(
            envelope.seq,
            Claim {
                task: claim.task.clone(),
                lane: claim.lane,
            },
        );
    } else if let Some(claim) = envelope.fact.read::<wire::body::ClaimCarried>() {
        witnesses.claims.insert(
            envelope.seq,
            Claim {
                task: claim.task.clone(),
                lane: claim.lane,
            },
        );
    } else if let Some(claim) = envelope.fact.read::<wire::body::ClaimRebound>() {
        witnesses.claims.insert(
            envelope.seq,
            Claim {
                task: claim.task.clone(),
                lane: claim.lane,
            },
        );
    }
    if let Some(done) = envelope.fact.read::<wire::body::TaskDone>()
        && let Some(terminal) = &done.terminal
    {
        witnesses.terminals.insert(
            done.task.clone(),
            Terminal {
                at: envelope.seq,
                completed: terminal.status == wire::body::TaskCompletionStatus::Completed,
            },
        );
    }
    if let Some(declared) = envelope.fact.read::<wire::body::LaneSessionDeclared>() {
        witnesses.sessions.insert(
            (declared.lane, declared.occupant),
            DesignSession {
                vendor: declared.vendor.clone(),
                identity: declared.identity.clone(),
            },
        );
    }
    Ok(())
}

fn cross_notice(
    transaction: &Transaction<'_>,
    seq: i64,
    bytes: &[u8],
    witnesses: &Witnesses,
) -> Result<(), OpenError> {
    let mut envelope: Value = serde_json::from_slice(bytes).map_err(|error| {
        refusal(
            seq,
            &format!("the retained envelope does not decode: {error}"),
        )
    })?;
    {
        let fact = envelope
            .get_mut("fact")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| refusal(seq, "the retained envelope carries no fact object"))?;
        if fact.contains_key("session") {
            return Ok(());
        }
        recover_design_statement(seq, fact, witnesses)?;
        let crossing = derive_crossing(seq, fact, witnesses)?;
        apply_crossing(seq, fact, crossing)?;
    }
    store_crossed_notice(transaction, seq, envelope)
}

fn recover_design_statement(
    seq: i64,
    fact: &mut Map<String, Value>,
    witnesses: &Witnesses,
) -> Result<(), OpenError> {
    if !fact.contains_key("statement") {
        let design = required_string(fact, "design", seq)?
            .parse::<TaskStamp>()
            .map_err(|error| {
                statement_refusal(seq, &format!("the retained Design is invalid: {error}"))
            })?;
        let statement = witnesses.statements.get(&design).copied().ok_or_else(|| {
            statement_refusal(
                seq,
                "the retained notice predates any provable Design statement",
            )
        })?;
        fact.insert("statement".to_owned(), Value::from(statement.get()));
    }
    fact.remove("provenance");
    Ok(())
}

fn derive_crossing(
    seq: i64,
    fact: &Map<String, Value>,
    witnesses: &Witnesses,
) -> Result<Crossing, OpenError> {
    let task = required_string(fact, "task", seq)?
        .parse::<TaskStamp>()
        .map_err(|error| refusal(seq, &format!("the retained task is invalid: {error}")))?;
    let lane = required_string(fact, "lane", seq)?
        .parse::<CanonicalUuid>()
        .map_err(|error| refusal(seq, &format!("the retained lane is invalid: {error}")))?;
    let occupant = required_string(fact, "occupant", seq)?
        .parse::<CanonicalUuid>()
        .map_err(|error| refusal(seq, &format!("the retained occupant is invalid: {error}")))?;
    let claim = required_string(fact, "claim", seq)?
        .parse::<SequenceCode>()
        .map_err(|error| refusal(seq, &format!("the retained claim is invalid: {error}")))?;
    let claim_seq = validate_claim(seq, witnesses, &task, lane, claim)?;
    let statement = witnesses
        .statements
        .get(&task)
        .copied()
        .ok_or_else(|| refusal(seq, "the retained notice task has no provable statement"))?;
    let session = witnesses
        .sessions
        .get(&(lane, occupant))
        .cloned()
        .ok_or_else(|| {
            refusal(
                seq,
                "the retained notice recipient has no declared vendor session",
            )
        })?;
    let terminal = witnesses
        .terminals
        .get(&task)
        .copied()
        .filter(|terminal| terminal.at > claim_seq);
    let (opportunity, evidence) = task_witnesses(task, statement, claim, terminal);
    Ok(Crossing {
        session,
        opportunity,
        evidence,
    })
}

fn validate_claim(
    seq: i64,
    witnesses: &Witnesses,
    task: &TaskStamp,
    lane: CanonicalUuid,
    claim: SequenceCode,
) -> Result<Seq, OpenError> {
    let claim_seq = Seq::new(claim.get())
        .ok_or_else(|| refusal(seq, "the retained notice names claim zero"))?;
    let claimed = witnesses
        .claims
        .get(&claim_seq)
        .ok_or_else(|| refusal(seq, &format!("the retained claim {claim} does not exist")))?;
    if claimed.task != *task || claimed.lane != lane {
        return Err(refusal(
            seq,
            &format!("the retained claim {claim} does not bind task {task} to lane {lane}"),
        ));
    }
    Ok(claim_seq)
}

fn task_witnesses(
    task: TaskStamp,
    statement: Seq,
    claim: SequenceCode,
    terminal: Option<Terminal>,
) -> (DesignNoticeOpportunity, DesignNoticeEvidence) {
    match terminal {
        Some(terminal) if terminal.completed => (
            DesignNoticeOpportunity::TaskCompletion {
                task: task.clone(),
                completed: terminal.at,
            },
            DesignNoticeEvidence::Task {
                task,
                statement,
                completed: terminal.at,
            },
        ),
        terminal => {
            let terminal = terminal.map(|terminal| terminal.at);
            (
                DesignNoticeOpportunity::TaskClaim {
                    task: task.clone(),
                    claim,
                    terminal,
                },
                DesignNoticeEvidence::TaskClaim {
                    task,
                    statement,
                    claim,
                    terminal,
                },
            )
        }
    }
}

fn apply_crossing(
    seq: i64,
    fact: &mut Map<String, Value>,
    crossing: Crossing,
) -> Result<(), OpenError> {
    for (field, value) in [
        (
            "session",
            serde_json::to_value(crossing.session).map_err(|error| {
                refusal(
                    seq,
                    &format!("the retained session identity cannot encode: {error}"),
                )
            })?,
        ),
        (
            "opportunity",
            serde_json::to_value(crossing.opportunity).map_err(|error| {
                refusal(
                    seq,
                    &format!("the retained opportunity witness cannot encode: {error}"),
                )
            })?,
        ),
        (
            "evidence",
            serde_json::to_value(crossing.evidence).map_err(|error| {
                refusal(
                    seq,
                    &format!("the retained evidence witness cannot encode: {error}"),
                )
            })?,
        ),
    ] {
        fact.insert(field.to_owned(), value);
    }
    fact.remove("task");
    fact.remove("claim");
    Ok(())
}

fn store_crossed_notice(
    transaction: &Transaction<'_>,
    seq: i64,
    envelope: Value,
) -> Result<(), OpenError> {
    let current: Envelope<AnyFact> = serde_json::from_value(envelope).map_err(|error| {
        refusal(
            seq,
            &format!("the crossed envelope is not current: {error}"),
        )
    })?;
    let bytes = wire::encode(&current)
        .map_err(|error| refusal(seq, &format!("the crossed envelope cannot encode: {error}")))?;
    let changed = transaction
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2 AND kind = ?3",
            params![bytes, seq, FactKind::DesignNoticed.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(refusal(
            seq,
            "the retained notice changed under the exclusive crossing",
        ));
    }
    Ok(())
}

fn fact_page(
    transaction: &Transaction<'_>,
    after: i64,
) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
    let mut statement = transaction
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes WHERE seq > ?1 \
             ORDER BY seq LIMIT ?2",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map(params![after, FACT_PAGE_ROWS], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(OpenError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn required_string<'a>(
    fact: &'a Map<String, Value>,
    field: &str,
    seq: i64,
) -> Result<&'a str, OpenError> {
    fact.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| refusal(seq, &format!("the retained notice names no {field}")))
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row: seq.to_string(),
        release: DESIGN_INVITATION_RELEASE,
        detail: detail.to_owned(),
    }
    .into()
}

fn statement_refusal(seq: i64, detail: &str) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row: seq.to_string(),
        release: NOTICE_STATEMENT_RELEASE,
        detail: detail.to_owned(),
    }
    .into()
}
