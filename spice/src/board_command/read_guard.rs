use super::*;

mod guard;
pub(super) use guard::guard_allocation;
use guard::{
    current_task_heads, current_trust_heads, refuse_active_claim_conflict, witnesses_hold,
};
mod capture;
pub(super) use capture::guard_capture;

pub(super) fn phase(name: &str) -> wire::Phase {
    name.parse()
        .expect("dialect phase is a lower-case wire phase")
}

pub(super) fn task_flow(
    flow: Option<&str>,
    acceptance: &[String],
) -> Result<Vec<wire::Phase>, Outcome<CommandAnswer>> {
    if flow.is_none() && !substantive_acceptance(acceptance) {
        return phases(Some("plan,todo,review"));
    }
    phases(flow)
}

pub(super) fn validated_snapshot(
    bus: &Bus,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("board::heads-unreadable", &error.to_string()))?;
    validated_snapshot_from(&observed, principal, projects, allocation)
}

pub(super) fn validated_snapshot_from(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    validated_snapshot_from_with_occupant(observed, principal, projects, allocation, None)
}

pub(super) fn validated_snapshot_for_automatic(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    occupant: wire::CanonicalUuid,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    validated_snapshot_from_with_occupant(observed, principal, projects, allocation, Some(occupant))
}

fn validated_snapshot_from_with_occupant(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    prospective_occupant: Option<wire::CanonicalUuid>,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let cursor = observed.cursor().ok_or_else(|| {
        store_fault(
            "board::cursor-missing",
            "a running resident has no durable snapshot cursor",
        )
    })?;
    let residency = observed.residency().ok_or_else(|| {
        store_fault(
            "board::residency-missing",
            "a running resident has no current residency head",
        )
    })?;
    let mut residency = positioned(residency);
    let origin_threads = task_origin_threads(observed.facts().iter(), &residency.body)?;
    let creator_lineages = task_creator_lineages(observed.facts().iter())?;
    let (claim_contexts, git_contexts) = task_contexts(observed.facts());
    let mut tasks = Vec::with_capacity(observed.tasks().len());
    let mut entries = Vec::with_capacity(observed.tasks().len());
    for heads in observed.tasks() {
        let statement = positioned(heads.statement());
        let workflow = positioned(heads.workflow());
        let claim = heads.claim().map(positioned);
        let lease = heads.lease().map(positioned);
        let claim_context = current_claim_context(
            &statement.body.born,
            claim.as_ref().map(|head| &head.body),
            &claim_contexts,
        )?;
        let git_context = git_contexts.get(&statement.body.born).cloned();
        let handle = TaskHandle::for_project(&statement.body.project, &statement.body.born);
        let creator_lineage = creator_lineages
            .get(&statement.body.born)
            .copied()
            .flatten();
        let entry = Entry::new(
            statement.body.clone(),
            workflow.body.clone(),
            claim.as_ref().map(|head| head.body.clone()),
            lease.as_ref().map(|head| head.body.clone()),
            projects,
        )
        .map_err(|error| {
            store_fault(
                "board::head-invalid",
                &format!("task {handle} current heads are invalid: {error}"),
            )
        })?
        .with_creator_lineage(creator_lineage);
        let visibility = entry.project().visibility().clone();
        let origin_thread = origin_threads
            .get(&statement.body.born)
            .cloned()
            .ok_or_else(|| {
                protocol_fault(
                    "board::task-origin-missing",
                    &format!("task {handle} has no authoritative creation fact"),
                )
            })?;
        entries.push(entry);
        tasks.push(
            BoardTask::from_authoritative(
                visibility,
                origin_thread,
                creator_lineage,
                statement,
                workflow,
                claim,
                lease,
            )
            .with_context(claim_context, git_context),
        );
    }
    let recent_cell = board::recent_claim_cell(&entries, observed.facts().iter(), &principal);
    let review_history = board::graph::review_history(observed.facts().iter(), &entries)
        .map_err(|error| error.to_string());
    for lane in &mut residency.body.lanes {
        lane.capabilities.clear();
    }
    let actor_route = match prospective_occupant {
        Some(occupant) => actor_route_with_occupant(&principal, &residency.body, Some(occupant))?,
        None => actor_route(&principal, &residency.body)?,
    };
    Ok(ValidatedSnapshot {
        cursor,
        facts: observed.facts().clone(),
        answer: BoardAnswer {
            schema: journal::SCHEMA_VERSION,
            cursor,
            principal,
            project_policy: BoardProjectPolicy {
                public_stems: projects.public_stems().iter().cloned().collect(),
                public_min_depth: projects.public_min_depth(),
                public_max_depth: projects.public_max_depth(),
            },
            actor_route,
            allocation_band: allocation.band(),
            recent_cell,
            tasks,
            trust: positioned_trust(observed),
            residency,
        },
        entries,
        review_history,
    })
}

