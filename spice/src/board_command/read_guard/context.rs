use super::*;
pub(super) fn route_actor_identity(identity: &str) -> Option<String> {
    if let Ok(identity) = identity.parse::<wire::CanonicalUuid>() {
        return Some(identity.to_string());
    }
    (!identity.is_empty()
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then(|| identity.to_owned())
}

pub(super) fn task_origin_threads<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    residency: &wire::ResidencyHead,
    selected: Option<&std::collections::BTreeSet<wire::TaskStamp>>,
) -> Result<std::collections::BTreeMap<wire::TaskStamp, String>, Outcome<CommandAnswer>> {
    let mut occupants = std::collections::BTreeMap::new();
    let mut sessions = std::collections::BTreeMap::new();
    let mut origins = std::collections::BTreeMap::new();
    for fact in facts {
        if let Some(declared) = fact.fact.read::<body::LaneSessionDeclared>() {
            sessions.insert((declared.lane, declared.occupant), declared.identity);
            continue;
        }
        if let Some(started) = fact.fact.read::<body::LaneStarted>() {
            occupants.insert(started.lane, started.occupant);
            continue;
        }
        let Some(added) = fact.fact.read::<body::TaskAdded>() else {
            continue;
        };
        if selected.is_some_and(|tasks| !tasks.contains(&added.task)) {
            continue;
        }
        let actor = match fact.actor {
            wire::Actor::Operator => "operator".to_owned(),
            wire::Actor::Spice => "spice".to_owned(),
            wire::Actor::Lane(lane) => {
                let occupant = occupants.get(&lane).copied().or_else(|| {
                    residency
                        .lanes
                        .iter()
                        .find(|row| row.lane == lane && row.session.is_none())
                        .and_then(|row| row.occupant)
                });
                let occupant = occupant.ok_or_else(|| {
                    protocol_fault(
                        "board::task-origin-occupant-missing",
                        "a lane task has no authoritative creating occupant",
                    )
                })?;
                match sessions.get(&(lane, occupant)) {
                    Some(identity) => route_actor_identity(identity).ok_or_else(|| {
                        protocol_fault(
                            "board::task-origin-session-invalid",
                            "the creating vendor session identity cannot name an actor task route",
                        )
                    })?,
                    None => occupant.to_string(),
                }
            }
        };
        if origins.insert(added.task.clone(), actor).is_some() {
            return Err(protocol_fault(
                "board::task-origin-duplicate",
                &format!("task {} has more than one creation fact", added.task),
            ));
        }
    }
    Ok(origins)
}

pub(super) fn task_creator_teams<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    selected: Option<&std::collections::BTreeSet<wire::TaskStamp>>,
) -> Result<
    std::collections::BTreeMap<wire::TaskStamp, Option<wire::CanonicalUuid>>,
    Outcome<CommandAnswer>,
> {
    let mut creators = std::collections::BTreeMap::new();
    for envelope in facts {
        let Some(added) = envelope.fact.read::<body::TaskAdded>() else {
            continue;
        };
        if selected.is_some_and(|tasks| !tasks.contains(&added.task)) {
            continue;
        }
        let creator = match envelope.actor {
            Actor::Lane(_) => added.creation.creator_team(),
            Actor::Operator | Actor::Spice if added.creation.creator_team().is_none() => None,
            Actor::Operator | Actor::Spice => {
                return Err(protocol_fault(
                    "board::task-creator-team-invalid",
                    "a non-lane task creation names a creator team",
                ));
            }
        };
        if creators.insert(added.task.clone(), creator).is_some() {
            return Err(protocol_fault(
                "board::task-creator-duplicate",
                &format!("task {} has more than one creation fact", added.task),
            ));
        }
    }
    Ok(creators)
}

pub(super) fn task_contexts(
    facts: &bus::TaskFacts,
    selected: Option<&std::collections::BTreeSet<wire::TaskStamp>>,
) -> (
    ClaimContexts,
    std::collections::BTreeMap<wire::TaskStamp, wire::intent::TaskGitContext>,
) {
    let mut sessions = std::collections::BTreeMap::new();
    let mut claims = std::collections::BTreeMap::new();
    let mut git = std::collections::BTreeMap::new();
    for envelope in facts.iter() {
        if let Some(declaration) = envelope.fact.read::<body::LaneSessionDeclared>() {
            sessions.insert(
                (
                    declaration.lane,
                    declaration.occupant,
                    declaration.vendor,
                    declaration.epoch,
                ),
                declaration.identity,
            );
            continue;
        }
        if let Some(won) = envelope.fact.read::<body::ClaimWon>() {
            if selected.is_some_and(|tasks| !tasks.contains(&won.task)) {
                continue;
            }
            let task = won.task.clone();
            if let Some(boundary) = won.git.clone() {
                git.insert(
                    task.clone(),
                    wire::intent::TaskGitContext {
                        fact: envelope.seq,
                        kind: wire::intent::TaskGitContextKind::Claim,
                        boundary,
                    },
                );
            }
            let recovery = claim_transcript_recovery(&won, &sessions);
            claims.insert(
                envelope.seq,
                (
                    task,
                    wire::intent::TaskClaimContext {
                        claimed: envelope.seq,
                        lane: won.lane,
                        occupant: won.occupant,
                        epoch: won.epoch,
                        source: won.source,
                        recovery,
                    },
                ),
            );
            continue;
        }
        if let Some(done) = envelope.fact.read::<body::TaskDone>()
            && selected.is_none_or(|tasks| tasks.contains(&done.task))
            && let Some(boundary) = done.git
        {
            git.insert(
                done.task,
                wire::intent::TaskGitContext {
                    fact: envelope.seq,
                    kind: wire::intent::TaskGitContextKind::Completion,
                    boundary,
                },
            );
            continue;
        }
        if let Some(reviewed) = envelope.fact.read::<body::TaskReviewed>()
            && selected.is_none_or(|tasks| tasks.contains(&reviewed.task))
            && let Some(boundary) = reviewed.git
        {
            git.insert(
                reviewed.task,
                wire::intent::TaskGitContext {
                    fact: envelope.seq,
                    kind: wire::intent::TaskGitContextKind::Review,
                    boundary,
                },
            );
        }
    }
    (claims, git)
}
