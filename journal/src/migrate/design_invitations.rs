//! Bind retained Design notices to exact sessions, sources, context
//! boundaries, and supporting-work presentation.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value};
use wire::{
    AnyFact, CanonicalUuid, Envelope, FactKind, Phase, Seq, SequenceCode, TaskStamp,
    rooms::{
        DesignNoticeEvidence, DesignNoticeOpportunity, DesignSession, DesignSupport, design_excerpt,
    },
};

use super::{OpenError, residual};

/// Release that made exact Design statement evidence the notice authority.
const NOTICE_STATEMENT_RELEASE: &str = "ROOMS-dW6GpYK65";
/// Release that separated the invitation opportunity from its supporting card.
const DESIGN_INVITATION_RELEASE: &str = "ROOMS-dWKcWx0xN";
/// Release that bound an invitation to its compaction-delimited context.
const CONTEXT_RELEASE: &str = "ROOMS-dWKwb7jw9";
/// Release that recorded an invitation's supporting-work presentation.
const PRESENTATION_RELEASE: &str = "ROOMS-dWL301SYq";
/// Excerpt of a retained conversation notice whose writer kept no prose. The
/// cleaned prose lived only in the resident's session profile, so the excerpt
/// is its own elision mark: the card quotes nothing it cannot prove and still
/// names the exact transcript source holding the prose.
const ELIDED_CONVERSATION_EXCERPT: &str = "…";
/// Retained envelopes decoded in one bounded crossing step.
const FACT_PAGE_ROWS: i64 = 256;

#[derive(Clone)]
struct Claim {
    task: TaskStamp,
    lane: CanonicalUuid,
    support: Option<DesignSupport>,
}

#[derive(Clone, Copy)]
struct Terminal {
    at: Seq,
    completed: bool,
}

/// One terminal task operation's presentation as supporting work.
struct Completion {
    excerpt: String,
    support: Option<DesignSupport>,
}

#[derive(Default)]
struct Witnesses {
    statements: BTreeMap<TaskStamp, Seq>,
    claims: BTreeMap<Seq, Claim>,
    /// Phase of each task's newest won claim; carried and rebound claims keep
    /// that lineage.
    phases: BTreeMap<TaskStamp, Phase>,
    terminals: BTreeMap<TaskStamp, Terminal>,
    completions: BTreeMap<Seq, Completion>,
    sessions: BTreeMap<(CanonicalUuid, CanonicalUuid), DesignSession>,
    compactions: BTreeMap<(CanonicalUuid, DesignSession), Seq>,
}

struct Crossing {
    session: DesignSession,
    opportunity: DesignNoticeOpportunity,
    evidence: DesignNoticeEvidence,
}

/// Rewrite every retained notice to the session-targeted, context-bound shape.
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
        witnesses
            .phases
            .insert(claim.task.clone(), claim.cell.phase.clone());
        observe_claim(envelope.seq, &claim.task, claim.lane, witnesses);
    } else if let Some(claim) = envelope.fact.read::<wire::body::ClaimCarried>() {
        observe_claim(envelope.seq, &claim.task, claim.lane, witnesses);
    } else if let Some(claim) = envelope.fact.read::<wire::body::ClaimRebound>() {
        observe_claim(envelope.seq, &claim.task, claim.lane, witnesses);
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
        witnesses.completions.insert(
            envelope.seq,
            Completion {
                excerpt: design_excerpt(&done.validation),
                support: witnesses
                    .phases
                    .get(&done.task)
                    .map(DesignSupport::for_phase),
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
    } else if let Some(compacted) = envelope.fact.read::<wire::body::LaneCompacted>()
        && let Some(session) = witnesses
            .sessions
            .get(&(compacted.lane, compacted.occupant))
            .cloned()
    {
        witnesses
            .compactions
            .insert((compacted.lane, session), envelope.seq);
    }
    Ok(())
}

fn observe_claim(seq: Seq, task: &TaskStamp, lane: CanonicalUuid, witnesses: &mut Witnesses) {
    witnesses.claims.insert(
        seq,
        Claim {
            task: task.clone(),
            lane,
            support: witnesses.phases.get(task).map(DesignSupport::for_phase),
        },
    );
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
    let changed = {
        let fact = envelope
            .get_mut("fact")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| refusal(seq, "the retained envelope carries no fact object"))?;
        let mut changed = false;
        if !fact.contains_key("session") {
            recover_design_statement(seq, fact, witnesses)?;
            let crossing = derive_crossing(seq, fact, witnesses)?;
            apply_crossing(seq, fact, crossing)?;
            changed = true;
        }
        if !fact.contains_key("compaction") {
            bind_compaction(seq, fact, witnesses)?;
            changed = true;
        }
        if !fact.contains_key("excerpt") {
            bind_presentation(transaction, seq, fact, witnesses)?;
            changed = true;
        }
        if changed {
            super::design_admission::admit(fact).map_err(|detail| refusal(seq, &detail))?;
        }
        changed
    };
    if changed {
        store_crossed_notice(transaction, seq, envelope)
    } else {
        Ok(())
    }
}