fn positioned_trust(observed: &bus::TaskSnapshot) -> Vec<Positioned<wire::TrustHead>> {
    observed.trust().iter().map(positioned).collect()
}

type ClaimContexts =
    std::collections::BTreeMap<Seq, (wire::TaskStamp, wire::intent::TaskClaimContext)>;

fn current_claim_context(
    task: &wire::TaskStamp,
    claim: Option<&wire::ClaimHead>,
    contexts: &ClaimContexts,
) -> Result<Option<wire::intent::TaskClaimContext>, Outcome<CommandAnswer>> {
    let Some(claim) = claim else {
        return Ok(None);
    };
    let (context_task, context) = contexts.get(&claim.claimed).ok_or_else(|| {
        protocol_fault(
            "board::claim-context-fact-missing",
            &format!("task {task} claim names absent fact {}", claim.claimed),
        )
    })?;
    if context_task != task {
        return Err(protocol_fault(
            "board::claim-context-fact-invalid",
            &format!(
                "task {task} claim position {} belongs to task {context_task}",
                claim.claimed
            ),
        ));
    }
    Ok(Some(context.clone()))
}

#[derive(Clone)]
pub(super) struct TaskCreationCapture {
    context: body::TaskCreationContext,
    residency: Option<Seq>,
    source: Option<(wire::CanonicalUuid, body::SourceBoundary)>,
}

impl TaskCreationCapture {
    pub(super) fn context(&self) -> body::TaskCreationContext {
        self.context.clone()
    }

    pub(super) const fn creator_lineage(&self) -> Option<wire::CanonicalUuid> {
        self.context.creator_lineage()
    }

    pub(super) fn guard(&self, write: &mut JournalWrite) {
        let Some(expected_residency) = self.residency else {
            return;
        };
        let (lane, expected_source) = self
            .source
            .clone()
            .expect("a lane creation capture carries a source boundary");
        write.guard(JournalGuard::new(move |read| {
            let residency = read.residency_head()?.ok_or_else(|| {
                journal::GuardError::Invalid(
                    "task creation context has no current residency head".to_owned(),
                )
            })?;
            if residency.position() != expected_residency {
                return Err(journal::GuardError::refused(
                    "task::creation-context-residency-moved",
                ));
            }
            if read.source_boundary(&lane)? != expected_source {
                return Err(journal::GuardError::refused(
                    "task::creation-context-source-moved",
                ));
            }
            Ok(())
        }));
    }
}

pub(super) fn task_creation_context_output(context: &body::TaskCreationContext) -> Box<str> {
    match context {
        body::TaskCreationContext::Unavailable { reason } => {
            format!("unavailable {}", reason.as_str()).into_boxed_str()
        }
        body::TaskCreationContext::Transcript { boundary, .. }
            if boundary.stream != body::TASK_CREATION_CONTEXT_STREAM =>
        {
            format!(
                "unavailable {}",
                body::TaskCreationUnavailable::LegacyStreamUnregistered.as_str()
            )
            .into_boxed_str()
        }
        body::TaskCreationContext::Transcript {
            session, boundary, ..
        } => shell_words::join([
            "spice".to_owned(),
            "session".to_owned(),
            "context".to_owned(),
            session.clone(),
            "--lane".to_owned(),
            boundary.lane.to_string(),
            "--vendor".to_owned(),
            boundary.vendor.to_string(),
            "--stream".to_owned(),
            boundary.stream.clone(),
            "--source".to_owned(),
            boundary.source.to_string(),
            "--member".to_owned(),
            boundary.next.member.to_string(),
            "--offset".to_owned(),
            boundary.next.offset.to_string(),
        ])
        .into_boxed_str(),
    }
}

