use super::*;

mod claim;
mod deferral;
mod edit;
mod oops;
mod requirements;
mod settlement;
mod taste_proposal;
pub(super) use deferral::{task_defer, task_file, task_wake};
pub(super) use settlement::{
    DecisionWrite, stage_conclusion, stage_rejection, task_close, task_conclude, task_delete,
};

#[cfg(test)]
pub(super) use claim::Possession;
#[cfg(test)]
pub(super) use claim::{ClaimPlan, claim_git_boundary, claim_grant, claim_plan, commit_claim};
use claim::{PreparedClaim, claim_room, prepare_capture_claim};
pub(super) use claim::{
    claim_activation_next_action, claim_lease, claim_win, mint_claim, refuse_held_design,
    task_parked_commits,
};
pub(super) use edit::task_edit;
pub(super) use oops::task_oops_add;
pub(super) use taste_proposal::taste_proposal_add;

/// One actor-filtered board snapshot whose cursor and entries were validated together.
pub(super) struct ValidatedSnapshot {
    pub(super) cursor: wire::Seq,
    pub(super) task_barrier: Digest,
    pub(super) facts: bus::TaskFacts,
    pub(super) answer: BoardAnswer,
    pub(super) entries: Vec<Entry>,
}

struct PreparedTask {
    task: TaskHandle,
    born: wire::TaskStamp,
    project: String,
    flow: Vec<wire::Phase>,
    phase: wire::Phase,
    origin: wire::Origin,
    creation: TaskCreationCapture,
    at: Instant,
    statement: TaskHead,
    workflow: WorkflowHead,
}

struct AtomicClaim<'a> {
    prepared: PreparedClaim<'a>,
    answer: Box<wire::intent::ClaimAnswer>,
}

fn prepare_mutation_claim<'a>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<Host<'a>>,
    snapshot: &ValidatedSnapshot,
    selected: &Entry,
    requested: bool,
) -> Result<Option<AtomicClaim<'a>>, Outcome<CommandAnswer>> {
    if !requested {
        return Ok(None);
    }
    let room_context = claim_room(bus, repository, selected)?;
    let prepared = prepare_capture_claim(bus, command, repository, snapshot, selected)?;
    let origin_context = task_steering_origin_output(bus, &snapshot.facts, selected)?;
    let answer =
        Box::new(prepared.answer(command, snapshot, selected, origin_context, room_context));
    Ok(Some(AtomicClaim { prepared, answer }))
}

fn prepare_mutation_write(
    command: &wire::AdmittedCommand,
    entry: &Entry,
    snapshot: &ValidatedSnapshot,
    atomic_claim: Option<&AtomicClaim<'_>>,
) -> Result<JournalWrite, Outcome<CommandAnswer>> {
    let at = match atomic_claim {
        Some(claim) => claim.prepared.at(),
        None => current_instant()?,
    };
    let origin = task_origin(entry.born())?;
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Some(claim) = atomic_claim {
        claim.prepared.guard(&mut write, snapshot, command);
    }
    Ok(write)
}

pub(super) fn task_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    task_add_with(bus, command, add, repository, || {})
}

pub(super) fn task_add_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    repository: Option<Host<'_>>,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = draft_creation_admitted(add) {
        return outcome;
    }
    create_task(bus, command, add, repository, after_validation)
}

/// A Draft names its exact public project and enters no claim or preparation.
/// Both attachments are filing-time authority, so the repair moves them to
/// the file transition rather than dropping them.
fn draft_creation_admitted(add: &TaskAdd) -> Result<(), Outcome<CommandAnswer>> {
    if !add.draft {
        return Ok(());
    }
    if add.project.is_none() {
        return Err(refusal(
            "task::draft-project-required",
            None,
            "name the Draft's public project with --project PROJECT",
        ));
    }
    if add.claim {
        return Err(refusal(
            "task::draft-claim-refused",
            None,
            "create the Draft without --claim, then claim it with spice task file DRAFT --claim",
        ));
    }
    if add.preparation.is_some() {
        return Err(refusal(
            "task::draft-preparation-refused",
            None,
            "create the Draft without --preparation, then bind it with spice task file DRAFT --preparation CONTROLLER",
        ));
    }
    Ok(())
}

