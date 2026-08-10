use super::*;

pub(super) fn phase(name: &str) -> wire::Phase {
    name.parse()
        .expect("dialect phase is a lower-case wire phase")
}

pub(super) fn task_flow(
    flow: Option<&str>,
    acceptance: &[String],
) -> Result<Vec<wire::Phase>, Outcome<CommandAnswer>> {
    if flow.is_none() && acceptance.is_empty() {
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
    let origin_threads = task_origin_threads(observed.facts().facts(), &residency.body)?;
    let mut tasks = Vec::with_capacity(observed.tasks().len());
    let mut entries = Vec::with_capacity(observed.tasks().len());
    for heads in observed.tasks() {
        let statement = positioned(heads.statement());
        let workflow = positioned(heads.workflow());
        let claim = heads.claim().map(positioned);
        let lease = heads.lease().map(positioned);
        let handle = TaskHandle::for_project(&statement.body.project, &statement.body.incepted);
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
        })?;
        let visibility = entry.project().visibility().clone();
        let origin_thread = origin_threads
            .get(&statement.body.incepted)
            .cloned()
            .ok_or_else(|| {
                protocol_fault(
                    "board::task-origin-missing",
                    &format!("task {handle} has no authoritative creation fact"),
                )
            })?;
        entries.push(entry);
        tasks.push(BoardTask::from_authoritative(
            visibility,
            origin_thread,
            statement,
            workflow,
            claim,
            lease,
        ));
    }
    board::project_subscription_history(&mut residency.body, &entries, observed.facts().facts())
        .map_err(|error| store_fault("lineage::subscription-history-invalid", &error))?;
    let recent_cell = board::recent_claim_cell(&entries, observed.facts().facts(), &principal);
    for lane in &mut residency.body.lanes {
        lane.capabilities.clear();
    }
    let actor_route = actor_route(&principal, &residency.body)?;
    Ok(ValidatedSnapshot {
        cursor,
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
            residency,
        },
        entries,
    })
}

pub(super) fn actor_route(
    principal: &wire::Actor,
    residency: &wire::ResidencyHead,
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
                    .ok_or_else(|| {
                        protocol_fault(
                            "board::actor-route-occupant-missing",
                            "a legacy lane without session authority has no current occupant",
                        )
                    })?
                    .get()
                    .simple()
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
        return Some(identity.get().simple().to_string());
    }
    (!identity.is_empty()
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'))
    .then(|| identity.to_owned())
}

fn task_origin_threads(
    facts: &[wire::Envelope<wire::AnyFact>],
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
                            &format!("task creation fact has an invalid inception stamp: {error}"),
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
                            None => occupant.get().simple().to_string(),
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
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let actor = command.actor().to_string();
    spice::task_git::publish(
        &lane,
        spice::task_git::Metadata {
            label: entry.handle().as_str(),
            title: &entry.statement().title,
            project: &entry.statement().project,
            phase: entry.workflow().phase.as_str(),
            actor: &actor,
        },
    )
    .map(Some)
    .map_err(task_git_outcome)
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
        "dependencies" => View::Dependencies,
        "workflow" => View::Workflow,
        "claims" => View::Claims,
        other => {
            return refusal(
                "board::graph-view-invalid",
                Some(other.to_owned()),
                "choose dependencies, workflow, or claims",
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
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::graph::render(view, &entries).text().to_owned(),
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
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::handout::compose(&entries),
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
    if !statement.tags.is_empty() {
        source.push_str(&format!("Tags: {}\n", statement.tags.join(", ")));
    }
    let Some(document) = dialect::classify(&source).document().cloned() else {
        return store_fault(
            "doc::ledger-unrepresentable",
            "current task heads did not classify as a canonical task document",
        );
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: dialect::emit(&document),
        }),
    )
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
    pub(super) winner: TaskHandle,
    pub(super) actor: Actor,
    pub(super) subscription: wire::LineageSubscription,
    pub(super) residency_position: Seq,
    pub(super) projects: ProjectPolicy,
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
            "the allocating lane lineage has no durable task-subscription policy",
        )
    })?;
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lineage::subscription-ambiguous",
            "the allocating lane lineage has more than one durable task-subscription policy",
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