pub(super) fn task_creation_capture(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
) -> Result<TaskCreationCapture, Outcome<CommandAnswer>> {
    let lane = match command.actor() {
        Actor::Operator => {
            return Ok(TaskCreationCapture {
                context: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::OperatorActor,
                ),
                residency: None,
                source: None,
            });
        }
        Actor::Spice => {
            return Ok(TaskCreationCapture {
                context: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
                residency: None,
                source: None,
            });
        }
        Actor::Lane(lane) => *lane,
    };
    let row = snapshot
        .answer
        .residency
        .body
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .ok_or_else(|| {
            protocol_fault(
                "task::creation-context-lane-missing",
                "the authenticated task actor has no current residency row",
            )
        })?;
    let session = row.session.as_ref().ok_or_else(|| {
        refusal(
            "task::creation-context-session-missing",
            Some(lane.to_string()),
            "re-seat the lane under current session authority before creating tasks",
        )
    })?;
    let current_occupant = row.occupant.ok_or_else(|| {
        protocol_fault(
            "task::creation-context-occupant-missing",
            "the authenticated task actor has no current occupant",
        )
    })?;
    if session.lane != lane
        || session.occupant != current_occupant
        || session.vendor != row.vendor
        || session.identity.is_empty()
    {
        return Err(protocol_fault(
            "task::creation-context-session-mismatch",
            "the authenticated lane, occupant, vendor, and causal session disagree",
        ));
    }
    let source = bus.source_boundary(&lane).map_err(|error| {
        store_fault(
            "task::creation-context-source-unreadable",
            &error.to_string(),
        )
    })?;
    let body::SourceBoundary::Proven { heads } = &source else {
        return Err(protocol_fault(
            "task::creation-context-source-incomplete",
            "a live lane task boundary is not proven",
        ));
    };
    let matching = heads
        .iter()
        .filter(|head| head.lane == lane && head.vendor == session.vendor)
        .collect::<Vec<_>>();
    let registered = matching
        .iter()
        .filter(|head| head.stream == body::TASK_CREATION_CONTEXT_STREAM)
        .copied()
        .collect::<Vec<_>>();
    if registered.is_empty() && !matching.is_empty() {
        return Err(refusal(
            "task::creation-context-stream-unregistered",
            Some(format!(
                "lane={lane} vendor={} matching_heads={}",
                session.vendor,
                matching.len()
            )),
            "account the selected session stream before creating tasks",
        ));
    }
    let [boundary] = registered.as_slice() else {
        return Err(refusal(
            "task::creation-context-source-ambiguous",
            Some(format!(
                "lane={lane} vendor={} registered_heads={}",
                session.vendor,
                registered.len()
            )),
            "account exactly one selected session stream before creating tasks",
        ));
    };
    Ok(TaskCreationCapture {
        context: body::TaskCreationContext::Transcript {
            session: session.identity.clone(),
            boundary: (**boundary).clone(),
            creator_lineage: Some(row.lineage_id),
        },
        residency: Some(snapshot.answer.residency.position),
        source: Some((lane, source)),
    })
}

pub(super) fn actor_route(
    principal: &wire::Actor,
    residency: &wire::ResidencyHead,
) -> Result<wire::intent::BoardActorRoute, Outcome<CommandAnswer>> {
    actor_route_with_occupant(principal, residency, None)
}