const PROJECT_REPAIR: &str = "name --project PROJECT as one lowercase word of [0-9a-z_]";

/// Admit one manually named project, refusing a malformed label.
fn admit_manual_project(requested: &str) -> Result<Project, Outcome<CommandAnswer>> {
    Project::admit(requested)
        .map_err(|error| refusal("task::add-refused", Some(error.to_string()), PROJECT_REPAIR))
}

fn oops_claim_project(
    actor: &wire::Actor,
    entries: &[Entry],
) -> Result<Project, Outcome<CommandAnswer>> {
    let mut projects = entries
        .iter()
        .filter(|entry| {
            entry.claim().is_some_and(|claim| {
                claim.actor == *actor && claim.disposition == wire::ClaimDisposition::Active
            })
        })
        .map(|entry| entry.project().clone());
    let Some(project) = projects.next() else {
        return Err(refusal(
            "task::oops-project-required",
            Some(actor.to_string()),
            "pass --project because this actor has no active claim",
        ));
    };
    if projects.next().is_some() {
        return Err(refusal(
            "task::oops-project-ambiguous",
            Some(actor.to_string()),
            "pass --project because this actor has more than one active claim",
        ));
    }
    Ok(project)
}
/// Admit one manual creation's project and read the actor's board against the
/// tasks the request names, refusing a malformed project or a dependency on an
/// unfiled Draft.
fn admit_add_project(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
) -> Result<(Project, ValidatedSnapshot), Outcome<CommandAnswer>> {
    let project = admit_manual_project(add.project.as_deref().unwrap_or(Project::PRIVATE))?;
    let roots = task_add_roots(add);
    let snapshot = validated_working_snapshot_for_tasks(bus, command.actor().clone(), &roots)?;
    let unfiled = referenced_unfiled(&snapshot.entries, add.depends.iter());
    if !unfiled.is_empty() {
        return Err(unfiled_dependency_refusal(&unfiled));
    }
    Ok((project, snapshot))
}