/// Refuse the allocator's write if the board it selected over has moved.
///
/// Four comparisons run in order and the caller sees the first one only: the
/// client's own witnesses, the residency position, the head set, the winner.
/// The last cannot refuse in production, and it is kept anyway.
///
/// It is unreachable because the head set precedes it. `AllocationHeads` carries
/// every task's statement, workflow, claim and lease position, so a head set
/// comparing equal means identical facts, and the entries rebuilt below are the
/// entries the snapshot selected over. The `board` crate reads no clock at all,
/// and eligibility turns on `workflow.deferral`, a head field only a commit can
/// clear, so waiting changes nothing the selection reads. Actor, subscription
/// and band all arrive from that same snapshot. Recomputing the selection here
/// therefore returns what it returned there, against itself.
///
/// It is kept because it is the only place the invariant is written down: the
/// head set is a conservative proxy that refuses on any movement anywhere, while
/// this says the lane is granted the task the allocator picked for it. Narrowing
/// the head set is what would make it live -- fewer fields, a digest, or only the
/// subscribed projects -- and a lease or workflow change that flips the selection
/// would then arrive here. Until that happens its only proof is fixture-level:
/// `a_winner_that_was_never_the_selection_refuses_the_allocator_write` hands it
/// an observation by hand, and reds when this comparison is severed while the
/// other six tests in that file stay green.
pub(super) fn guard_allocation(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationObservation,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let residency = read.residency_head()?.ok_or_else(|| {
            GuardError::Invalid("allocator current residency head is missing".to_owned())
        })?;
        if residency.position() != observed.residency_position {
            return Err(GuardError::refused(
                "board::witness-moved: allocator subscription head changed",
            ));
        }
        let heads = read.task_heads()?;
        let current = heads
            .iter()
            .map(|task| {
                (
                    TaskHandle::for_project(
                        &task.statement().body().project,
                        &task.statement().body().incepted,
                    ),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current != observed.heads {
            return Err(GuardError::refused(
                "board::witness-moved: allocator head set changed",
            ));
        }
        let entries = heads
            .iter()
            .map(|task| {
                Entry::new(
                    task.statement().body().clone(),
                    task.workflow().body().clone(),
                    task.claim().map(|head| head.body().clone()),
                    task.lease().map(|head| head.body().clone()),
                    &observed.projects,
                )
                .map_err(|error| {
                    GuardError::Invalid(format!("allocator current heads are invalid: {error}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if board::select(
            &entries,
            &observed.actor,
            &observed.subscription,
            observed.band,
            observed.recent_cell.as_ref(),
        )
        .is_some_and(|selected| selected.entry().handle() == &observed.winner)
        {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: allocator winner changed",
            ))
        }
    }));
}

pub(super) fn guard_capture(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationHeads,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let current = read
            .task_heads()?
            .iter()
            .map(|task| {
                (
                    TaskHandle::for_project(
                        &task.statement().body().project,
                        &task.statement().body().incepted,
                    ),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current == observed {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: capture head set changed",
            ))
        }
    }));
}

fn witnesses_hold(read: &JournalRead<'_>, expected: &[Witness]) -> Result<(), GuardError> {
    for witness in expected {
        let holds = witness_holds(read, witness)?;
        if !holds {
            return Err(GuardError::refused(format!(
                "board::witness-moved: {witness:?}"
            )));
        }
    }
    Ok(())
}

fn witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    match witness {
        Witness::Task { .. }
        | Witness::Absence { .. }
        | Witness::Existence { .. }
        | Witness::Edge { .. } => task_witness_holds(read, witness),
        Witness::Workflow { .. }
        | Witness::Claim { .. }
        | Witness::Lease { .. }
        | Witness::Deferral { .. }
        | Witness::Assignment { .. } => workflow_witness_holds(read, witness),
        Witness::Residency { .. }
        | Witness::Eligibility { .. }
        | Witness::Preview { .. }
        | Witness::Barrier { .. } => board_witness_holds(read, witness),
        Witness::Directive { .. }
        | Witness::Recoverable { .. }
        | Witness::Consent { .. }
        | Witness::Rule { .. }
        | Witness::Budget { .. }
        | Witness::Trust { .. }
        | Witness::Taste { .. }
        | Witness::Observation { .. }
        | Witness::Sealed { .. }
        | Witness::Transition { .. } => Err(GuardError::Invalid(
            "board transaction carried a witness outside its authority plane".to_owned(),
        )),
    }
}

