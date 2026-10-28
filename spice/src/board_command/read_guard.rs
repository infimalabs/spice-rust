use super::*;

mod guard;
pub(super) use guard::guard_allocation;
use guard::{refuse_active_claim_conflict, witnesses_hold};
mod capture;
mod context;
pub(super) use capture::guard_capture;
mod task_detail;
pub(super) use task_detail::{read_task_detail, recorded_note};
mod task_relations;
pub(super) use task_relations::read_task_relations;
mod task_timeline;
pub(super) use task_timeline::read_task_timeline;
pub(super) mod next_work;
mod task_selection;
mod working;
use context::{
    route_actor_identity, selected_task_contexts, selected_task_creator_teams,
    selected_task_origin_threads, task_contexts, task_creator_teams, task_origin_threads,
};
mod projection;
use projection::project_task_rows;
#[cfg(test)]
pub(in crate::board_command) use task_selection::history_cursor_order;
use task_selection::{TaskSelection, retain_family, selected_tasks};
pub(in crate::board_command) use task_selection::{in_family, validate_family};
pub(super) use working::{
    command_task_snapshot, task_ancestry_selection, validated_snapshot_for_automatic,
    validated_working_snapshot, validated_working_snapshot_for_tasks,
};

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
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("board::heads-unreadable", &error.to_string()))?;
    validated_snapshot_from(&observed, principal)
}

/// The principal role heads an author-naming read resolves fact actors
/// through.
pub(super) fn principal_roles(
    bus: &Bus,
) -> Result<wire::principal::PrincipalRoles, Outcome<CommandAnswer>> {
    bus.read(|read| read.principal_roles())
        .map_err(|error| store_fault("board::principal-roles-unreadable", &error.to_string()))
}

pub(super) fn validated_snapshot_from(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    validated_snapshot_from_with_occupant(observed, principal, None, None, true)
}

fn validated_snapshot_from_with_occupant(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    prospective_occupant: Option<wire::CanonicalUuid>,
    selected: Option<&std::collections::BTreeSet<wire::TaskStamp>>,
    retain_facts: bool,
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
    let appliance = spice::version::running()
        .map_err(Outcome::Fault)?
        .product
        .clone();
    let (entries, mut tasks) = project_task_rows(observed, &residency.body, selected)?;
    for task in &mut tasks {
        withhold_taste_findings(&principal, &mut task.statement.body.taste);
    }
    for lane in &mut residency.body.lanes {
        lane.capabilities.clear();
    }
    let actor_route = match prospective_occupant {
        Some(occupant) => actor_route_with_occupant(&principal, &residency.body, Some(occupant))?,
        None => actor_route(&principal, &residency.body)?,
    };
    let task_barrier = task_projection_barrier(
        observed
            .tasks()
            .iter()
            .flat_map(|heads| [heads.statement().position(), heads.workflow().position()])
            .max(),
    );
    Ok(ValidatedSnapshot {
        cursor,
        task_barrier,
        facts: if retain_facts {
            observed.facts().clone()
        } else {
            Default::default()
        },
        answer: BoardAnswer {
            schema: journal::SCHEMA_VERSION,
            cursor,
            oops_generation: observed.oops_generation().map(positioned),
            principal,
            actor_route,
            next: None,
            tasks,
            offer: Vec::new(),
            blocked: Vec::new(),
            workflow_waiting: Vec::new(),
            design_cycles: Default::default(),
            origins: Default::default(),
            acknowledgments: Default::default(),
            closures: Default::default(),
            steps: Default::default(),
            history: None,
            trust: positioned_trust(observed),
            appliance,
            addresses: observed.facts().seat_addresses(&residency.body.lanes),
            residency,
        },
        entries,
    })
}

/// Everything one task row needs beyond its own authoritative heads.
///
/// The maps are built once per snapshot, so projecting a row is a lookup
/// rather than another pass over the fact log.
struct TaskProjection<'a> {
    origin_threads: &'a std::collections::BTreeMap<wire::TaskStamp, String>,
    creator_teams: &'a std::collections::BTreeMap<wire::TaskStamp, Option<wire::CanonicalUuid>>,
    claim_contexts: &'a ClaimContexts,
    git_contexts: &'a std::collections::BTreeMap<wire::TaskStamp, wire::intent::TaskGitContext>,
    facts: &'a bus::TaskFacts,
}