/// Create one manually added task: admit its project, validate the request
/// against the actor's board, prepare its heads, and commit them with any
/// claim the request takes.
fn create_task(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    repository: Option<Host<'_>>,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let (project, snapshot) = match admit_add_project(bus, command, add) {
        Ok(admitted) => admitted,
        Err(outcome) => return outcome,
    };
    let filing = if add.draft {
        wire::TaskFiling::Draft
    } else {
        wire::TaskFiling::Filed
    };
    let intrinsic = operator_draft_origin(command, filing);
    let offers_draft = command.holds(wire::principal::PrincipalRole::Operator);
    let origin = match resolve_creation_origin(
        add.origin.as_ref(),
        command.actor(),
        &snapshot,
        intrinsic,
        offers_draft,
    ) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let creation = match task_creation_capture(bus, command, &snapshot) {
        Ok(creation) => creation,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) =
        validate_acceptance(&add.acceptance).and_then(|()| validate_pitch(&add.pitch))
    {
        return outcome;
    }
    let (flow, dependency_witnesses) = match task_flow_for_dependencies(command, add, &snapshot) {
        Ok(observation) => observation,
        Err(outcome) => return outcome,
    };
    let mut prepared = match prepare_task(
        command,
        add,
        &project,
        TaskSeed {
            cause: origin,
            creation,
            flow,
            filing,
        },
    ) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let taste_review = match prepared_taste_review(
        repository,
        &snapshot,
        command.actor(),
        &prepared.statement,
        filing,
    ) {
        Ok(review) => review.filter(TasteReview::required),
        Err(outcome) => return outcome,
    };
    if let Some(review) = &taste_review
        && captures_implementation(command)
    {
        return review.refusal(
            "captured implementation requires taste-reviewed task prose".to_owned(),
            "correct the capture title and retry; no task or claim was created and the local commits remain on this lane",
        );
    }
    if let Err(outcome) = admit_prepared(&snapshot, command, add, filing, &mut prepared) {
        return outcome;
    }
    if let Some(review) = &taste_review {
        hold_for_taste(&mut prepared.flow, &mut prepared.statement.taste, review);
        prepared.phase = prepared.flow[0].clone();
        prepared.workflow.flow.clone_from(&prepared.flow);
        prepared.workflow.phase.clone_from(&prepared.phase);
    }
    let post = prepared_entry(&prepared);
    let added_claim = if add.claim {
        match prepare_capture_claim(bus, command, repository, &snapshot, &post) {
            Ok(claim) => Some(claim),
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    after_validation();
    commit_prepared_task(
        bus,
        command,
        add.clone(),
        &snapshot,
        AdmittedCreation {
            prepared,
            post,
            dependency_witnesses,
            claim: added_claim,
        },
        |_| {},
    )
}

/// The taste review of a prepared statement. A Draft asserts nothing yet:
/// taste and preparation bind when it is filed.
fn prepared_taste_review(
    repository: Option<Host<'_>>,
    snapshot: &ValidatedSnapshot,
    actor: &wire::Actor,
    statement: &wire::TaskHead,
    filing: wire::TaskFiling,
) -> Result<Option<TasteReview>, Outcome<CommandAnswer>> {
    if filing.is_draft() {
        return Ok(None);
    }
    task_taste_review(
        repository,
        snapshot,
        actor,
        &statement.title,
        &statement.pitch,
        statement.description.as_deref(),
        &statement.acceptance,
    )
}

/// Bind a filed statement to its planning parent and admit its route there; a
/// Draft binds at filing instead.
fn admit_prepared(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    filing: wire::TaskFiling,
    prepared: &mut PreparedTask,
) -> Result<(), Outcome<CommandAnswer>> {
    if filing.is_draft() {
        return Ok(());
    }
    planning::admit_preparation(snapshot, command, add, &mut prepared.statement)?;
    planning::refuse_disposed_construction(snapshot, &prepared.statement, &prepared.workflow)
}

/// The operator is the root authority, so a Draft it authors needs no further
/// cause. A lane's Draft still names its directive or held parent, so drafting
/// never mints causeless work for an agent.
fn operator_draft_origin(
    command: &wire::AdmittedCommand,
    filing: wire::TaskFiling,
) -> Option<wire::TaskProvenance> {
    (filing.is_draft() && command.holds(wire::principal::PrincipalRole::Operator))
        .then_some(wire::TaskProvenance::OperatorDraft)
}

pub(super) fn task_add_roots(add: &TaskAdd) -> Vec<wire::TaskStamp> {
    let mut roots = add
        .depends
        .iter()
        .map(|handle| {
            handle
                .born()
                .expect("an admitted task handle carries a canonical task stamp")
        })
        .collect::<Vec<_>>();
    if let Some(parent) = add
        .origin
        .as_ref()
        .and_then(|origin| wire::TaskProvenance::authored(origin.as_str()).ok())
        .and_then(|provenance| provenance.parent().cloned())
    {
        roots.push(parent);
    }
    roots.extend(add.preparation.as_ref().map(|parent| {
        parent
            .born()
            .expect("an admitted preparation handle carries a canonical task stamp")
    }));
    roots
}

pub(super) fn task_review_roots(review: &TaskReview) -> Vec<wire::TaskStamp> {
    let mut roots = std::iter::once(&review.task)
        .chain(review.followup.iter())
        .map(|handle| {
            handle
                .born()
                .expect("an admitted task handle carries a canonical task stamp")
        })
        .collect::<Vec<_>>();
    roots.extend(
        review
            .then
            .iter()
            .flat_map(|followup| task_add_roots(&followup.add)),
    );
    roots
}

/// One creation admitted against its snapshot: the prepared heads, the entry
/// the task stands as once born, the dependency witnesses that guard the
/// write, and any claim taken in the same transaction.
struct AdmittedCreation<'a> {
    prepared: PreparedTask,
    post: Entry,
    dependency_witnesses: Vec<Witness>,
    claim: Option<PreparedClaim<'a>>,
}

