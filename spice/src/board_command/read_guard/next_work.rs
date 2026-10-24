use super::*;
use crate::resident::message_surface;
use occupant_match::OccupantMatch;

mod occupant_match;

/// The resident's retained session profiles, when message startup restored
/// them: what a running lane's `task next` ranks in-band ties by.
pub(crate) type OccupantProfiles<'a> = Option<&'a std::sync::Mutex<message_surface::MessageState>>;

/// Project and decide `task next` from the snapshot's live allocation frontier.
///
/// Terminal task heads stay behind the snapshot index. Only dependency stamps
/// named by the live graph consult their compact standing, so request work is
/// proportional to live vertices and edges rather than retained history.
pub(crate) fn read_next_work(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    profiles: OccupantProfiles<'_>,
) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
    let residency = observed.residency().ok_or_else(|| {
        store_fault(
            "board::residency-missing",
            "a running resident has no current residency head",
        )
    })?;
    if let Some(fault) = observed.allocation_fault() {
        return Err(match fault {
            bus::AllocationFault::ActiveTerminal { task } => protocol_fault(
                "board::allocation-active-terminal",
                &format!("terminal task {task} retains an active claim"),
            ),
        });
    }
    let (entries, satisfied, origin_threads, frontier_heads, _work) =
        project_frontier(observed, projects, residency.body())?;

    let held = held_index(&entries, &principal)?;

    let subscription = allocation_subscription(residency.body(), &principal)?;
    let recent_cell = observed.recent_claim_cell(&principal);
    let environment = observed.environment();
    let now = current_instant()?;
    let design_request = if held.is_none() {
        project_design_request(bus, observed, &entries, residency.body(), &principal)?
    } else {
        None
    };
    // Only a selection asks for scores: a held claim or an owed Design request
    // answers without fitting the frequency profile or taking the profile lock.
    let selecting = held.is_none() && design_request.is_none();
    let running = running_occupant(residency.body(), &principal).filter(|_| selecting);
    let mut occupant_profiles = match (profiles, running) {
        (Some(profiles), Some((lane, occupant))) => Some((
            message_surface::lock_state(profiles)
                .map_err(crate::resident::design_awareness::command_answer_outcome)?,
            lane,
            occupant,
        )),
        _ => None,
    };
    let matcher = occupant_profiles.as_mut().map(|(state, lane, occupant)| {
        OccupantMatch::new(&mut state.profiles, &entries, *lane, *occupant)
    });
    let match_score = |entry: &Entry| matcher.as_ref().map_or(0.0, |matcher| matcher.score(entry));
    let state = if let Some(index) = held {
        NextWorkState::Active {
            task: next_task(frontier_heads[index], &principal),
        }
    } else if let Some(request) = design_request {
        NextWorkState::DesignRequest { request }
    } else if let Some(selected) = board::select_with_satisfied_at(
        &entries,
        board::Claimant {
            actor: &principal,
            subscription: &subscription,
            recent_cell: recent_cell.as_ref(),
            environment: &environment,
            match_score: matcher
                .as_ref()
                .map(|_| &match_score as &dyn Fn(&Entry) -> f64),
        },
        allocation.band(),
        &satisfied,
        now,
    ) {
        let heads = frontier_heads.get(selected.entry_index()).ok_or_else(|| {
            protocol_fault(
                "board::allocation-index-invalid",
                "selected task position is absent from the live frontier",
            )
        })?;
        NextWorkState::Candidate {
            task: next_task(heads, &principal),
        }
    } else {
        NextWorkState::Dry {
            board: dry_counts(
                &entries,
                &satisfied,
                &origin_threads,
                &principal,
                &actor_route(&principal, residency.body())?,
                &environment,
            ),
            team: subscription.policy,
        }
    };

    next_answer(
        observed,
        residency,
        &principal,
        projects,
        allocation,
        recent_cell,
        state,
    )
}