impl TaskProjection<'_> {
    fn project(
        &self,
        heads: &journal::TaskHeads,
    ) -> Result<(Entry, BoardTask), Outcome<CommandAnswer>> {
        let statement = positioned(heads.statement());
        let workflow = positioned(heads.workflow());
        let claim = heads.claim().map(positioned);
        let lease = heads.lease().map(positioned);
        let explicit_release = claim.as_ref().is_some_and(|head| {
            self.facts
                .at(head.position)
                .is_some_and(|envelope| envelope.fact.kind() == wire::FactKind::ClaimFreed)
        });
        let parked_commits = claim.as_ref().and_then(|head| {
            self.facts
                .at(head.position)
                .and_then(|envelope| envelope.fact.read::<body::ClaimFreed>())
                .and_then(|freed| freed.parked)
        });
        let phase_handoff = phase_handoff(self.facts, heads)?;
        let claim_context = current_claim_context(
            &statement.body.born,
            claim.as_ref().map(|head| &head.body),
            self.claim_contexts,
        )?;
        let git_context = self.git_contexts.get(&statement.body.born).cloned();
        let handle = TaskHandle::for_filing(
            &statement.body.project,
            &statement.body.born,
            statement.body.filing,
        );
        let creator_team = self
            .creator_teams
            .get(&statement.body.born)
            .copied()
            .flatten();
        let grouped = self.facts.grouped(&statement.body.born);
        let design = self.facts.design(&statement.body);
        let coverage = design_coverage(
            statement.body.intent,
            &workflow.body,
            &statement.body.acceptance,
            self.facts.design_items(&statement.body.born),
        );
        let statement_born = statement.body.born.clone();
        let preparation_decision = self.facts.preparation_decision(&statement_born).cloned();
        let entry = Entry::new(
            statement.body.clone(),
            workflow.body.clone(),
            claim.as_ref().map(|head| head.body.clone()),
            lease.as_ref().map(|head| head.body.clone()),
        )
        .map_err(|error| {
            store_fault(
                "board::head-invalid",
                &format!("task {handle} current heads are invalid: {error}"),
            )
        })?
        .with_creator_team(creator_team)
        .with_explicit_release(explicit_release)
        .with_phase_handoff(phase_handoff)
        .with_grouped(grouped)
        .with_design(design)
        .with_preparation_decision(preparation_decision.clone())
        .with_queued_at(queued_at(self.facts, &statement.body));
        let origin_thread = self
            .origin_threads
            .get(&statement.body.born)
            .cloned()
            .ok_or_else(|| {
                protocol_fault(
                    "board::task-origin-missing",
                    &format!("task {handle} has no authoritative creation fact"),
                )
            })?;
        let task = BoardTask::from_authoritative(
            origin_thread,
            creator_team,
            statement,
            workflow,
            claim,
            lease,
        )
        .with_explicit_release(explicit_release)
        .with_grouped(grouped)
        .with_design_cycle(design)
        .with_design_coverage(coverage)
        .with_preparation_decision(preparation_decision)
        .with_collapse_reason(
            self.facts
                .collapse_reason(&statement_born)
                .map(str::to_owned),
        )
        .with_context(claim_context, git_context, parked_commits);
        Ok((entry, task))
    }
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

    pub(super) const fn creator_team(&self) -> Option<wire::CanonicalUuid> {
        self.context.creator_team()
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
        Actor::Operator | Actor::Human(_) => {
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
    let body::SourceBoundary::Proven { heads } = &source;
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
            creator_team: Some(row.team),
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
        wire::Actor::Operator | wire::Actor::Spice | wire::Actor::Human(_) => principal.to_string(),
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
        origin_thread: actor.clone(),
        actor,
    })
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
    let body::SourceBoundary::Proven { heads } = &won.source;
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
            occupant: Some(occupant),
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
    repository: Option<Host<'_>>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, &resident)?;
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
            .at(claim.claimed)
            .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
            .and_then(|won| won.git)
    });
    match spice::task_git::publish_claimed(
        &lane,
        resident.ambient,
        metadata,
        &active,
        opening.as_ref(),
    ) {
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
    command: &wire::AdmittedCommand,
    allocation: &board::NextConfig,
    profiles: next_work::OccupantProfiles<'_>,
) -> Outcome<CommandAnswer> {
    let principal = command.actor().clone();
    let observed = match bus.task_snapshot() {
        Ok(observed) => observed,
        Err(error) => return store_fault("board::heads-unreadable", &error.to_string()),
    };
    if matches!(read, BoardRead::Next) {
        let next = next_work::read_next_work(bus, &observed, principal, allocation, profiles);
        return match next {
            Ok(answer) => snapshot_reply(answer.cursor, CommandAnswer::Board(Box::new(answer))),
            Err(outcome) => outcome,
        };
    }
    let TaskSelection {
        tasks: selected,
        evidence,
        history,
    } = match selected_tasks(&observed, read, &principal) {
        Ok(selection) => selection,
        Err(outcome) => return outcome,
    };
    let mut snapshot = match validated_snapshot_from_with_occupant(
        &observed,
        principal,
        None,
        selected.as_ref(),
        false,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if matches!(read, BoardRead::Open | BoardRead::All { .. }) {
        let resolved = board::resolved_tasks(&snapshot.entries);
        let environment = snapshot.answer.environment();
        let waits = board::work_waits_resolved(&snapshot.entries, &resolved, &environment);
        for (task, wait) in waits {
            match wait {
                board::WorkWait::Blocked => snapshot.answer.blocked.push(task.clone()),
                board::WorkWait::Workflow => {
                    snapshot.answer.workflow_waiting.push(task.clone());
                }
                board::WorkWait::Selection => {}
            }
        }
        snapshot.answer.design_cycles = board::design_cycles(&snapshot.entries, &resolved);
    } else {
        snapshot.answer.design_cycles = match next_work::scoped_design_cycles(&observed, &snapshot)
        {
            Ok(cycles) => cycles,
            Err(outcome) => return outcome,
        };
    }
    if matches!(read, BoardRead::Open) {
        snapshot.answer.offer = board_offer(&snapshot.entries, &snapshot.answer);
        snapshot.answer.origins = steering_origins(&observed);
        snapshot
            .answer
            .tasks
            .retain(|task| !evidence.contains(&task.statement.body.born));
    }
    if let BoardRead::All {
        family: Some(family),
    } = read
        && let Err(outcome) = retain_family(&mut snapshot.answer.tasks, family)
    {
        return outcome;
    }
    if history.is_some() {
        snapshot.answer.tasks.sort_by(|left, right| {
            right
                .workflow
                .position
                .cmp(&left.workflow.position)
                .then_with(|| left.statement.body.born.cmp(&right.statement.body.born))
        });
    }
    if let BoardRead::Tasks {
        tasks,
        history: true,
        ..
    } = read
    {
        // Only named rows the snapshot admitted, so a hidden task's notes stay hidden.
        let named = snapshot
            .answer
            .tasks
            .iter()
            .map(|task| &task.statement.body.born)
            .filter(|born| tasks.contains(born))
            .cloned()
            .collect();
        let roles = match principal_roles(bus) {
            Ok(roles) => roles,
            Err(outcome) => return outcome,
        };
        let record = task_detail::task_record(observed.facts(), &named, &roles);
        snapshot.answer.acknowledgments = record.acknowledgments;
        snapshot.answer.closures = record.closures;
        snapshot.answer.steps = record.steps;
    }
    snapshot.answer.history = history;
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Board(Box::new(snapshot.answer)),
    )
}