fn actor_route_with_occupant(
    principal: &wire::Actor,
    residency: &wire::ResidencyHead,
    prospective_occupant: Option<wire::CanonicalUuid>,
) -> Result<wire::intent::BoardActorRoute, Outcome<CommandAnswer>> {
    let actor = match principal {
        wire::Actor::Operator => "operator".to_owned(),
        wire::Actor::Spice => "spice".to_owned(),
        wire::Actor::Lane(lane) => {
            let row = residency
                .lanes
                .iter()
                .find(|row| row.lane == *lane)
                .ok_or_else(|| {
                    protocol_fault(
                        "board::actor-route-lane-missing",
                        "the board principal has no current residency row",
                    )
                })?;
            match &row.session {
                Some(session) => route_actor_identity(&session.identity).ok_or_else(|| {
                    protocol_fault(
                        "board::actor-route-session-invalid",
                        "the current vendor session identity cannot name an actor task route",
                    )
                })?,
                None => row
                    .occupant
                    .or(prospective_occupant)
                    .ok_or_else(|| {
                        protocol_fault(
                            "board::actor-route-occupant-missing",
                            "a legacy lane without session authority has no current occupant",
                        )
                    })?
                    .to_string(),
            }
        }
    };
    Ok(wire::intent::BoardActorRoute {
        project: format!("agent.{actor}.task"),
        origin_thread: actor.clone(),
        actor,
    })
}