fn next_answer(
    observed: &bus::TaskSnapshot,
    residency: &journal::JournalHead<wire::ResidencyHead>,
    principal: &wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    recent_cell: Option<wire::ClaimCell>,
    next: NextWorkState,
) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
    let cursor = observed.cursor().ok_or_else(|| {
        store_fault(
            "board::cursor-missing",
            "a running resident has no durable snapshot cursor",
        )
    })?;
    let mut residency = positioned(residency);
    for lane in &mut residency.body.lanes {
        lane.capabilities.clear();
    }
    let appliance = spice::version::running()
        .map_err(Outcome::Fault)?
        .product
        .clone();
    Ok(BoardAnswer {
        schema: journal::SCHEMA_VERSION,
        cursor,
        oops_generation: observed.oops_generation().map(positioned),
        principal: principal.clone(),
        project_policy: BoardProjectPolicy {
            public_min_depth: projects.public_min_depth(),
            public_max_depth: projects.public_max_depth(),
        },
        actor_route: actor_route(principal, &residency.body)?,
        allocation_band: allocation.band(),
        recent_cell,
        next: Some(next),
        tasks: Vec::new(),
        offer: Vec::new(),
        blocked: Vec::new(),
        workflow_waiting: Vec::new(),
        design_cycles: Default::default(),
        origins: Default::default(),
        history: None,
        acknowledgments: Default::default(),
        closures: Default::default(),
        steps: Default::default(),
        trust: positioned_trust(observed),
        appliance,
        residency,
        addresses: Default::default(),
    })
}
fn held_index(
    entries: &[Entry],
    principal: &wire::Actor,
) -> Result<Option<usize>, Outcome<CommandAnswer>> {
    let mut held = entries.iter().enumerate().filter(|(_, entry)| {
        entry.claim().is_some_and(|claim| {
            claim.actor == *principal && claim.disposition == ClaimDisposition::Active
        })
    });
    let first = held.next().map(|(index, _)| index);
    if held.next().is_some() {
        return Err(protocol_fault(
            "claim::active-ambiguous",
            "one principal holds more than one active claim",
        ));
    }
    Ok(first)
}

fn project_design_request(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    entries: &[Entry],
    residency: &wire::ResidencyHead,
    principal: &wire::Actor,
) -> Result<Option<wire::intent::DesignRequest>, Outcome<CommandAnswer>> {
    running_occupant(residency, principal)
        .map(|(lane, occupant)| {
            crate::resident::design_awareness::project_request(
                bus, observed, entries, lane, occupant,
            )
        })
        .transpose()
        .map(Option::flatten)
}

/// The lane and occupant seated for `principal`, when it is an occupied lane.
fn running_occupant(
    residency: &wire::ResidencyHead,
    principal: &wire::Actor,
) -> Option<(wire::CanonicalUuid, wire::CanonicalUuid)> {
    let wire::Actor::Lane(lane) = principal else {
        return None;
    };
    residency
        .lanes
        .iter()
        .find(|row| row.lane == *lane && row.disposition == "occupied")
        .and_then(|row| row.occupant)
        .map(|occupant| (*lane, occupant))
}

type FrontierProjection<'a> = (
    Vec<Entry>,
    board::ResolvedTasks,
    Vec<String>,
    Vec<&'a journal::TaskHeads>,
    bus::AllocationWork,
);

/// Every open Design's cycle among a scoped read's rows, judged against the
/// live frontier: the same resolution live allocation uses, since the scoped
/// rows need not include the work those Designs released.
pub(in crate::board_command) fn scoped_design_cycles(
    observed: &bus::TaskSnapshot,
    projects: &ProjectPolicy,
    snapshot: &ValidatedSnapshot,
) -> Result<
    std::collections::BTreeMap<wire::TaskStamp, wire::rooms::DesignCycle>,
    Outcome<CommandAnswer>,
> {
    if !snapshot
        .entries
        .iter()
        .any(|entry| wire::intent::is_open_design(entry.statement().intent, entry.workflow()))
    {
        return Ok(std::collections::BTreeMap::new());
    }
    let (_, live, ..) = project_frontier(observed, projects, &snapshot.answer.residency.body)?;
    Ok(board::design_cycles(&snapshot.entries, &live))
}