/// The order the allocator would offer this board's work to a draining lane.
///
/// The app asks no particular lane, so the reader is a neutral seat: every
/// active claim is a peer's and no subscription narrows the creator team.
/// Each step is `task next`'s own choice.
pub(super) fn board_offer(entries: &[Entry], answer: &BoardAnswer) -> Vec<wire::TaskStamp> {
    let subscription = wire::TeamSubscription {
        team: wire::CanonicalUuid::new(uuid::Uuid::nil()),
        formed_at: None,
        policy: wire::intent::TeamLife::Drain,
        fast_mode: false,
    };
    board::offer_order(
        entries,
        &wire::Actor::Spice,
        &subscription,
        &answer.environment(),
    )
    .into_iter()
    .map(|selection| selection.entry().born().clone())
    .collect()
}

pub(super) fn read_graph(
    bus: &Bus,
    graph: &BoardGraph,
    principal: wire::Actor,
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
    let snapshot = match if graph.all {
        validated_snapshot(bus, principal)
    } else {
        validated_working_snapshot(bus, principal, &[])
    } {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entries = match actor_entries(&snapshot.answer) {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    let reviews = if view == View::Review {
        match review_history(bus, &snapshot) {
            Ok(reviews) => reviews,
            Err(outcome) => return outcome,
        }
    } else {
        Vec::new()
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: if graph.all {
                board::graph::render(view, &entries, &reviews)
            } else {
                board::graph::render_live(view, &entries, &reviews)
            }
            .text()
            .to_owned(),
        }),
    )
}