fn route_actor_identity(identity: &str) -> Option<String> {
    if let Ok(identity) = identity.parse::<wire::CanonicalUuid>() {
        return Some(identity.to_string());
    }
    (!identity.is_empty()
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then(|| identity.to_owned())
}

fn task_origin_threads<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    residency: &wire::ResidencyHead,
) -> Result<std::collections::BTreeMap<wire::TaskStamp, String>, Outcome<CommandAnswer>> {
    let mut occupants = std::collections::BTreeMap::new();
    let mut sessions = std::collections::BTreeMap::new();
    let mut origins = std::collections::BTreeMap::new();
    for fact in facts {
        match fact.fact.kind() {
            wire::FactKind::LaneSessionDeclared => {
                let lane = required_fact_uuid(fact, "lane")?;
                let occupant = required_fact_uuid(fact, "occupant")?;
                let identity = required_fact_text(fact, "identity")?;
                sessions.insert((lane, occupant), identity.to_owned());
            }
            wire::FactKind::LaneStarted => {
                occupants.insert(
                    required_fact_uuid(fact, "lane")?,
                    required_fact_uuid(fact, "occupant")?,
                );
            }
            wire::FactKind::TaskAdded => {
                let task = required_fact_text(fact, "task")?
                    .parse::<wire::TaskStamp>()
                    .map_err(|error| {
                        protocol_fault(
                            "board::task-origin-fact-invalid",
                            &format!("task creation fact has an invalid birth stamp: {error}"),
                        )
                    })?;
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
                if origins.insert(task.clone(), actor).is_some() {
                    return Err(protocol_fault(
                        "board::task-origin-duplicate",
                        &format!("task {task} has more than one creation fact"),
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(origins)
}

fn task_creator_lineages<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
) -> Result<
    std::collections::BTreeMap<wire::TaskStamp, Option<wire::CanonicalUuid>>,
    Outcome<CommandAnswer>,
> {
    let mut creators = std::collections::BTreeMap::new();
    for envelope in facts {
        let Some(added) = envelope.fact.read::<body::TaskAdded>() else {
            continue;
        };
        let creator = match envelope.actor {
            Actor::Lane(_) => added.creation.creator_lineage(),
            Actor::Operator | Actor::Spice if added.creation.creator_lineage().is_none() => None,
            Actor::Operator | Actor::Spice => {
                return Err(protocol_fault(
                    "board::task-creator-lineage-invalid",
                    "a non-lane task creation names a creator lineage",
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

pub(super) fn claim_transcript_recovery(
    won: &body::ClaimWon,
    sessions: &std::collections::BTreeMap<
        (
            wire::CanonicalUuid,
            wire::CanonicalUuid,
            wire::Name,
            wire::CanonicalUuid,
        ),
        String,
    >,
) -> wire::intent::TaskTranscriptRecovery {
    let body::SourceBoundary::Proven { heads } = &won.source else {
        let body::SourceBoundary::Incomplete { reason } = &won.source else {
            unreachable!("source boundary has a closed wire vocabulary")
        };
        return wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: format!("source-incomplete: {reason}"),
        };
    };
    let mut registered = heads
        .iter()
        .filter(|head| head.stream == body::TASK_CREATION_CONTEXT_STREAM);
    let Some(boundary) = registered.next() else {
        return wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: "registered-session-stream-missing".to_owned(),
        };
    };
    if registered.next().is_some() {
        return wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: "registered-session-stream-ambiguous".to_owned(),
        };
    }
    if boundary.lane != won.lane {
        return wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: "registered-session-lane-mismatch".to_owned(),
        };
    }
    let key = (won.lane, won.occupant, boundary.vendor.clone(), won.epoch);
    match sessions.get(&key) {
        Some(session) if !session.is_empty() => wire::intent::TaskTranscriptRecovery::Transcript {
            session: session.clone(),
        },
        Some(_) => wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: "session-identity-empty".to_owned(),
        },
        None => wire::intent::TaskTranscriptRecovery::Unavailable {
            reason: "session-declaration-missing".to_owned(),
        },
    }
}

fn task_contexts(
    facts: &bus::TaskFacts,
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

fn required_fact_uuid(
    fact: &wire::Envelope<wire::AnyFact>,
    field: &str,
) -> Result<wire::CanonicalUuid, Outcome<CommandAnswer>> {
    required_fact_text(fact, field)?.parse().map_err(|error| {
        protocol_fault(
            "board::task-origin-fact-invalid",
            &format!(
                "{} fact field {field} is invalid: {error}",
                fact.fact.kind()
            ),
        )
    })
}

fn required_fact_text<'a>(
    fact: &'a wire::Envelope<wire::AnyFact>,
    field: &str,
) -> Result<&'a str, Outcome<CommandAnswer>> {
    fact.fact
        .body()
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            protocol_fault(
                "board::task-origin-fact-invalid",
                &format!("{} fact has no text field {field}", fact.fact.kind()),
            )
        })
}

pub(super) fn positioned<T: Clone>(head: &journal::JournalHead<T>) -> Positioned<T> {
    Positioned {
        position: head.position(),
        body: head.body().clone(),
    }
}

pub(super) fn task_repository(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    resident: &Repository,
) -> Result<Repository, Outcome<CommandAnswer>> {
    let Some((lane, occupant)) = command.expected().iter().find_map(|witness| match witness {
        Witness::Residency {
            lane,
            occupant,
            epoch,
        } if *epoch == command.epoch() => Some((*lane, *occupant)),
        _ => None,
    }) else {
        return Err(protocol_fault(
            "git::residency-witness",
            "task Git boundary has no current residency witness",
        ));
    };
    let Some(row) = snapshot
        .answer
        .residency
        .body
        .lanes
        .iter()
        .find(|row| row.lane == lane && row.occupant == Some(occupant))
    else {
        return Err(refusal(
            "git::lane-moved",
            Some(lane.to_string()),
            "retry through the lane and occupant that currently hold the claim",
        ));
    };
    let repository = Repository::discover(&row.worktree)
        .map_err(|error| environment_fault("git::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != resident.common_dir()
        || repository.worktree_root() != std::path::Path::new(&row.worktree)
    {
        return Err(environment_fault(
            "git::worktree-moved",
            "the claimed lane no longer names one worktree in the resident repository",
        ));
    }
    Ok(repository)
}

pub(super) fn publish_task_boundary(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    publish_task_boundary_inner(repository, snapshot, command, entry)
}

fn publish_task_boundary_inner(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let actor = command.actor().to_string();
    let at = current_instant()?;
    let active = board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "git::resolution-claim-refused",
            Some(error.to_string()),
            "renew or win the exact active task phase before retrying its Git boundary",
        )
    })?;
    let metadata = spice::task_git::Metadata {
        label: entry.handle().as_str(),
        title: &entry.statement().title,
        project: &entry.statement().project,
        phase: entry.workflow().phase.as_str(),
        actor: &actor,
    };
    let opening = entry.claim().and_then(|claim| {
        snapshot
            .facts
            .iter()
            .find(|envelope| envelope.seq == claim.claimed)
            .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
            .and_then(|won| won.git)
    });
    match spice::task_git::publish_claimed(&lane, metadata, &active, opening.as_ref()) {
        Ok(boundary) => Ok(Some(boundary)),
        Err(error) => Err(task_git_outcome(error)),
    }
}

pub(super) fn task_git_outcome(error: spice::task_git::Error) -> Outcome<CommandAnswer> {
    match error.class() {
        spice::task_git::ErrorClass::Refusal => refusal(
            error.reason(),
            Some(error.detail().to_owned()),
            error.remedy(),
        ),
        spice::task_git::ErrorClass::Fault => environment_fault(error.reason(), error.detail()),
    }
}

pub(super) fn read_board(
    bus: &Bus,
    read: &BoardRead,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let mut snapshot = match validated_snapshot(bus, principal, projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if let Some(family) = &read.family {
        if family.is_empty()
            || !family
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return refusal(
                "board::family-invalid",
                Some(family.clone()),
                "name one upper-case task family",
            );
        }
        snapshot.answer.tasks.retain(|task| {
            task.statement
                .body
                .handle
                .as_str()
                .split_once('-')
                .is_some_and(|(stem, _)| stem == family)
        });
    }
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Board(Box::new(snapshot.answer)),
    )
}

pub(super) fn read_graph(
    bus: &Bus,
    graph: &BoardGraph,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Some(at) = &graph.at {
        return refusal(
            "board::historical-barrier-refused",
            Some(at.clone()),
            "omit --at to read the current validated board barrier",
        );
    }
    let view = match graph.view.as_str() {
        "origin" => View::Origin,
        "dependencies" => View::Dependencies,
        "review" => View::Review,
        "workflow" => View::Workflow,
        "claims" => View::Claims,
        other => {
            return refusal(
                "board::graph-view-invalid",
                Some(other.to_owned()),
                "choose origin, dependencies, review, workflow, or claims",
            );
        }
    };
    let snapshot = match validated_snapshot(bus, principal, projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entries = match actor_entries(&snapshot.answer) {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    let reviews = if view == View::Review {
        match &snapshot.review_history {
            Ok(reviews) => reviews.as_slice(),
            Err(error) => {
                return store_fault("board::review-history-invalid", error);
            }
        }
    } else {
        &[]
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::graph::render(view, &entries, reviews)
                .text()
                .to_owned(),
        }),
    )
}

pub(super) fn read_handout(
    bus: &Bus,
    handout: &BoardHandout,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Some(at) = &handout.at {
        return refusal(
            "board::historical-barrier-refused",
            Some(at.clone()),
            "omit --at to read the current validated board barrier",
        );
    }
    if handout.output.trim().is_empty() {
        return refusal(
            "board::handout-output-invalid",
            Some(handout.output.clone()),
            "name an output path or -",
        );
    }
    let snapshot = match validated_snapshot(bus, principal, projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entries = match actor_entries(&snapshot.answer) {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    let reviews = match &snapshot.review_history {
        Ok(reviews) => reviews,
        Err(error) => return store_fault("board::review-history-invalid", error),
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::handout::compose(&entries, reviews),
        }),
    )
}

pub(super) fn read_ledger(
    bus: &Bus,
    ledger: &DocLedger,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, principal, projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(task) = snapshot
        .answer
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == ledger.task)
    else {
        return refusal(
            "doc::task-unknown",
            Some(ledger.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let statement = &task.statement.body;
    let mut source = format!("# {}\n", task.presentation_title());
    source.push_str(&format!(
        "\nFlow: {}\n",
        join_phases(&task.workflow.body.flow)
    ));
    let Some(mut document) = dialect::classify(&source).document().cloned() else {
        return store_fault(
            "doc::ledger-unrepresentable",
            "current task heads did not classify as a canonical task document",
        );
    };
    let root = document.root;
    let node = &mut document.nodes[root];
    node.acceptance = statement.acceptance.clone();
    node.priority = statement
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix(board::PRIORITY_TAG_PREFIX))
        .and_then(dialect::Priority::parse);
    node.tags = visible_tags(&statement.tags)
        .filter(|tag| {
            !tag.starts_with(board::PRIORITY_TAG_PREFIX) && !tag.starts_with("document-slug:")
        })
        .cloned()
        .collect();
    node.description = statement.description.clone().unwrap_or_default();
    if !task.visibility.is_hidden() {
        node.annotations = snapshot
            .facts
            .iter()
            .filter_map(|envelope| envelope.fact.read::<body::TaskNoted>())
            .filter(|noted| noted.task == statement.born)
            .map(|noted| note_annotation(&noted.note))
            .collect();
    }
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: dialect::emit(&document),
        }),
    )
}