fn commit_prepared_task<B, F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: B,
    snapshot: &ValidatedSnapshot,
    creation: AdmittedCreation,
    extra_guard: F,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize,
    F: FnOnce(&mut JournalWrite),
{
    let AdmittedCreation {
        prepared,
        post,
        dependency_witnesses,
        claim: added_claim,
    } = creation;
    let origin_context = if added_claim.is_some() {
        match task_steering_origin_output(bus, &snapshot.facts, &post) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    // A task born in this transaction has published nothing in its room.
    let claim_answer = added_claim
        .as_ref()
        .map(|claim| Box::new(claim.answer(command, snapshot, &post, origin_context, None)));
    let next = claim_answer
        .as_ref()
        .and_then(|claim| claim.next.clone())
        .or_else(|| next_action(snapshot, &post));
    let at = added_claim.as_ref().map_or(prepared.at, PreparedClaim::at);
    let mut write = JournalWrite::new(
        command.actor().clone(),
        prepared.origin.clone(),
        at,
        Producer::CommandDoor,
    );
    let mut expected = command.expected().to_vec();
    expected.extend(dependency_witnesses);
    guard(&mut write, expected);
    prepared.creation.guard(&mut write);
    if let Some(claim) = &added_claim {
        claim.guard(&mut write, snapshot, command);
    }
    extra_guard(&mut write);
    let task = prepared.task.clone();
    if let Err(outcome) = append_prepared_task(&mut write, prepared, FactCause::Root) {
        return outcome;
    }
    if let Some(claim) = added_claim
        && let Err(outcome) = claim.append(&mut write, command, &post)
    {
        return outcome;
    }
    let answer = wire::intent::TaskAnswer {
        task,
        status: "pending".to_owned(),
        completion: None,
        claim: claim_answer,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, request_body),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::add-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}

/// Append one prepared task's birth fact and both heads. The fact repeats the
/// prepared statement's authored fields, so a statement adjusted after
/// preparation is born as adjusted.
fn append_prepared_task(
    write: &mut JournalWrite,
    prepared: PreparedTask,
    cause: FactCause,
) -> Result<journal::PendingFact, Outcome<CommandAnswer>> {
    let added = write
        .fact(
            body::TaskAdded {
                preparation: prepared.statement.preparation.clone(),
                intent: prepared.statement.intent,
                task: prepared.born.clone(),
                title: prepared.statement.title.clone(),
                pitch: prepared.statement.pitch.clone(),
                description: prepared.statement.description.clone(),
                acceptance: prepared.statement.acceptance.clone(),
                requirements: prepared.statement.requirements.clone(),
                project: prepared.project,
                filing: prepared.statement.filing,
                priority: prepared.statement.priority,
                taste: prepared.statement.taste.clone(),
                document_slug: prepared.statement.document_slug.clone(),
                taste_proposal: prepared.statement.taste_proposal.clone(),
                depends: prepared.statement.dependencies.clone(),
                flow: prepared.flow,
                phase: prepared.phase,
                revision: 1,
                creation: prepared.creation.context(),
            },
            cause,
        )
        .map_err(|error| store_fault("task::add-build", &error.to_string()))?;
    let statement = prepared.statement;
    write.task_head(added, move |position| TaskHead {
        queue_origin: position,
        revision: position,
        ..statement
    });
    write.workflow_head(prepared.born, added, move |_| prepared.workflow);
    Ok(added)
}

fn prepared_entry(prepared: &PreparedTask) -> Entry {
    Entry::new(
        prepared.statement.clone(),
        prepared.workflow.clone(),
        None,
        None,
    )
    .expect("prepared task already passed entry validation")
    .with_creator_team(prepared.creation.creator_team())
}

/// What one new task is born from: the cause it answers, the creation capture
/// it records, the flow it opens in, and its filing.
#[derive(Clone)]
struct TaskSeed {
    cause: CreationCause,
    creation: TaskCreationCapture,
    flow: Vec<wire::Phase>,
    filing: wire::TaskFiling,
}

fn prepare_task(
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    project: &Project,
    seed: TaskSeed,
) -> Result<PreparedTask, Outcome<CommandAnswer>> {
    let handles = command
        .expected()
        .iter()
        .filter_map(|witness| match witness {
            Witness::Absence { task } => Some(task.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [born] = handles.as_slice() else {
        return Err(protocol_fault(
            "task::add-witness-invalid",
            "task add requires exactly one intended absent birth stamp",
        ));
    };
    prepare_task_for(add, project, seed, born.clone())
}

fn prepare_task_for(
    add: &TaskAdd,
    project: &Project,
    seed: TaskSeed,
    born: wire::TaskStamp,
) -> Result<PreparedTask, Outcome<CommandAnswer>> {
    let TaskSeed {
        cause,
        creation,
        flow,
        filing,
    } = seed;
    if cause.provenance.parent() == Some(&born) {
        return Err(refusal(
            "task::origin-self",
            Some(cause.provenance.to_string()),
            "name the steering directive or existing parent task that caused this task",
        ));
    }
    let project_text = project.as_str().to_owned();
    let task = TaskHandle::for_filing(&project_text, &born, filing);
    let phase = flow[0].clone();
    let at = current_instant()?;
    let statement = TaskHead {
        preparation: None,
        intent: add.intent,
        born: born.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: add.title.clone(),
        pitch: add.pitch.trim().to_owned(),
        description: add.description.clone(),
        acceptance: add.acceptance.clone(),
        requirements: add.requirements.clone(),
        project: project_text.clone(),
        filing,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: add
            .depends
            .iter()
            .map(TaskHandle::born)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                refusal(
                    "task::dependency-invalid",
                    Some(error.to_string()),
                    "name dependencies carrying canonical birth stamps",
                )
            })?,
        provenance: cause.provenance,
        creation: creation.context(),
        revision: Seq::new(1).expect("placeholder revision is non-zero"),
    };
    let workflow = WorkflowHead {
        flow: flow.clone(),
        phase: phase.clone(),
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    };
    if let Err(error) = Entry::new(statement.clone(), workflow.clone(), None, None) {
        return Err(refusal(
            "task::add-refused",
            Some(error.to_string()),
            "correct the task project, dependencies, or phase flow and retry",
        ));
    }
    Ok(PreparedTask {
        task,
        born,
        project: project_text,
        flow,
        phase,
        origin: cause.origin,
        creation,
        at,
        statement,
        workflow,
    })
}

fn captures_implementation(command: &wire::AdmittedCommand) -> bool {
    command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Eligibility { rule } if rule.as_str() == CAPTURE_WORK_RULE
        )
    })
}