pub(super) fn read_handout(
    bus: &Bus,
    handout: &BoardHandout,
    principal: wire::Actor,
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
    let snapshot = match validated_snapshot(bus, principal) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entries = match actor_entries(&snapshot.answer) {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    let reviews = match review_history(bus, &snapshot) {
        Ok(reviews) => reviews,
        Err(outcome) => return outcome,
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::handout::compose(&entries, &reviews),
        }),
    )
}

/// Join every completed review the snapshot's visible tasks carry to the
/// phase author it followed, naming each author and reviewer as the
/// principal roles resolve it. This scans the whole fact log, so only the
/// reads that render reviews pay for it.
fn review_history(
    bus: &Bus,
    snapshot: &ValidatedSnapshot,
) -> Result<Vec<board::graph::ReviewEdge>, Outcome<CommandAnswer>> {
    let roles = principal_roles(bus)?;
    let reviews = board::graph::review_history(snapshot.facts.iter(), &snapshot.entries)
        .map_err(|error| store_fault("board::review-history-invalid", &error.to_string()))?;
    Ok(reviews
        .into_iter()
        .map(|review| review.resolved(&roles))
        .collect())
}

pub(super) fn read_ledger(
    bus: &Bus,
    ledger: &DocLedger,
    principal: wire::Actor,
) -> Outcome<CommandAnswer> {
    let roles = match principal_roles(bus) {
        Ok(roles) => roles,
        Err(outcome) => return outcome,
    };
    let snapshot = match validated_snapshot(bus, principal) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Ok(task) = task_heads(&snapshot.answer, &ledger.task) else {
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
    node.priority = statement.priority.map(board::priority_to_document);
    node.description = statement.description.clone().unwrap_or_default();
    let publications = match super::room::room_publications(bus, &statement.born) {
        Ok(publications) => publications,
        Err(outcome) => return outcome,
    };
    let mut annotations = publications
        .iter()
        .filter_map(|envelope| {
            let (_, note) = recorded_note(envelope)?;
            Some((envelope.seq, note_annotation(&note)))
        })
        .collect::<Vec<_>>();
    annotations.extend(ledger_step_annotations(
        &snapshot.facts,
        &snapshot.answer,
        &statement.born,
        &roles,
    ));
    annotations.sort_by_key(|(position, _)| *position);
    node.annotations = annotations.into_iter().map(|(_, text)| text).collect();
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: dialect::emit(&document),
        }),
    )
}

/// Each workflow step on `task` as the fenced annotation a ledger exports,
/// keyed by journal position; applying a ledger recognizes exactly these.
pub(super) fn ledger_step_annotations(
    facts: &bus::TaskFacts,
    board: &BoardAnswer,
    task: &wire::TaskStamp,
    roles: &wire::principal::PrincipalRoles,
) -> Vec<(wire::Seq, String)> {
    let named = std::collections::BTreeSet::from([task.clone()]);
    let mut record = task_detail::task_record(facts, &named, roles);
    record
        .steps
        .remove(task)
        .unwrap_or_default()
        .iter()
        .map(|step| {
            let text = step_annotation(step, |stamp| stamp_name(board, stamp));
            (step.position, note_annotation(&text))
        })
        .collect()
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

/// Refuse a multi-task mutation if any task statement or workflow changed
/// after its closure set was classified.
pub(super) fn guard_task_projection(write: &mut JournalWrite, expected: Digest) {
    write.guard(JournalGuard::new(move |read| {
        if task_projection_barrier(read.task_projection_cursor()?) == expected {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: task projection changed",
            ))
        }
    }));
}