pub(in crate::board_command) fn project_frontier<'a>(
    observed: &'a bus::TaskSnapshot,
    projects: &ProjectPolicy,
    residency: &wire::ResidencyHead,
) -> Result<FrontierProjection<'a>, Outcome<CommandAnswer>> {
    let allocation = observed.allocation_read();
    let frontier = allocation.frontier();
    let mut entries = Vec::with_capacity(frontier.len());
    let mut satisfied = board::ResolvedTasks::new();
    let mut origin_threads = Vec::with_capacity(frontier.len());
    let mut frontier_heads = Vec::with_capacity(frontier.len());
    let mut prior = None;
    for heads in frontier {
        let statement = heads.statement().body();
        if !heads.workflow().body().status.is_pending() {
            return Err(protocol_fault(
                "board::allocation-index-invalid",
                "live frontier contains a terminal workflow",
            ));
        }
        if prior
            .as_ref()
            .is_some_and(|prior: &wire::TaskStamp| prior >= &statement.born)
        {
            return Err(protocol_fault(
                "board::allocation-index-invalid",
                "live frontier is not in unique task-stamp order",
            ));
        }
        prior = Some(statement.born.clone());
        let references = statement.dependencies.iter().chain(
            statement
                .preparation
                .iter()
                .flat_map(|preparation| [&preparation.parent, &preparation.controller]),
        );
        for dependency in references {
            if let Some(standing) = allocation.dependency_standing(dependency)
                && standing.is_settled()
            {
                satisfied.settle(
                    dependency.clone(),
                    standing == bus::DependencyStanding::Completed,
                );
            }
        }
        // Control reads the parent's intent and whether the controller still
        // stands, so both must be known even when off the frontier.
        for known in statement
            .preparation
            .iter()
            .flat_map(|preparation| [&preparation.parent, &preparation.controller])
        {
            if let Some(task) = observed.task_heads(known) {
                satisfied.know(known.clone(), task.statement().body().intent);
            }
        }
        let preparation_decision = observed.facts().preparation_decision(&statement.born);
        let explicit_release = heads.claim().is_some_and(|claim| {
            observed
                .facts()
                .is_explicit_release(&statement.born, claim.position())
        });
        let phase_handoff = phase_handoff(observed.facts(), heads)?;
        if observed.facts().has_invalid_creator_team(&statement.born) {
            return Err(protocol_fault(
                "board::task-creator-team-invalid",
                "a non-lane task creation names a creator team",
            ));
        }
        let handle = TaskHandle::for_project(&statement.project, &statement.born);
        let entry = Entry::new(
            statement.clone(),
            heads.workflow().body().clone(),
            heads.claim().map(|head| head.body().clone()),
            heads.lease().map(|head| head.body().clone()),
            projects,
        )
        .map_err(|error| {
            store_fault(
                "board::head-invalid",
                &format!("task {handle} current heads are invalid: {error}"),
            )
        })?
        .with_creator_team(statement.creation.creator_team())
        .with_explicit_release(explicit_release)
        .with_phase_handoff(phase_handoff)
        .with_grouped(observed.facts().grouped(&statement.born))
        .with_design(observed.facts().design(statement))
        .with_preparation_decision(preparation_decision.cloned());
        origin_threads.push(task_origin_thread(
            observed,
            &statement.born,
            residency,
            &handle,
        )?);
        entries.push(entry);
        frontier_heads.push(heads);
    }
    satisfied.hold_released_work(&entries);
    let work = allocation.work();
    Ok((entries, satisfied, origin_threads, frontier_heads, work))
}

fn next_task(heads: &journal::TaskHeads, principal: &wire::Actor) -> NextWorkTask {
    let mut statement = positioned(heads.statement());
    withhold_taste_findings(principal, &mut statement.body.taste);
    NextWorkTask {
        statement,
        workflow: positioned(heads.workflow()),
        claim: heads.claim().map(positioned),
        lease: heads.lease().map(positioned),
    }
}