fn recovers_implementation(command: &wire::AdmittedCommand) -> bool {
    command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Eligibility { rule } if rule.as_str() == "recover"
        )
    })
}

fn task_flow_for_dependencies(
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    snapshot: &ValidatedSnapshot,
) -> Result<(Vec<wire::Phase>, Vec<Witness>), Outcome<CommandAnswer>> {
    let requested = planning::default_flow(add.intent, add.flow.as_deref(), &add.acceptance);
    let adopts_implementation =
        captures_implementation(command) || recovers_implementation(command);
    let requested = if adopts_implementation && requested.is_none() {
        Some("todo,review")
    } else {
        requested
    };
    let mut flow = task_flow(requested, &add.acceptance)?;
    planning::admit_design_flow(add.intent, &flow)?;
    if adopts_implementation
        && (add.intent.is_some() || gate::lacks_commit_authority(flow[0].as_str()))
    {
        return Err(refusal(
            "task::capture-planning",
            None,
            "captured implementation cannot create or replace a planning contract",
        ));
    }
    let dependencies = add
        .depends
        .iter()
        .map(TaskHandle::born)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            refusal(
                "task::dependency-invalid",
                Some(error.to_string()),
                "name dependencies carrying canonical birth stamps",
            )
        })?;
    let witnesses = derive_dependency_flow(
        &mut flow,
        add.flow.is_some() || add.intent.is_some(),
        &dependencies,
        snapshot,
    );
    Ok((flow, witnesses))
}