pub(super) struct AllocationObservation {
    pub(super) cursor: Seq,
    pub(super) actor: Actor,
    pub(super) residency_position: Seq,
}

/// The task-allocation lifetime the allocator honours for one actor's seat.
///
/// This is the seat's current team policy and nothing else: a lane seated
/// alone follows the life its own team was seeded with, and a lane placed on
/// another team follows that team's policy.
pub(super) fn allocation_subscription(
    residency: &wire::ResidencyHead,
    actor: &Actor,
) -> Result<wire::TeamSubscription, Outcome<CommandAnswer>> {
    let occupied =
        |lane: &&wire::LaneResidency| lane.disposition == "occupied" && lane.occupant.is_some();
    let seat = match actor {
        Actor::Lane(actor) => residency.lanes.iter().find(|seat| seat.lane == *actor),
        principal if wire::principal::ActingPrincipal::of(principal).is_some() => {
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
        _ => None,
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
        .filter(|subscription| subscription.team == seat.team);
    let subscription = subscriptions.next().ok_or_else(|| {
        store_fault(
            "team::subscription-missing",
            "the allocating lane lineage has no durable task-allocation lifetime",
        )
    })?;
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "team::subscription-ambiguous",
            "the allocating lane lineage has more than one durable task-allocation lifetime",
        ));
    }
    Ok(subscription.clone())
}

pub(super) enum PreparedTaskGit<'a> {
    Release(spice::task_git::ParkedRelease<'a>),
    Claim(spice::task_git::ClaimPreparation<'a>),
}

impl PreparedTaskGit<'_> {
    fn rollback(self) -> Result<(), spice::task_git::Error> {
        match self {
            Self::Release(prepared) => prepared.rollback(),
            Self::Claim(prepared) => prepared.rollback(),
        }
    }
}

/// Undo a prepared parked release after its board write could not commit.
pub(super) fn rollback_release(
    parked: Option<spice::task_git::ParkedRelease>,
    outcome: Outcome<CommandAnswer>,
) -> Outcome<CommandAnswer> {
    match parked {
        Some(parked) => match parked.rollback() {
            Ok(()) => outcome,
            Err(error) => task_git_outcome(error),
        },
        None => outcome,
    }
}

pub(super) fn committed_with_task_git(
    bus: &Bus,
    write: JournalWrite,
    body: CommandAnswer,
    prepared: PreparedTaskGit<'_>,
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
            if let Err(error) = prepared.rollback() {
                return task_git_outcome(error);
            }
            guard_refusal(refusal)
        }
        Ok(CommitOutcome::Fault(fault)) => {
            if let Err(error) = prepared.rollback() {
                return task_git_outcome(error);
            }
            store_fault("board::commit-failed", fault.detail())
        }
        Err(error) => {
            if let Err(error) = prepared.rollback() {
                return task_git_outcome(error);
            }
            store_fault("board::bus-closed", &error.to_string())
        }
    }
}

pub(super) fn committed(
    bus: &Bus,
    write: JournalWrite,
    body: CommandAnswer,
) -> Outcome<CommandAnswer> {
    committed_with_guard_refusal(bus, write, body, None)
}

/// Commit one answer while allowing a command-specific atomic guard refusal
/// to remain its public reason rather than the generic stale-witness reason.
pub(super) fn committed_preserving_guard_refusal(
    bus: &Bus,
    write: JournalWrite,
    body: CommandAnswer,
    reason: &'static str,
) -> Outcome<CommandAnswer> {
    committed_with_guard_refusal(bus, write, body, Some(reason))
}

