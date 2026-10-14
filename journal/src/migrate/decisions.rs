//! Exact crossing for the retired two-step preparation decision protocol.

use std::collections::BTreeMap;

use rusqlite::{Transaction, params};
use serde::Deserialize;
use serde_json::{Map, Value};
use wire::rooms::{
    CandidateRef, DecisionRef, ExecutionAuthorization, PreparationDecision, RetainedCandidate,
};
use wire::{AnyFact, Envelope, FactKind};

use super::{OpenError, residual::retained};

/// Release whose accepted decision and later authorization were separate acts.
const TWO_STEP_DECISION_RELEASE: &str = "128e75e97";
const RETAINED_RETURN_REASON: &str =
    "returned before preparation decisions required an explicit reason";

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum RetiredOutcome {
    Accepted { candidate: CandidateRef },
    Returned { candidate: CandidateRef },
    Negative { conclusion: Value },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredDecision {
    decision: DecisionRef,
    source: wire::rooms::RoomSource,
    outcome: RetiredOutcome,
}

/// Cross every retained two-step decision to one exact selected decision.
///
/// An accepted decision is supportable only when its later authorization names
/// the exact selected outputs. Returns retain their outcome and gain the one
/// truthful reason absent from the retired shape. A negative decision named no
/// candidate version, so it cannot be made exact and is refused with its owner.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let authorizations = retained_authorizations(transaction)?;
    let candidates = retained_candidates(transaction)?;
    cross_decision_facts(transaction, &authorizations, &candidates)?;
    cross_authorization_facts(transaction)?;
    cross_decision_heads(transaction, &authorizations, &candidates)?;
    cross_authorization_heads(transaction)
}

fn retained_authorizations(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<String, ExecutionAuthorization>, OpenError> {
    let rows = fact_rows(transaction, FactKind::ExecutionAuthorized)?;
    let mut authorizations = BTreeMap::new();
    for (seq, bytes) in rows {
        let authorization = fact_member(&bytes, seq, "authorization")?;
        let authorization: ExecutionAuthorization =
            serde_json::from_value(authorization).map_err(|error| {
                retained(
                    "fact_envelopes",
                    seq.to_string(),
                    TWO_STEP_DECISION_RELEASE,
                    format!("the retained execution authorization is unsupported: {error}"),
                )
            })?;
        let key = decision_key(&authorization.decision).map_err(|detail| {
            retained(
                "fact_envelopes",
                seq.to_string(),
                TWO_STEP_DECISION_RELEASE,
                detail,
            )
        })?;
        if authorizations.insert(key, authorization).is_some() {
            return Err(retained(
                "fact_envelopes",
                seq.to_string(),
                TWO_STEP_DECISION_RELEASE,
                "more than one retained authorization names the same decision".to_owned(),
            ));
        }
    }
    Ok(authorizations)
}

fn retained_candidates(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<String, RetainedCandidate>, OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT subject, body FROM proposal_heads \
                 WHERE json_type(CAST(body AS TEXT), '$.outputs') = 'array' \
                 ORDER BY subject",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    let mut candidates = BTreeMap::new();
    for (subject, body) in rows {
        let candidate: RetainedCandidate = wire::decode(&body).map_err(|error| {
            retained(
                "proposal_heads",
                subject.clone(),
                TWO_STEP_DECISION_RELEASE,
                format!("the retained candidate is unsupported: {error}"),
            )
        })?;
        let key = candidate_key(&candidate.candidate).map_err(|detail| {
            retained(
                "proposal_heads",
                subject.clone(),
                TWO_STEP_DECISION_RELEASE,
                detail,
            )
        })?;
        candidates.insert(key, candidate);
    }
    Ok(candidates)
}

fn cross_decision_facts(
    transaction: &Transaction<'_>,
    authorizations: &BTreeMap<String, ExecutionAuthorization>,
    candidates: &BTreeMap<String, RetainedCandidate>,
) -> Result<(), OpenError> {
    for (seq, bytes) in fact_rows(transaction, FactKind::PreparationDecided)? {
        let mut envelope = value(&bytes, "fact_envelopes", seq.to_string())?;
        let fact = fact_object(&mut envelope, seq)?;
        let retired: RetiredDecision = serde_json::from_value(
            fact.get("decision")
                .cloned()
                .ok_or_else(|| fact_refusal(seq, "the retained decision has no decision body"))?,
        )
        .map_err(|error| {
            fact_refusal(
                seq,
                &format!("the retained decision is unsupported: {error}"),
            )
        })?;
        let current = current_decision(retired, authorizations, candidates)
            .map_err(|detail| fact_refusal(seq, &detail))?;
        fact.insert(
            "decision".to_owned(),
            serde_json::to_value(current).map_err(|error| {
                fact_refusal(seq, &format!("the crossed decision cannot encode: {error}"))
            })?,
        );
        update_fact(transaction, seq, FactKind::PreparationDecided, envelope)?;
    }
    Ok(())
}

fn cross_authorization_facts(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for (seq, bytes) in fact_rows(transaction, FactKind::ExecutionAuthorized)? {
        let mut envelope = value(&bytes, "fact_envelopes", seq.to_string())?;
        let fact = fact_object(&mut envelope, seq)?;
        let authorization = fact
            .get_mut("authorization")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| fact_refusal(seq, "the retained authorization has no object body"))?;
        authorization.remove("coverage");
        update_fact(transaction, seq, FactKind::ExecutionAuthorized, envelope)?;
    }
    Ok(())
}

