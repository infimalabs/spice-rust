use super::*;

/// Project and decide `task next` from the snapshot's live allocation frontier.
///
/// Terminal task heads stay behind the snapshot index. Only dependency stamps
/// named by the live graph consult their compact standing, so request work is
/// proportional to live vertices and edges rather than retained history.
pub(super) fn read_next_work(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
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

    let mut held = entries.iter().enumerate().filter(|(_, entry)| {
        entry.claim().is_some_and(|claim| {
            claim.actor == principal && claim.disposition == ClaimDisposition::Active
        })
    });
    let first_held = held.next();
    if held.next().is_some() {
        return Err(protocol_fault(
            "claim::active-ambiguous",
            "one principal holds more than one active claim",
        ));
    }

    let subscription = allocation_subscription(residency.body(), &principal)?;
    let recent_cell = observed.recent_claim_cell(&principal);
    let environment = observed.environment();
    let now = current_instant()?;
    let state = if let Some((index, _)) = first_held {
        NextWorkState::Active {
            task: next_task(frontier_heads[index]),
        }
    } else if let Some(selected) = board::select_with_satisfied_at(
        &entries,
        &principal,
        &subscription,
        allocation.band(),
        recent_cell.as_ref(),
        &environment,
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
            task: next_task(heads),
        }
    } else {
        NextWorkState::Dry {
            board: dry_counts(
                &entries,
                &satisfied,
                &origin_threads,
                projects,
                &actor_route(&principal, residency.body())?,
            ),
            team: subscription.policy,
        }
    };

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
        principal: principal.clone(),
        project_policy: BoardProjectPolicy {
            public_stems: projects.public_stems().iter().cloned().collect(),
            public_min_depth: projects.public_min_depth(),
            public_max_depth: projects.public_max_depth(),
        },
        actor_route: actor_route(&principal, &residency.body)?,
        allocation_band: allocation.band(),
        recent_cell,
        next: Some(state),
        tasks: Vec::new(),
        offer: Vec::new(),
        blocked: Vec::new(),
        origins: Default::default(),
        history: None,
        notes: Default::default(),
        acknowledgments: Default::default(),
        closures: Default::default(),
        steps: Default::default(),
        trust: positioned_trust(observed),
        appliance,
        residency,
        addresses: Default::default(),
    })
}

type FrontierProjection<'a> = (
    Vec<Entry>,
    board::ResolvedTasks,
    Vec<String>,
    Vec<&'a journal::TaskHeads>,
    bus::AllocationWork,
);

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
        if matches!(
            heads.workflow().body().status.as_str(),
            "completed" | "deleted"
        ) {
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
        if let Some(preparation) = &statement.preparation
            && let Some(parent) = observed.task_heads(&preparation.parent)
        {
            satisfied.know(preparation.parent.clone(), parent.statement().body().intent);
        }
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
        .with_phase_handoff(phase_handoff);
        origin_threads.push(task_origin_thread(
            observed,
            &statement.born,
            residency,
            &handle,
        )?);
        entries.push(entry);
        frontier_heads.push(heads);
    }
    let work = allocation.work();
    Ok((entries, satisfied, origin_threads, frontier_heads, work))
}

fn next_task(heads: &journal::TaskHeads) -> NextWorkTask {
    NextWorkTask {
        statement: positioned(heads.statement()),
        workflow: positioned(heads.workflow()),
        claim: heads.claim().map(positioned),
        lease: heads.lease().map(positioned),
    }
}

fn dry_counts(
    entries: &[Entry],
    satisfied: &board::ResolvedTasks,
    origin_threads: &[String],
    projects: &ProjectPolicy,
    route: &wire::intent::BoardActorRoute,
) -> NextWorkCounts {
    let visible = entries
        .iter()
        .zip(origin_threads)
        .filter(|(entry, origin)| actor_route_matches_entry(entry, origin, projects, route))
        .map(|(entry, _)| entry);
    let mut counts = NextWorkCounts {
        active: 0,
        stale: 0,
        blocked: 0,
        waiting: 0,
        oops: entries
            .iter()
            .filter(|entry| {
                entry.project().visibility().hidden_stem() == Some("oops")
                    && entry.workflow().status == "pending"
                    && entry.workflow().completion.is_none()
            })
            .count(),
    };
    for entry in visible.filter(|entry| {
        entry.workflow().status == "pending" && entry.workflow().completion.is_none()
    }) {
        match entry.claim().map(|claim| claim.disposition) {
            Some(ClaimDisposition::Active) => counts.active += 1,
            Some(ClaimDisposition::Lapsed) => counts.stale += 1,
            _ => {}
        }
        if !board::dependencies_satisfied(entry, satisfied) {
            counts.blocked += 1;
        }
        if entry.workflow().deferral.is_some() {
            counts.waiting += 1;
        }
    }
    counts
}

fn actor_route_matches_entry(
    entry: &Entry,
    origin_thread: &str,
    projects: &ProjectPolicy,
    route: &wire::intent::BoardActorRoute,
) -> bool {
    if entry.project().as_str() == route.project {
        return true;
    }
    if !entry.project().visibility().is_hidden()
        && projects.public_stems().iter().any(|stem| {
            entry.project().as_str() == stem
                || entry
                    .project()
                    .as_str()
                    .strip_prefix(stem)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    {
        return true;
    }
    origin_thread == route.origin_thread
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
