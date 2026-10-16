//! Exact crossings for retired preparation proposal and decision protocols.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Map, Value};
use wire::rooms::{
    CandidateRef, DecisionOutcome, DecisionRef, DesignPassProposal, DesignPropose,
    ExecutionAuthorization, OutputDecision, PreparationDecision,
};
use wire::{AnyFact, Envelope, FactKind};

use super::{OpenError, residual::retained};

/// Release whose accepted decision and later authorization were separate acts.
const TWO_STEP_DECISION_RELEASE: &str = "128e75e97";
const RETAINED_RETURN_REASON: &str =
    "returned before preparation decisions required an explicit reason";
const DESIGN_PASS_RELEASE: &str = "schema-73";
const RETAINED_WORK_JUSTIFICATION: &str =
    "retained work proposal predates explicit Design pass justification";

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

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum PreviousOutcome {
    Selected {
        candidate: CandidateRef,
        selected_outputs: Vec<wire::TaskStamp>,
    },
    Returned {
        candidate: CandidateRef,
        reason: String,
    },
    Resolved {
        candidate: CandidateRef,
        outputs: Vec<OutputDecision>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    Collapse {
        candidate: CandidateRef,
        reason: String,
    },
}

impl PreviousOutcome {
    const fn candidate(&self) -> &CandidateRef {
        match self {
            Self::Selected { candidate, .. }
            | Self::Returned { candidate, .. }
            | Self::Resolved { candidate, .. }
            | Self::Collapse { candidate, .. } => candidate,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousDecision {
    decision: DecisionRef,
    source: wire::rooms::RoomSource,
    outcome: PreviousOutcome,
}

/// Cross every retained two-step decision to one exact selected decision.
///
/// An accepted decision is supportable only when its later authorization names
/// the exact selected outputs. Returns retain their outcome and gain the one
/// truthful reason absent from the retired shape. A negative decision named no
/// candidate version, so it cannot be made exact and is refused with its owner.
pub(super) fn cross_two_step(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let authorizations = retained_authorizations(transaction)?;
    let candidates = retained_candidates(transaction)?;
    cross_decision_facts(transaction, &authorizations, &candidates)?;
    cross_authorization_facts(transaction)?;
    cross_decision_heads(transaction, &authorizations, &candidates)?;
    cross_authorization_heads(transaction)
}

/// Add the exact statement and available claim horizon to each output-only
/// predecessor.
///
/// The receipt's evaluated witnesses are the immutable authority the old
/// command actually used. Some predecessor receipts predate claim witnesses;
/// others name a non-active claim head. Either crosses as explicit historical
/// absence rather than inventing authority the receipt never carried.
pub(super) fn cross_design_passes(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let collapses = previous_collapses(transaction)?;
    for (seq, bytes) in fact_rows(transaction, FactKind::DesignProposed)? {
        let mut envelope = value(&bytes, "fact_envelopes", seq.to_string())?;
        let fact = fact_object(&mut envelope, seq)?;
        let Some(candidate) = fact.get("candidate").cloned() else {
            return Err(proposal_refusal(
                seq,
                "the retained proposal has no candidate body",
            ));
        };
        if candidate.get("statement").is_some() {
            continue;
        }
        let identity: CandidateRef =
            serde_json::from_value(candidate.get("candidate").cloned().ok_or_else(|| {
                proposal_refusal(seq, "the retained proposal has no candidate identity")
            })?)
            .map_err(|error| {
                proposal_refusal(
                    seq,
                    &format!("the retained proposal identity is invalid: {error}"),
                )
            })?;
        let (command_id, request, witnesses) = proposal_receipt(transaction, seq)?;
        let (statement, claim) = proposal_horizon(transaction, &identity.controller, &witnesses)
            .map_err(|detail| proposal_refusal(seq, &detail))?;
        let collapse = collapses
            .get(&candidate_key(&identity).map_err(|detail| proposal_refusal(seq, &detail))?);
        let mut crossed = candidate;
        let object = crossed
            .as_object_mut()
            .expect("a retained proposal candidate decoded as an object");
        object.insert(
            "statement".to_owned(),
            serde_json::to_value(statement).expect("a sequence encodes"),
        );
        object.insert(
            "claim".to_owned(),
            serde_json::to_value(claim).expect("a sequence encodes"),
        );
        if let Some(reason) = collapse {
            object.insert(
                "recommendation".to_owned(),
                Value::String("collapse".to_owned()),
            );
            object.insert("governed_inputs".to_owned(), Value::Array(Vec::new()));
            object.insert("outputs".to_owned(), Value::Array(Vec::new()));
            object.insert("justification".to_owned(), Value::String(reason.clone()));
        } else {
            object.insert(
                "recommendation".to_owned(),
                Value::String("work".to_owned()),
            );
            object.insert(
                "justification".to_owned(),
                Value::String(RETAINED_WORK_JUSTIFICATION.to_owned()),
            );
        }
        let crossed: DesignPassProposal = serde_json::from_value(crossed).map_err(|error| {
            proposal_refusal(
                seq,
                &format!("the retained proposal cannot cross to one Design pass: {error}"),
            )
        })?;
        fact.insert(
            "candidate".to_owned(),
            serde_json::to_value(&crossed).map_err(|error| {
                proposal_refusal(seq, &format!("the crossed proposal cannot encode: {error}"))
            })?,
        );
        update_fact(transaction, seq, FactKind::DesignProposed, envelope)?;
        cross_proposal_head(transaction, seq, &crossed)?;
        cross_proposal_receipt(transaction, seq, &command_id, request, crossed)?;
    }
    Ok(())
}
fn previous_collapses(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<String, String>, OpenError> {
    let mut collapses = BTreeMap::new();
    for (seq, bytes) in fact_rows(transaction, FactKind::PreparationDecided)? {
        let mut envelope = value(&bytes, "fact_envelopes", seq.to_string())?;
        let fact = fact_object(&mut envelope, seq)?;
        let Some(outcome) = fact
            .get("decision")
            .and_then(|decision| decision.get("outcome"))
            .filter(|outcome| outcome.get("kind").and_then(Value::as_str) == Some("collapse"))
        else {
            continue;
        };
        let candidate: CandidateRef = serde_json::from_value(
            outcome
                .get("candidate")
                .cloned()
                .ok_or_else(|| proposal_refusal(seq, "a collapse has no candidate identity"))?,
        )
        .map_err(|error| {
            proposal_refusal(seq, &format!("a collapse candidate is invalid: {error}"))
        })?;
        let reason = outcome
            .get("reason")
            .and_then(Value::as_str)
            .ok_or_else(|| proposal_refusal(seq, "a collapse has no reason"))?
            .to_owned();
        let key = candidate_key(&candidate).map_err(|detail| proposal_refusal(seq, &detail))?;
        if collapses.insert(key, reason).is_some() {
            return Err(proposal_refusal(
                seq,
                "more than one collapse decision names the same proposal",
            ));
        }
    }
    Ok(collapses)
}

fn proposal_receipt(
    transaction: &Transaction<'_>,
    seq: i64,
) -> Result<(String, Vec<u8>, Vec<wire::Witness>), OpenError> {
    let row = transaction
        .query_row(
            "SELECT command_id, request, evaluated_witnesses FROM command_receipts \
             WHERE first_seq <= ?1 AND last_seq >= ?1 AND command_kind = ?2",
            params![seq, wire::CommandKind::DesignPropose.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .ok_or_else(|| {
            proposal_refusal(seq, "the retained proposal has no owning command receipt")
        })?;
    let witnesses = wire::decode(&row.2).map_err(|error| {
        proposal_refusal(
            seq,
            &format!("the retained proposal witnesses do not decode: {error}"),
        )
    })?;
    Ok((row.0, row.1, witnesses))
}

fn proposal_horizon(
    transaction: &Transaction<'_>,
    controller: &wire::TaskStamp,
    witnesses: &[wire::Witness],
) -> Result<(wire::Seq, Option<wire::Seq>), String> {
    let statement = witnesses.iter().find_map(|witness| match witness {
        wire::Witness::Task { task, head } if task == controller => Some(*head),
        _ => None,
    });
    let claim = witnesses.iter().find_map(|witness| match witness {
        wire::Witness::Claim {
            task,
            head: Some(head),
        } if task == controller => Some(*head),
        _ => None,
    });
    let statement = statement.ok_or_else(|| {
        format!("retained proposal controller {controller} lacks an exact statement witness")
    })?;
    let claim = claim
        .map(|claim| historical_active_claim(transaction, controller, claim))
        .transpose()?
        .flatten();
    Ok((statement, claim))
}

fn historical_active_claim(
    transaction: &Transaction<'_>,
    controller: &wire::TaskStamp,
    claim: wire::Seq,
) -> Result<Option<wire::Seq>, String> {
    let seq = i64::try_from(claim.get())
        .map_err(|_| format!("retained proposal claim {claim} does not fit SQLite"))?;
    let bytes = transaction
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|error| format!("retained proposal claim {claim} cannot be read: {error}"))?
        .ok_or_else(|| format!("retained proposal claim {claim} does not exist"))?;
    let envelope: Envelope<AnyFact> = wire::decode(&bytes)
        .map_err(|error| format!("retained proposal claim {claim} does not decode: {error}"))?;
    if envelope.fact.task_subject() != Some(controller) {
        return Err(format!(
            "retained proposal claim {claim} does not address controller {controller}"
        ));
    }
    match envelope.fact.kind() {
        FactKind::ClaimWon | FactKind::ClaimCarried | FactKind::ClaimRebound => Ok(Some(claim)),
        FactKind::ClaimFreed | FactKind::ClaimLapsed => Ok(None),
        kind => Err(format!(
            "retained proposal claim {claim} names unsupported fact {kind}"
        )),
    }
}

fn cross_proposal_head(
    transaction: &Transaction<'_>,
    seq: i64,
    proposal: &DesignPassProposal,
) -> Result<(), OpenError> {
    let key = serde_json::to_string(&wire::rooms::ProposalRef::Candidate {
        candidate: proposal.candidate.clone(),
    })
    .map_err(|error| {
        proposal_refusal(
            seq,
            &format!("the Design pass proposal identity is invalid: {error}"),
        )
    })?;
    let body = wire::encode(proposal).map_err(|error| {
        proposal_refusal(
            seq,
            &format!("the crossed proposal head cannot encode: {error}"),
        )
    })?;
    let changed = transaction
        .execute(
            "UPDATE proposal_heads SET body = ?1 WHERE subject = ?2",
            params![body, key],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(proposal_refusal(
            seq,
            "the retained proposal has no exact proposal head",
        ));
    }
    Ok(())
}

fn cross_proposal_receipt(
    transaction: &Transaction<'_>,
    seq: i64,
    command_id: &str,
    bytes: Vec<u8>,
    proposal: DesignPassProposal,
) -> Result<(), OpenError> {
    let mut request: Value = wire::decode(&bytes).map_err(|error| {
        proposal_refusal(
            seq,
            &format!("the retained proposal request does not decode: {error}"),
        )
    })?;
    let candidate = request
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| proposal_refusal(seq, "the retained proposal request has no body object"))?;
    candidate.insert(
        "candidate".to_owned(),
        serde_json::to_value(proposal).map_err(|error| {
            proposal_refusal(
                seq,
                &format!("the crossed proposal request cannot encode: {error}"),
            )
        })?,
    );
    let request: wire::Command<DesignPropose> =
        serde_json::from_value(request).map_err(|error| {
            proposal_refusal(
                seq,
                &format!("the crossed proposal request is invalid: {error}"),
            )
        })?;
    let bytes = request
        .canonical_json()
        .map_err(|error| {
            proposal_refusal(
                seq,
                &format!("the crossed proposal request cannot encode: {error}"),
            )
        })?
        .into_bytes();
    let changed = transaction
        .execute(
            "UPDATE command_receipts SET request = ?1 WHERE command_id = ?2",
            params![bytes, command_id],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(proposal_refusal(
            seq,
            "the retained proposal receipt changed under the exclusive crossing",
        ));
    }
    Ok(())
}

/// Cross the immediately preceding selected/resolved/returned/collapse model
/// after its proposal bodies have gained their exact pass horizon.
pub(super) fn cross_output_decisions(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let candidates = retained_candidates(transaction)?;
    for (seq, bytes) in fact_rows(transaction, FactKind::PreparationDecided)? {
        let mut envelope = value(&bytes, "fact_envelopes", seq.to_string())?;
        let fact = fact_object(&mut envelope, seq)?;
        let previous: PreviousDecision = serde_json::from_value(
            fact.get("decision")
                .cloned()
                .ok_or_else(|| proposal_refusal(seq, "the decision has no body"))?,
        )
        .map_err(|error| {
            proposal_refusal(
                seq,
                &format!("the previous Design pass decision is unsupported: {error}"),
            )
        })?;
        let current = current_output_decision(previous, &candidates)
            .map_err(|detail| proposal_refusal(seq, &detail))?;
        fact.insert(
            "decision".to_owned(),
            serde_json::to_value(current).map_err(|error| {
                proposal_refusal(seq, &format!("the crossed decision cannot encode: {error}"))
            })?,
        );
        update_fact(transaction, seq, FactKind::PreparationDecided, envelope)?;
    }

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
        let previous: PreviousDecision = wire::decode(&bytes).map_err(|error| {
            retained(
                "proposal_heads",
                subject.clone(),
                DESIGN_PASS_RELEASE,
                format!("the previous Design pass decision is unsupported: {error}"),
            )
        })?;
        let current = current_output_decision(previous, &candidates).map_err(|detail| {
            retained(
                "proposal_heads",
                subject.clone(),
                DESIGN_PASS_RELEASE,
                detail,
            )
        })?;
        let body = wire::encode(&current).map_err(|error| {
            retained(
                "proposal_heads",
                subject.clone(),
                DESIGN_PASS_RELEASE,
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
                DESIGN_PASS_RELEASE,
                "the decision head changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn current_output_decision(
    previous: PreviousDecision,
    candidates: &BTreeMap<String, DesignPassProposal>,
) -> Result<PreparationDecision, String> {
    let key = candidate_key(previous.outcome.candidate())?;
    let proposal = candidates
        .get(&key)
        .ok_or_else(|| "the decision has no exact Design pass proposal".to_owned())?;
    let outcome = match previous.outcome {
        PreviousOutcome::Selected {
            candidate,
            selected_outputs,
        } => {
            if proposal.recommendation != wire::rooms::DesignPassRecommendation::Work {
                return Err("a selected decision does not name a work proposal".to_owned());
            }
            let selected = selected_outputs
                .iter()
                .collect::<std::collections::BTreeSet<_>>();
            if selected.len() != selected_outputs.len()
                || selected_outputs
                    .iter()
                    .any(|output| !proposal.outputs.contains(output))
            {
                return Err(
                    "a selected decision does not name one unique subset of its proposal outputs"
                        .to_owned(),
                );
            }
            DecisionOutcome::Resolved {
                candidate,
                outputs: proposal
                    .outputs
                    .iter()
                    .cloned()
                    .map(|output| OutputDecision {
                        verdict: if selected_outputs.contains(&output) {
                            wire::rooms::OutputVerdict::Accepted
                        } else {
                            wire::rooms::OutputVerdict::Rejected
                        },
                        output,
                        comment: None,
                    })
                    .collect(),
                comment: None,
            }
        }
        PreviousOutcome::Returned { candidate, reason } => {
            DecisionOutcome::Returned { candidate, reason }
        }
        PreviousOutcome::Resolved {
            candidate,
            outputs,
            comment,
        } => {
            if proposal.recommendation != wire::rooms::DesignPassRecommendation::Work
                || outputs.len() != proposal.outputs.len()
                || outputs
                    .iter()
                    .zip(&proposal.outputs)
                    .any(|(decision, proposal)| decision.output != *proposal)
            {
                return Err(
                    "a resolved decision does not name every work proposal output in order"
                        .to_owned(),
                );
            }
            DecisionOutcome::Resolved {
                candidate,
                outputs,
                comment,
            }
        }
        PreviousOutcome::Collapse { candidate, reason } => {
            if proposal.recommendation != wire::rooms::DesignPassRecommendation::Collapse
                || proposal.justification != reason
            {
                return Err(
                    "a collapse decision does not match its crossed outputless proposal".to_owned(),
                );
            }
            DecisionOutcome::Accepted { candidate }
        }
    };
    Ok(PreparationDecision {
        decision: previous.decision,
        source: previous.source,
        outcome,
    })
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
) -> Result<BTreeMap<String, DesignPassProposal>, OpenError> {
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
        let candidate: DesignPassProposal = wire::decode(&body).map_err(|error| {
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
    candidates: &BTreeMap<String, DesignPassProposal>,
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
    candidates: &BTreeMap<String, DesignPassProposal>,
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
    candidates: &BTreeMap<String, DesignPassProposal>,
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
            wire::rooms::DecisionOutcome::Resolved {
                candidate,
                outputs: retained
                    .outputs
                    .iter()
                    .cloned()
                    .map(|output| wire::rooms::OutputDecision {
                        verdict: if authorization.selected_outputs.contains(&output) {
                            wire::rooms::OutputVerdict::Accepted
                        } else {
                            wire::rooms::OutputVerdict::Rejected
                        },
                        output,
                        comment: None,
                    })
                    .collect(),
                comment: None,
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

fn proposal_refusal(seq: i64, detail: &str) -> OpenError {
    retained(
        "fact_envelopes",
        seq.to_string(),
        DESIGN_PASS_RELEASE,
        detail.to_owned(),
    )
}

fn fact_refusal(seq: i64, detail: &str) -> OpenError {
    retained(
        "fact_envelopes",
        seq.to_string(),
        TWO_STEP_DECISION_RELEASE,
        detail.to_owned(),
    )
}