fn cross_decision_heads(
    transaction: &Transaction<'_>,
    authorizations: &BTreeMap<String, ExecutionAuthorization>,
    candidates: &BTreeMap<String, RetainedCandidate>,
) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT subject, body FROM proposal_heads \
                 WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL \
                 ORDER BY subject",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (subject, bytes) in rows {
        let retired: RetiredDecision = wire::decode(&bytes).map_err(|error| {
            retained(
                "proposal_heads",
                subject.clone(),
                TWO_STEP_DECISION_RELEASE,
                format!("the retained decision head is unsupported: {error}"),
            )
        })?;
        let current = current_decision(retired, authorizations, candidates).map_err(|detail| {
            retained(
                "proposal_heads",
                subject.clone(),
                TWO_STEP_DECISION_RELEASE,
                detail,
            )
        })?;
        let body = wire::encode(&current).map_err(|error| {
            retained(
                "proposal_heads",
                subject.clone(),
                TWO_STEP_DECISION_RELEASE,
                format!("the crossed decision head cannot encode: {error}"),
            )
        })?;
        let changed = transaction
            .execute(
                "UPDATE proposal_heads SET body = ?1 WHERE subject = ?2",
                params![body, subject],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(retained(
                "proposal_heads",
                subject,
                TWO_STEP_DECISION_RELEASE,
                "the decision head changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn cross_authorization_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare("SELECT output, body FROM authorization_heads ORDER BY output")
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (output, bytes) in rows {
        let mut body = value(&bytes, "authorization_heads", output.clone())?;
        let object = body.as_object_mut().ok_or_else(|| {
            retained(
                "authorization_heads",
                output.clone(),
                TWO_STEP_DECISION_RELEASE,
                "the retained authorization head is not an object".to_owned(),
            )
        })?;
        object.remove("coverage");
        let current: ExecutionAuthorization = serde_json::from_value(body).map_err(|error| {
            retained(
                "authorization_heads",
                output.clone(),
                TWO_STEP_DECISION_RELEASE,
                format!("the retained authorization head is unsupported: {error}"),
            )
        })?;
        let body = wire::encode(&current).map_err(|error| {
            retained(
                "authorization_heads",
                output.clone(),
                TWO_STEP_DECISION_RELEASE,
                format!("the crossed authorization head cannot encode: {error}"),
            )
        })?;
        let changed = transaction
            .execute(
                "UPDATE authorization_heads SET body = ?1 WHERE output = ?2",
                params![body, output],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(retained(
                "authorization_heads",
                output,
                TWO_STEP_DECISION_RELEASE,
                "the authorization head changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn current_decision(
    retired: RetiredDecision,
    authorizations: &BTreeMap<String, ExecutionAuthorization>,
    candidates: &BTreeMap<String, RetainedCandidate>,
) -> Result<PreparationDecision, String> {
    let outcome = match retired.outcome {
        RetiredOutcome::Accepted { candidate } => {
            let key = decision_key(&retired.decision)?;
            let authorization = authorizations.get(&key).ok_or_else(|| {
                "an accepted retained decision has no exact authorization".to_owned()
            })?;
            if authorization.candidate != candidate {
                return Err(
                    "the retained decision and authorization name different candidates".to_owned(),
                );
            }
            let retained = candidates.get(&candidate_key(&candidate)?).ok_or_else(|| {
                "the retained authorization candidate has no proposal head".to_owned()
            })?;
            if authorization.governed_inputs.len() != retained.governed_inputs.len()
                || !authorization
                    .governed_inputs
                    .iter()
                    .all(|input| retained.governed_inputs.contains(input))
            {
                return Err(
                    "the retained authorization governed inputs differ from its candidate"
                        .to_owned(),
                );
            }
            if authorization
                .selected_outputs
                .iter()
                .any(|output| !retained.outputs.contains(output))
            {
                return Err(
                    "the retained authorization selects an output outside its candidate".to_owned(),
                );
            }
            wire::rooms::DecisionOutcome::Selected {
                candidate,
                selected_outputs: authorization.selected_outputs.clone(),
            }
        }
        RetiredOutcome::Returned { candidate } => wire::rooms::DecisionOutcome::Returned {
            candidate,
            reason: RETAINED_RETURN_REASON.to_owned(),
        },
        RetiredOutcome::Negative { conclusion } => {
            return Err(format!(
                "the retained {conclusion:?} decision names no exact candidate version"
            ));
        }
    };
    Ok(PreparationDecision {
        decision: retired.decision,
        source: retired.source,
        outcome,
    })
}

fn fact_rows(
    transaction: &Transaction<'_>,
    kind: FactKind,
) -> Result<Vec<(i64, Vec<u8>)>, OpenError> {
    let mut statement = transaction
        .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind = ?1 ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([kind.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn fact_member(bytes: &[u8], seq: i64, member: &str) -> Result<Value, OpenError> {
    let mut envelope = value(bytes, "fact_envelopes", seq.to_string())?;
    fact_object(&mut envelope, seq)?
        .get(member)
        .cloned()
        .ok_or_else(|| fact_refusal(seq, &format!("the retained fact has no {member} member")))
}

fn fact_object(envelope: &mut Value, seq: i64) -> Result<&mut Map<String, Value>, OpenError> {
    envelope
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| fact_refusal(seq, "the stored envelope carries no fact object"))
}

fn value(bytes: &[u8], table: &'static str, row: String) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes).map_err(|error| {
        retained(
            table,
            row,
            TWO_STEP_DECISION_RELEASE,
            format!("the retained JSON does not decode: {error}"),
        )
    })
}

fn update_fact(
    transaction: &Transaction<'_>,
    seq: i64,
    kind: FactKind,
    envelope: Value,
) -> Result<(), OpenError> {
    let bytes = serde_json::to_vec(&envelope).map_err(|error| {
        fact_refusal(seq, &format!("the crossed envelope cannot encode: {error}"))
    })?;
    let current: Envelope<AnyFact> = wire::decode(&bytes).map_err(|error| {
        fact_refusal(
            seq,
            &format!("the crossed envelope is not current: {error}"),
        )
    })?;
    let bytes = wire::encode(&current).map_err(|error| {
        fact_refusal(seq, &format!("the crossed envelope cannot encode: {error}"))
    })?;
    let changed = transaction
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2 AND kind = ?3",
            params![bytes, seq, kind.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(fact_refusal(
            seq,
            "the fact changed under the exclusive crossing",
        ));
    }
    Ok(())
}

fn decision_key(decision: &DecisionRef) -> Result<String, String> {
    serde_json::to_string(decision)
        .map_err(|error| format!("the retained decision identity is invalid: {error}"))
}

fn candidate_key(candidate: &CandidateRef) -> Result<String, String> {
    serde_json::to_string(candidate)
        .map_err(|error| format!("the retained candidate identity is invalid: {error}"))
}

fn fact_refusal(seq: i64, detail: &str) -> OpenError {
    retained(
        "fact_envelopes",
        seq.to_string(),
        TWO_STEP_DECISION_RELEASE,
        detail.to_owned(),
    )
}