/// Witness each dependency's workflow and, for a defaulted `flow`, omit the
/// review phase when every dependency is awaiting review. A named flow is
/// honored as written.
pub(super) fn derive_dependency_flow(
    flow: &mut Vec<wire::Phase>,
    explicit: bool,
    dependencies: &[TaskStamp],
    snapshot: &ValidatedSnapshot,
) -> Vec<Witness> {
    let mut witnesses = Vec::with_capacity(dependencies.len());
    let mut all_review = !dependencies.is_empty();
    for born in dependencies {
        let Some(heads) = snapshot
            .answer
            .tasks
            .iter()
            .find(|heads| &heads.statement.body.born == born)
        else {
            all_review = false;
            witnesses.push(Witness::Absence { task: born.clone() });
            continue;
        };
        all_review &= heads.workflow.body.phase.as_str() == "review"
            && heads.workflow.body.status.is_pending();
        witnesses.push(workflow_witness(heads));
    }
    if !explicit {
        omit_review_phase(flow, all_review);
    }
    witnesses
}

/// Drop the review phase from a defaulted flow. Every default names a
/// non-review phase, so the flow stays nonempty.
pub(super) fn omit_review_phase(flow: &mut Vec<wire::Phase>, all_review: bool) {
    if all_review {
        flow.retain(|phase| phase.as_str() != "review");
    }
}