fn note_annotation(note: &str) -> String {
    let longest = note
        .lines()
        .map(str::trim)
        .filter(|line| line.chars().all(|character| character == '~'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "~".repeat(longest.saturating_add(1).max(3));
    format!("{fence}\n{note}\n{fence}")
}

fn join_phases(phases: &[wire::Phase]) -> String {
    phases
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn request<B>(command: &wire::AdmittedCommand, body: B) -> Command<B> {
    Command {
        v: WireVersion::CURRENT,
        id: command.id(),
        epoch: command.epoch(),
        kind: command.kind(),
        expected: command.expected().to_vec(),
        body,
    }
}

pub(super) fn guard(write: &mut JournalWrite, expected: Vec<Witness>) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)
    }));
}

type AllocationHeads = Vec<(TaskHandle, Seq, Seq, Option<Seq>, Option<Seq>)>;

pub(super) struct AllocationObservation {
    pub(super) heads: AllocationHeads,
    pub(super) entries: Vec<Entry>,
    pub(super) winner: TaskHandle,
    pub(super) actor: Actor,
    pub(super) subscription: wire::LineageSubscription,
    pub(super) residency_position: Seq,
    pub(super) band: u64,
    pub(super) recent_cell: Option<wire::ClaimCell>,
}

pub(super) fn allocation_subscription<'a>(
    residency: &'a wire::ResidencyHead,
    actor: &Actor,
) -> Result<&'a wire::LineageSubscription, Outcome<CommandAnswer>> {
    let occupied =
        |lane: &&wire::LaneResidency| lane.disposition == "occupied" && lane.occupant.is_some();
    let seat = match actor {
        Actor::Lane(actor) => residency.lanes.iter().find(|seat| seat.lane == *actor),
        Actor::Operator => {
            let mut seats = residency.lanes.iter().filter(occupied);
            let first = seats.next();
            if seats.next().is_some() {
                return Err(refusal(
                    "claim::operator-lane-ambiguous",
                    None,
                    "use a lane capability when more than one occupied lane is seated",
                ));
            }
            first
        }
        Actor::Spice => None,
    }
    .filter(occupied)
    .ok_or_else(|| {
        refusal(
            "claim::lane-unavailable",
            Some(actor.to_string()),
            "seat one occupied lane before requesting task next",
        )
    })?;
    let mut subscriptions = residency
        .subscriptions
        .iter()
        .filter(|subscription| subscription.lineage_id == seat.lineage_id);
    let subscription = subscriptions.next().ok_or_else(|| {
        store_fault(
            "lineage::subscription-missing",
            "the allocating lane lineage has no durable task-allocation lifetime",
        )
    })?;
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lineage::subscription-ambiguous",
            "the allocating lane lineage has more than one durable task-allocation lifetime",
        ));
    }
    Ok(subscription)
}