fn task_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Task { task, head } => read
            .task_head(task)?
            .is_some_and(|observed| observed.position() == *head),
        Witness::Absence { task } => read.task_head(task)?.is_none(),
        Witness::Existence { task } => read.task_head(task)?.is_some(),
        Witness::Edge { task, after } => {
            let subject = read.task_head(task)?;
            let dependency = read.task_head(after)?;
            subject.is_some_and(|head| !head.body().dependencies.contains(after))
                && dependency.is_some()
        }
        _ => unreachable!("task witness partition is closed"),
    })
}

fn workflow_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Workflow { task, head, phase } => {
            read.workflow_head(task)?.is_some_and(|observed| {
                observed.position() == *head && observed.body().phase == *phase
            })
        }
        Witness::Claim { task, head } => {
            read.claim_head(task)?.map(|observed| observed.position()) == *head
        }
        Witness::Lease { task, head } => read
            .lease_head(task)?
            .is_some_and(|observed| observed.position() == *head),
        Witness::Deferral {
            task,
            head,
            deferred,
        } => read.workflow_head(task)?.is_some_and(|observed| {
            observed.position() == *head && observed.body().deferral.is_some() == *deferred
        }),
        Witness::Assignment {
            task,
            head,
            reviewer,
        } => read.workflow_head(task)?.is_some_and(|observed| {
            observed.position() == *head && observed.body().assignment.as_ref() == Some(reviewer)
        }),
        _ => unreachable!("workflow witness partition is closed"),
    })
}

fn board_witness_holds(read: &JournalRead<'_>, witness: &Witness) -> Result<bool, GuardError> {
    Ok(match witness {
        Witness::Residency {
            lane,
            occupant,
            epoch,
        } => read.residency_head()?.is_some_and(|observed| {
            observed.body().epoch == *epoch
                && observed.body().lanes.iter().any(|seat| {
                    seat.lane == *lane
                        && seat.occupant == Some(*occupant)
                        && seat.disposition == "occupied"
                })
        }),
        Witness::Eligibility { rule } => matches!(rule.as_str(), "next" | "capture" | "steal"),
        Witness::Preview { .. } => true,
        Witness::Barrier { name, digest } => {
            name.as_str() == "board" && board_barrier_from_read(read)? == *digest
        }
        _ => unreachable!("board witness partition is closed"),
    })
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

fn board_barrier_from_read(read: &JournalRead<'_>) -> Result<Digest, GuardError> {
    let heads = read
        .task_heads()?
        .into_iter()
        .map(|task| {
            (
                TaskHandle::for_project(
                    &task.statement().body().project,
                    &task.statement().body().incepted,
                ),
                task.statement().position(),
                task.workflow().position(),
            )
        })
        .collect::<Vec<_>>();
    let bytes = wire::encode(&heads)
        .map_err(|error| GuardError::Invalid(format!("board barrier did not encode: {error}")))?;
    Ok(Digest::compute(Domain::BoardBarrier, &bytes))
}

pub(super) fn task_origin(task: &wire::TaskStamp) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    format!("task:{task}").parse().map_err(|error| {
        protocol_fault(
            "task::origin-invalid",
            &format!("validated inception stamp did not form a task origin: {error}"),
        )
    })
}

pub(super) fn normalize_task_origin(
    origin: wire::Origin,
) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    let Some(identity) = origin.as_str().strip_prefix("task:") else {
        return Ok(origin);
    };
    let incepted = match identity.parse::<wire::TaskStamp>() {
        Ok(incepted) => Ok(incepted),
        Err(stamp_error) => identity
            .parse::<TaskHandle>()
            .map_err(|handle_error| format!("{stamp_error}; {handle_error}"))
            .and_then(|handle| handle.incepted().map_err(|error| error.to_string())),
    };
    let incepted = incepted.map_err(|error| {
        refusal(
            "task::origin-invalid",
            Some(error.to_string()),
            "name a task origin carrying one canonical inception stamp",
        )
    })?;
    task_origin(&incepted)
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
        return task_origin(parent.incepted());
    }
    if let Some(origin) = intrinsic {
        return Ok(origin);
    }
    Err(refusal(
        "task::origin-required",
        Some(actor.to_string()),
        "supply --origin ack:<steering-key> or --origin task:<parent-inception-stamp>, or create while holding one active claim",
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