pub(super) fn task_note(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    note: &TaskNote,
) -> Outcome<CommandAnswer> {
    let content = match room::authored_observation(&note.note, &note.task) {
        Ok(content) => content,
        Err(outcome) => return outcome,
    };
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&note.task),
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &note.task) else {
        return refusal(
            "task::unknown",
            Some(note.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if let Err(outcome) = room::publishable_room(entry) {
        return outcome;
    }
    let next = next_action(&snapshot, entry);
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let publication = match room::mint_publication(bus, entry.born(), at.get().as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        wire::rooms::RoomPublished {
            room: entry.born().clone(),
            publication,
            content,
        },
        FactCause::Root,
    ) {
        return store_fault("task::note-build", &error.to_string());
    }
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: "noted".to_owned(),
        completion: None,
        claim: None,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, note.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::note-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}

pub(super) fn task_depends(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
) -> Outcome<CommandAnswer> {
    task_depends_with(bus, command, depends, || {})
}

pub(super) fn task_depends_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let roots = std::iter::once(depends.task.clone())
        .chain(depends.after.iter().cloned())
        .collect::<Vec<_>>();
    let snapshot = match validated_working_snapshot(bus, command.actor().clone(), &roots) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match dependency_entry(&snapshot, depends) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let statement = match dependency_statement(&snapshot.entries, entry, depends) {
        Ok(statement) => statement,
        Err(outcome) => return outcome,
    };
    after_validation();
    if let Err(outcome) =
        require_dependency_reachability_witnesses(&snapshot, entry, &statement, command)
    {
        return outcome;
    }
    let post = match Entry::new(
        statement.clone(),
        entry.workflow().clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
    ) {
        Ok(post) => post
            .with_grouped(entry.grouped())
            .with_design(entry.design()),
        Err(error) => {
            return refusal(
                "task::dependency-refused",
                Some(error.to_string()),
                "submit a valid acyclic dependency edit",
            );
        }
    };
    if !depends.remove
        && let Err(outcome) =
            planning::refuse_disposed_construction(&snapshot, post.statement(), post.workflow())
    {
        return outcome;
    }
    if depends.remove
        && post.workflow().phase.as_str() == "plan"
        && (post.workflow().flow.len() == 1
            || !substantive_acceptance(&post.statement().acceptance))
    {
        match accepted_pending_child(&snapshot, &post) {
            Ok(Some(child)) => {
                if let Err(outcome) = require_dependency_witness(
                    command,
                    &statement_witness(child),
                    "read the current plan and its remaining accepted child before removing a dependency",
                ) {
                    return outcome;
                }
                if let Err(outcome) = require_dependency_witness(
                    command,
                    &workflow_witness(child),
                    "read the current plan and its remaining accepted child before removing a dependency",
                ) {
                    return outcome;
                }
            }
            Ok(None) => {
                return refusal(
                    "task::plan-acceptance-orphaned",
                    Some(entry.handle().to_string()),
                    "retain a pending dependency carrying acceptance while the plan relies on one",
                );
            }
            Err(outcome) => return outcome,
        }
    }
    let next = next_action(&snapshot, &post);
    commit_dependency_edit(bus, command, depends, entry, statement, next, at)
}

fn require_dependency_witness(
    command: &wire::AdmittedCommand,
    witness: &Witness,
    remedy: &str,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.expected().contains(witness) {
        return Ok(());
    }
    Err(refusal(
        "task::dependency-witness-missing",
        Some(format!("{witness:?}")),
        remedy,
    ))
}

fn require_dependency_reachability_witnesses(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    statement: &TaskHead,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let added = statement
        .dependencies
        .iter()
        .filter(|dependency| !entry.statement().dependencies.contains(dependency))
        .cloned()
        .collect::<Vec<_>>();
    let mut required = Vec::new();
    append_dependency_reachability_witnesses(&snapshot.answer, &added, &mut required)?;
    let missing = required
        .iter()
        .find(|witness| !command.expected().contains(witness));
    if let Some(missing) = missing {
        return require_dependency_witness(
            command,
            missing,
            "read the complete current prerequisite graph before editing dependencies",
        );
    }
    Ok(())
}

fn dependency_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    depends: &TaskDepends,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    let unfiled = referenced_unfiled(&snapshot.entries, depends.after.iter());
    if !unfiled.is_empty() {
        return Err(unfiled_dependency_refusal(&unfiled));
    }
    let Some(entry) = task_entry(&snapshot.entries, &depends.task) else {
        return Err(refusal(
            "task::unknown",
            Some(depends.task.to_string()),
            "read board and name one current task handle",
        ));
    };
    if entry.workflow().status.is_completed() {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "edit dependencies only while the task is pending",
        ));
    }
    Ok(entry)
}