pub(super) fn allocation_heads(board: &BoardAnswer) -> AllocationHeads {
    board
        .tasks
        .iter()
        .map(|task| {
            (
                task.statement.body.handle.clone(),
                task.statement.position,
                task.workflow.position,
                task.claim.as_ref().map(|head| head.position),
                task.lease.as_ref().map(|head| head.position),
            )
        })
        .collect()
}

pub(super) fn committed(
    bus: &Bus,
    write: JournalWrite,
    body: CommandAnswer,
) -> Outcome<CommandAnswer> {
    match bus.commit(write) {
        Ok(CommitOutcome::Committed(commit)) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(commit.first()),
                last: seq(commit.last()),
            }),
            body,
        }),
        Ok(CommitOutcome::Refused(refusal)) => {
            Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
                "board::witness-moved".to_owned(),
                Some("current durable heads".to_owned()),
                None,
                Some(refusal.reason().to_owned()),
            )))
        }
        Ok(CommitOutcome::Fault(fault)) => store_fault("board::commit-failed", fault.detail()),
        Err(error) => store_fault("board::bus-closed", &error.to_string()),
    }
}

pub(super) fn phases(flow: Option<&str>) -> Result<Vec<wire::Phase>, Outcome<CommandAnswer>> {
    let names = flow.map_or_else(
        || vec!["todo", "review"],
        |flow| {
            flow.split(',')
                .map(str::trim)
                .filter(|phase| !phase.is_empty())
                .collect()
        },
    );
    if names.is_empty() {
        return Err(refusal(
            "task::flow-empty",
            flow.map(str::to_owned),
            "name at least one lower-case workflow phase",
        ));
    }
    names
        .into_iter()
        .map(|phase| {
            phase.parse::<wire::Phase>().map_err(|error| {
                refusal(
                    "task::flow-invalid",
                    Some(phase.to_owned()),
                    &error.to_string(),
                )
            })
        })
        .collect()
}

