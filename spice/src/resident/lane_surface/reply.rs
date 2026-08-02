//! Lane answers, recall provenance, and typed reply construction.

use super::*;

pub(super) fn command_result<B: serde::Serialize>(
    write: &mut JournalWrite,
    admitted: &wire::AdmittedCommand,
    body: B,
    epoch: CanonicalUuid,
    answer: LaneChangeAnswer,
) -> CommandResult<()> {
    write
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: admitted.id(),
                epoch: admitted.epoch(),
                kind: admitted.kind(),
                expected: admitted.expected().to_vec(),
                body,
            },
            epoch,
            answer,
        )
        .map_err(|error| store_fault("lane::receipt-invalid", &error.to_string()))
}

pub(super) fn answer(head: &wire::ResidencyHead, selected: Option<CanonicalUuid>) -> LaneAnswer {
    let lanes = head
        .lanes
        .iter()
        .filter(|row| selected.is_none_or(|lane| row.lane == lane))
        .cloned()
        .map(|mut row| {
            row.capabilities.clear();
            row
        })
        .collect();
    LaneAnswer {
        epoch: head.epoch,
        state: head.state,
        lanes,
    }
}

pub(super) fn relevant(fact: &wire::Envelope<wire::AnyFact>, lane: CanonicalUuid) -> bool {
    fact.actor == Actor::Lane(lane) || fact_lane(fact) == Some(lane)
}

pub(super) fn fact_lane(fact: &wire::Envelope<wire::AnyFact>) -> Option<CanonicalUuid> {
    fact.fact
        .body()
        .get("lane")
        .and_then(serde_json::Value::as_str)
        .and_then(|lane| lane.parse().ok())
        .or(match fact.actor {
            Actor::Lane(lane) => Some(lane),
            Actor::Operator | Actor::Spice => None,
        })
}

pub(super) fn recall_provenance(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<Option<RecallProvenance>> {
    let provenance = match fact.fact.kind() {
        wire::FactKind::LaneSessionDeclared => {
            let body: wire::body::LaneSessionDeclared = recall_body(fact)?;
            Some(RecallProvenance::Session {
                occupant: body.occupant,
                worktree: body.worktree,
                vendor: body.vendor,
                epoch: body.epoch,
                stream: body.stream,
                identity: body.identity,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::LaneCompacted => {
            let body: wire::body::LaneCompacted = recall_body(fact)?;
            Some(RecallProvenance::Compaction {
                occupant: body.occupant,
                stream: body.stream,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::SteerSent => {
            let body: wire::body::SteerSent = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                None,
                None,
                None,
            ))
        }
        wire::FactKind::SteerResent => {
            let body: wire::body::SteerResent = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                Some(body.ancestor),
                None,
                None,
            ))
        }
        wire::FactKind::SteerAcked => {
            let body: wire::body::SteerAcked = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Acked,
                None,
                Some(body.epoch),
                Some(body.locus),
            ))
        }
        wire::FactKind::SteerDeclined => {
            let body: wire::body::SteerDeclined = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Declined,
                None,
                Some(body.epoch),
                Some(body.locus),
            ))
        }
        wire::FactKind::SteerCanceled => {
            let body: wire::body::SteerCanceled = recall_body(fact)?;
            Some(steering_provenance(
                None,
                body.key,
                wire::Disposition::Canceled,
                None,
                None,
                None,
            ))
        }
        wire::FactKind::LaneRenewed => {
            let body: wire::body::LaneRenewed = recall_body(fact)?;
            Some(RecallProvenance::Renewal {
                occupant: body.occupant,
                ancestor: body.ancestor,
                epoch: body.epoch,
            })
        }
        _ => None,
    };
    Ok(provenance)
}

pub(super) fn steering_provenance(
    directive: Option<CanonicalUuid>,
    key: String,
    disposition: wire::Disposition,
    ancestor: Option<String>,
    epoch: Option<CanonicalUuid>,
    locus: Option<wire::body::TranscriptLocus>,
) -> RecallProvenance {
    RecallProvenance::Steering {
        directive,
        key,
        disposition,
        ancestor,
        epoch,
        locus,
    }
}

pub(super) fn recall_body<T: serde::de::DeserializeOwned>(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<T> {
    let bytes = wire::encode(fact.fact.body())
        .map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))?;
    wire::decode(&bytes).map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))
}

pub(super) fn committed_lane_reply(
    committed: bus::JournalCommit,
    answer: LaneChangeAnswer,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneChange(answer),
    })
}

pub(super) fn authority_refusal(actor: &Actor, expected: &str) -> Outcome<CommandAnswer> {
    lane_refusal(
        "command::authority",
        Some(expected.to_owned()),
        Some(actor.to_string()),
        "reconnect with a capability admitted for this command family",
    )
}

pub(super) fn missing_lane(lane: CanonicalUuid) -> Outcome<CommandAnswer> {
    lane_refusal(
        "lane::missing",
        Some("one current lane".to_owned()),
        Some(lane.to_string()),
        "read lane and retry with a current lane identity",
    )
}

pub(super) fn lane_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("lane".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

pub(super) fn typed_environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