fn commit_dependency_edit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    entry: &Entry,
    statement: TaskHead,
    next: Option<Box<str>>,
    at: Instant,
) -> Outcome<CommandAnswer> {
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let dependencies = statement.dependencies.clone();
    let edited = match write.fact(
        body::TaskEdited {
            intent: None,
            task: entry.born().clone(),
            revision,
            dependencies: Some(dependencies),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::depends-build", &error.to_string()),
    };
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: "edited".to_owned(),
        completion: None,
        claim: None,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, depends.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::depends-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}

fn dependency_statement(
    entries: &[Entry],
    entry: &Entry,
    depends: &TaskDepends,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    let prerequisites = depends
        .after
        .iter()
        .map(|after| task_entry(entries, after))
        .collect::<Vec<_>>();
    let mut resolved = Vec::with_capacity(depends.after.len());
    for (after, prerequisite) in depends.after.iter().zip(prerequisites) {
        let Some(prerequisite) = prerequisite else {
            return Err(refusal(
                "task::dependency-unknown",
                Some(after.to_string()),
                "read board and name an existing prerequisite",
            ));
        };
        resolved.push(prerequisite);
    }
    let mut statement = entry.statement().clone();
    if depends.remove {
        for prerequisite in resolved {
            if !entry.statement().dependencies.contains(prerequisite.born()) {
                return Err(refusal(
                    "task::dependency-absent",
                    Some(format!(
                        "{} after {}",
                        entry.handle(),
                        prerequisite.handle()
                    )),
                    "name a dependency edge currently present on the task",
                ));
            }
            statement
                .dependencies
                .retain(|dependency| dependency != prerequisite.born());
        }
        return Ok(statement);
    }
    let mut added = Vec::new();
    for prerequisite in resolved {
        if prerequisite.born() == entry.born() {
            return Err(refusal(
                "task::dependency-refused",
                Some(format!(
                    "{} after {}",
                    entry.handle(),
                    prerequisite.handle()
                )),
                "a task cannot depend on itself",
            ));
        }
        if statement.dependencies.contains(prerequisite.born())
            || added.contains(prerequisite.born())
        {
            continue;
        }
        if dependency_reaches(entries, prerequisite.born(), entry.born()) {
            return Err(refusal(
                "task::dependency-refused",
                Some(format!(
                    "{} after {}",
                    entry.handle(),
                    prerequisite.handle()
                )),
                "name dependencies that keep the complete task graph acyclic",
            ));
        }
        statement.dependencies.push(prerequisite.born().clone());
        added.push(prerequisite.born().clone());
    }
    Ok(statement)
}

fn dependency_reaches(entries: &[Entry], start: &TaskStamp, target: &TaskStamp) -> bool {
    let mut pending = vec![start.clone()];
    let mut visited = Vec::new();
    while let Some(current) = pending.pop() {
        if &current == target {
            return true;
        }
        if visited.contains(&current) {
            continue;
        }
        visited.push(current.clone());
        if let Some(entry) = entries.iter().find(|entry| entry.born() == &current) {
            pending.extend(entry.statement().dependencies.iter().cloned());
        }
    }
    false
}

/// One unfiled row's filing standing and possession, for refusal detail.
pub(super) fn filing_state(entry: &Entry) -> String {
    let claim = match entry.claim().map(|claim| claim.disposition) {
        Some(ClaimDisposition::Active) => "active",
        Some(ClaimDisposition::Freed) => "freed",
        Some(ClaimDisposition::Lapsed) => "lapsed",
        None => "none",
    };
    format!(
        "{} project={} filing={} status={} claim={claim}",
        entry.handle(),
        entry.project(),
        entry.statement().filing.as_str(),
        entry.workflow().status,
    )
}

fn unfiled_dependency_refusal(entries: &[&Entry]) -> Outcome<CommandAnswer> {
    let offenders = entries
        .iter()
        .map(|entry| filing_state(entry))
        .collect::<Vec<_>>()
        .join("; ");
    refusal(
        "task::dependency-unfiled",
        Some(offenders.clone()),
        &format!(
            "unfiled dependency endpoint(s): {offenders}; file every Draft first with spice task file HANDLE [--into PUBLIC_PROJECT]",
        ),
    )
}

fn referenced_unfiled<'a, 'b>(
    entries: &'a [Entry],
    handles: impl IntoIterator<Item = &'b TaskHandle>,
) -> Vec<&'a Entry> {
    let mut unfiled = Vec::new();
    for born in handles.into_iter().filter_map(|handle| handle.born().ok()) {
        let Some(entry) = entries.iter().find(|entry| entry.born() == &born) else {
            continue;
        };
        if !entry.is_filed()
            && !unfiled
                .iter()
                .any(|seen: &&Entry| seen.born() == entry.born())
        {
            unfiled.push(entry);
        }
    }
    unfiled
}