/// Record the supporting work's classification and bounded excerpt from the
/// claim and terminal operation the evidence names, or from the task's
/// current title when its claim reached no terminal operation. Conversation
/// evidence kept no prose in retained authority, so its excerpt is elided.
fn bind_presentation(
    transaction: &Transaction<'_>,
    seq: i64,
    fact: &mut Map<String, Value>,
    witnesses: &Witnesses,
) -> Result<(), OpenError> {
    let conversation = fact
        .get("evidence")
        .and_then(|evidence| evidence.get("kind"))
        .and_then(Value::as_str)
        == Some("conversation");
    if conversation {
        fact.insert(
            "excerpt".to_owned(),
            Value::from(ELIDED_CONVERSATION_EXCERPT),
        );
        return Ok(());
    }
    let evidence = fact
        .get_mut("evidence")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| presentation_refusal(seq, "the retained notice names no evidence"))?;
    let position = |field: &str| {
        evidence
            .get(field)
            .and_then(Value::as_u64)
            .and_then(Seq::new)
    };
    let (support, excerpt) = match evidence.get("kind").and_then(Value::as_str) {
        Some("task") => {
            let completion = position("completed")
                .and_then(|completed| witnesses.completions.get(&completed))
                .ok_or_else(|| {
                    presentation_refusal(seq, "the retained task evidence names no completion")
                })?;
            (completion.support, completion.excerpt.clone())
        }
        Some("task-claim") => {
            let claim = evidence
                .get("claim")
                .and_then(Value::as_str)
                .and_then(|claim| claim.parse::<SequenceCode>().ok())
                .and_then(|claim| Seq::new(claim.get()))
                .and_then(|claim| witnesses.claims.get(&claim))
                .ok_or_else(|| {
                    presentation_refusal(seq, "the retained task evidence names no claim")
                })?;
            let excerpt = match position("terminal") {
                Some(terminal) => witnesses
                    .completions
                    .get(&terminal)
                    .ok_or_else(|| {
                        presentation_refusal(
                            seq,
                            "the retained task evidence names no terminal operation",
                        )
                    })?
                    .excerpt
                    .clone(),
                None => String::new(),
            };
            (claim.support, excerpt)
        }
        _ => {
            return Err(presentation_refusal(
                seq,
                "the retained notice names no supporting evidence kind",
            ));
        }
    };
    let support = support.ok_or_else(|| {
        presentation_refusal(seq, "the supporting task never won a claim with a phase")
    })?;
    let excerpt = if excerpt.is_empty() {
        let task = evidence
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| presentation_refusal(seq, "the retained task evidence names no task"))?;
        design_excerpt(&current_title(transaction, seq, task)?)
    } else {
        excerpt
    };
    if excerpt.is_empty() {
        return Err(presentation_refusal(
            seq,
            "the supporting task has no authored text to excerpt",
        ));
    }
    if !evidence.contains_key("support") {
        let support = serde_json::to_value(support).map_err(|error| {
            presentation_refusal(seq, &format!("the support class cannot encode: {error}"))
        })?;
        evidence.insert("support".to_owned(), support);
    }
    fact.insert("excerpt".to_owned(), Value::from(excerpt));
    Ok(())
}

fn current_title(transaction: &Transaction<'_>, seq: i64, task: &str) -> Result<String, OpenError> {
    transaction
        .query_row(
            "SELECT json_extract(CAST(body AS TEXT), '$.title') FROM task_heads WHERE stamp = ?1",
            [task],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .flatten()
        .ok_or_else(|| presentation_refusal(seq, "the supporting task has no current statement"))
}

fn bind_compaction(
    seq: i64,
    fact: &mut Map<String, Value>,
    witnesses: &Witnesses,
) -> Result<(), OpenError> {
    let lane = fact
        .get("lane")
        .and_then(Value::as_str)
        .ok_or_else(|| context_refusal(seq, "the retained notice names no lane"))?
        .parse::<CanonicalUuid>()
        .map_err(|error| context_refusal(seq, &format!("the retained lane is invalid: {error}")))?;
    let session = fact
        .get("session")
        .cloned()
        .ok_or_else(|| context_refusal(seq, "the retained notice names no session"))
        .and_then(|value| {
            serde_json::from_value::<DesignSession>(value).map_err(|error| {
                context_refusal(seq, &format!("the retained session is invalid: {error}"))
            })
        })?;
    let compaction = witnesses.compactions.get(&(lane, session)).copied();
    fact.insert(
        "compaction".to_owned(),
        compaction.map_or(Value::Null, |compaction| Value::from(compaction.get())),
    );
    Ok(())
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
    let (claim_seq, support) = validate_claim(seq, witnesses, &task, lane, claim)?;
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
    let (opportunity, evidence) = task_witnesses(task, statement, claim, terminal, support);
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
) -> Result<(Seq, DesignSupport), OpenError> {
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
    let support = claimed.support.ok_or_else(|| {
        presentation_refusal(seq, "the supporting task never won a claim with a phase")
    })?;
    Ok((claim_seq, support))
}

fn task_witnesses(
    task: TaskStamp,
    statement: Seq,
    claim: SequenceCode,
    terminal: Option<Terminal>,
    support: DesignSupport,
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
                support,
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
                    support,
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
        context_refusal(
            seq,
            &format!("the crossed envelope is not current: {error}"),
        )
    })?;
    let bytes = wire::encode(&current).map_err(|error| {
        context_refusal(seq, &format!("the crossed envelope cannot encode: {error}"))
    })?;
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
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        DESIGN_INVITATION_RELEASE,
        detail.to_owned(),
    )
}

fn context_refusal(seq: i64, detail: &str) -> OpenError {
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        CONTEXT_RELEASE,
        detail.to_owned(),
    )
}

fn statement_refusal(seq: i64, detail: &str) -> OpenError {
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        NOTICE_STATEMENT_RELEASE,
        detail.to_owned(),
    )
}

fn presentation_refusal(seq: i64, detail: &str) -> OpenError {
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        PRESENTATION_RELEASE,
        detail.to_owned(),
    )
}