pub(super) fn task_origin(task: &wire::TaskStamp) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    format!("task:{task}").parse().map_err(|error| {
        protocol_fault(
            "task::origin-invalid",
            &format!("validated birth stamp did not form a task origin: {error}"),
        )
    })
}

pub(super) fn normalize_task_origin(
    origin: wire::Origin,
) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    let Some(identity) = origin.as_str().strip_prefix("task:") else {
        return Ok(origin);
    };
    let born = match identity.parse::<wire::TaskStamp>() {
        Ok(born) => Ok(born),
        Err(stamp_error) => identity
            .parse::<TaskHandle>()
            .map_err(|handle_error| format!("{stamp_error}; {handle_error}"))
            .and_then(|handle| handle.born().map_err(|error| error.to_string())),
    };
    let born = born.map_err(|error| {
        refusal(
            "task::origin-invalid",
            Some(error.to_string()),
            "name a task origin carrying one canonical birth stamp",
        )
    })?;
    task_origin(&born)
}

/// Resolve the causal realm for one task-creation transaction.
///
/// Authored provenance wins because it is how an inline steering capture names
/// its exact `ack:` directive. Otherwise the actor's one active claim is the
/// parent work that caused the creation. Typed system routes may finally name
/// an intrinsic cause they have already proved, such as a gate report or
/// authored-document digest. Dependency edges never participate: scheduling
/// order and creation causality are independent facts.
pub(super) fn resolve_creation_origin(
    authored: Option<&wire::Origin>,
    actor: &Actor,
    snapshot: &ValidatedSnapshot,
    intrinsic: Option<wire::Origin>,
) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    if let Some(origin) = authored {
        return normalize_task_origin(origin.clone());
    }
    let mut held = snapshot.entries.iter().filter(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == *actor && claim.disposition == ClaimDisposition::Active
        })
    });
    if let Some(parent) = held.next() {
        if held.next().is_some() {
            return Err(protocol_fault(
                "task::origin-ambiguous",
                "one actor holds more than one active claim at task creation",
            ));
        }
        return task_origin(parent.born());
    }
    if let Some(origin) = intrinsic {
        return Ok(origin);
    }
    Err(refusal(
        "task::origin-required",
        Some(actor.to_string()),
        "supply --origin ack:<steering-key> or --origin task:<parent-born-stamp>, or create while holding one active claim",
    ))
}

pub(super) fn current_instant() -> Result<Instant, Outcome<CommandAnswer>> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("board::clock-invalid", &error.to_string()))
}

pub(super) fn instant_after(at: Instant, seconds: i64) -> Result<Instant, Outcome<CommandAnswer>> {
    let timestamp = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .map_err(|error| environment_fault("board::clock-range", &error.to_string()))?;
    Instant::new(timestamp)
        .map_err(|error| environment_fault("board::clock-invalid", &error.to_string()))
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("journal commits use non-zero positions")
}

fn snapshot_reply(cursor: wire::Seq, body: CommandAnswer) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body,
    })
}

pub(super) fn required(matches: &ArgMatches, name: &str) -> String {
    matches
        .get_one::<String>(name)
        .expect("generated syntax requires this argument")
        .clone()
}

pub(super) fn refusal(
    reason: &str,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(reason.to_owned(), None, None, observed).with_detail(detail),
    ))
}

pub(super) fn store_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Store,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

pub(super) fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

pub(super) fn protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