fn committed_with_guard_refusal(
    bus: &Bus,
    write: JournalWrite,
    body: CommandAnswer,
    preserved: Option<&str>,
) -> Outcome<CommandAnswer> {
    match bus.commit(write) {
        Ok(CommitOutcome::Committed(commit)) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(commit.first()),
                last: seq(commit.last()),
            }),
            body,
        }),
        Ok(CommitOutcome::Refused(refusal)) if preserved == Some(refusal.reason()) => {
            Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
                refusal.reason().to_owned(),
                None,
                None,
                None,
            )))
        }
        Ok(CommitOutcome::Refused(refusal)) => guard_refusal(refusal),
        Ok(CommitOutcome::Fault(fault)) => store_fault("board::commit-failed", fault.detail()),
        Err(error) => store_fault("board::bus-closed", &error.to_string()),
    }
}

/// A guarded commit refused by a durable-head comparison lost a race, so the
/// caller may read again and retry. Room authority instead judged the request
/// itself unsound against current authority; that keeps its own reason and
/// names the offending subject, because retrying it unchanged cannot succeed.
fn guard_refusal(refusal: journal::GuardRefusal) -> Outcome<CommandAnswer> {
    if refusal.reason() == journal::ROOM_AUTHORITY_REFUSED {
        return Outcome::Refusal(Refusal::bare(refusal.into_diagnostic().0));
    }
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        "board::witness-moved".to_owned(),
        Some("current durable heads".to_owned()),
        None,
        Some(refusal.reason().to_owned()),
    )))
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

/// The typed cause of one task-creation transaction and the envelope origin
/// that cause spells for every fact the transaction writes.
#[derive(Debug, Clone)]
pub(super) struct CreationCause {
    pub(super) provenance: wire::TaskProvenance,
    pub(super) origin: wire::Origin,
}

impl CreationCause {
    fn of(provenance: wire::TaskProvenance) -> Result<Self, Outcome<CommandAnswer>> {
        let origin = provenance.origin().ok_or_else(|| {
            protocol_fault(
                "task::origin-parentless",
                "task creation resolved a parentless cause, which no creation path mints",
            )
        })?;
        Ok(Self { provenance, origin })
    }
}

/// Admit an authored creation origin: a steering directive or a parent task.
pub(super) fn authored_provenance(
    origin: &wire::Origin,
) -> Result<wire::TaskProvenance, Outcome<CommandAnswer>> {
    wire::TaskProvenance::authored(origin.as_str()).map_err(|error| {
        let reason = if error.canonical().is_some() {
            "task::origin-noncanonical"
        } else {
            "task::origin-invalid"
        };
        refusal(reason, Some(origin.to_string()), &error.to_string())
    })
}

/// Resolve the causal realm for one task-creation transaction.
///
/// Authored provenance wins because it is how an inline steering capture names
/// its exact `ack:` directive. Otherwise the actor's one active claim is the
/// parent work that caused the creation. Typed system routes may finally name
/// an intrinsic cause they have already proved, such as a reviewed parent,
/// an authored-document digest, or an operator Draft. Dependency edges never
/// participate: scheduling order and creation causality are independent facts.
/// `offers_draft` names the operator Draft route in the refusal; only a
/// `task add` whose actor holds the Operator role can take it.
pub(super) fn resolve_creation_origin(
    authored: Option<&wire::Origin>,
    actor: &Actor,
    snapshot: &ValidatedSnapshot,
    intrinsic: Option<wire::TaskProvenance>,
    offers_draft: bool,
) -> Result<CreationCause, Outcome<CommandAnswer>> {
    if let Some(origin) = authored {
        return CreationCause::of(authored_provenance(origin)?);
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
        return CreationCause::of(wire::TaskProvenance::Task(parent.born().clone()));
    }
    if let Some(provenance) = intrinsic {
        return CreationCause::of(provenance);
    }
    let origins = "supply --origin ack:<steering-key> or --origin task:<parent-born-stamp>, or create while holding one active claim";
    let with_draft;
    let detail = if offers_draft {
        with_draft = format!(
            "{origins}; without either, record a Draft with --draft --project PROJECT and file it with spice task file DRAFT"
        );
        with_draft.as_str()
    } else {
        origins
    };
    Err(refusal(
        "task::origin-required",
        Some(actor.to_string()),
        detail,
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

pub(super) fn seq(value: u64) -> Seq {
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