fn dry_counts(
    entries: &[Entry],
    satisfied: &board::ResolvedTasks,
    origin_threads: &[String],
    principal: &wire::Actor,
    route: &wire::intent::BoardActorRoute,
    environment: &wire::Environment,
) -> NextWorkCounts {
    let mut counts = NextWorkCounts {
        active: 0,
        stale: 0,
        blocked: 0,
        workflow_waiting: 0,
        designs: wire::rooms::DesignCycleCounts::default(),
        deferred: 0,
        boarded_elsewhere: Vec::new(),
    };
    let waits = board::work_waits(entries, environment);
    let pending = entries
        .iter()
        .zip(origin_threads)
        .filter(|(entry, origin)| actor_route_matches_entry(entry, origin, route))
        .filter(|(entry, _)| entry.is_filed() && entry.workflow().status.is_pending());
    for (entry, origin) in pending {
        if let Some(cycle) = board::design_cycle(entry, satisfied) {
            counts.designs.observe(cycle);
        } else {
            match waits.get(entry.born()) {
                Some(board::WorkWait::Blocked) => counts.blocked += 1,
                Some(board::WorkWait::Workflow) => counts.workflow_waiting += 1,
                Some(board::WorkWait::Selection) | None => {}
            }
        }
        if satisfied.speculative_design_work(entry.statement(), entry.workflow()) {
            continue;
        }
        match entry.claim() {
            Some(claim) if claim.disposition == ClaimDisposition::Active => {
                counts.active += 1;
                if *origin == route.origin_thread && claim.actor != *principal {
                    counts
                        .boarded_elsewhere
                        .push(wire::intent::BoardedElsewhere {
                            task: entry.handle().clone(),
                            holder: claim.actor.clone(),
                        });
                }
            }
            Some(claim) if claim.disposition == ClaimDisposition::Lapsed => counts.stale += 1,
            _ => {}
        }
        if entry.workflow().deferral.is_some() {
            counts.deferred += 1;
        }
    }
    counts
}

/// The default read route: every filed public row, plus rows created on the
/// principal's own origin thread.
fn actor_route_matches_entry(
    entry: &Entry,
    origin_thread: &str,
    route: &wire::intent::BoardActorRoute,
) -> bool {
    (!entry.project().visibility().is_hidden() && entry.is_filed())
        || origin_thread == route.origin_thread
}

fn task_origin_thread(
    observed: &bus::TaskSnapshot,
    task: &wire::TaskStamp,
    residency: &wire::ResidencyHead,
    handle: &TaskHandle,
) -> Result<String, Outcome<CommandAnswer>> {
    if observed.facts().has_duplicate_task_origin(task) {
        return Err(protocol_fault(
            "board::task-origin-duplicate",
            &format!("task {handle} has more than one creation fact"),
        ));
    }
    match observed.facts().task_origin(task) {
        Some(bus::TaskOrigin::Identity(identity)) => {
            route_actor_identity(identity).ok_or_else(|| {
                protocol_fault(
                    "board::task-origin-session-invalid",
                    "the creating vendor session identity cannot name an actor task route",
                )
            })
        }
        Some(bus::TaskOrigin::ResidencyIdentity(lane)) => {
            let occupant = residency
                .lanes
                .iter()
                .find(|row| row.lane == *lane && row.session.is_none())
                .and_then(|row| row.occupant)
                .ok_or_else(|| {
                    protocol_fault(
                        "board::task-origin-occupant-missing",
                        "a lane task has no authoritative creating occupant",
                    )
                })?;
            let identity = observed
                .facts()
                .lane_session_identity(*lane, occupant)
                .map_or_else(|| occupant.to_string(), str::to_owned);
            route_actor_identity(&identity).ok_or_else(|| {
                protocol_fault(
                    "board::task-origin-session-invalid",
                    "the creating vendor session identity cannot name an actor task route",
                )
            })
        }
        None => Err(protocol_fault(
            "board::task-origin-missing",
            &format!("task {handle} has no authoritative creation fact"),
        )),
    }
}
