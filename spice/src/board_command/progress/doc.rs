//! Applying a previewed task document: one atomic write that adds or edits
//! every authored task against the board barrier the preview was taken on.

use super::*;

struct DocPlan {
    preview: board::Preview,
    snapshot: ValidatedSnapshot,
    /// The roles the exported ledger named step authors through.
    roles: wire::principal::PrincipalRoles,
    touched: Vec<TaskHandle>,
    handles: std::collections::BTreeMap<usize, TaskHandle>,
}

struct DocTaskSpec {
    task: TaskHandle,
    born: wire::TaskStamp,
    title: String,
    /// Derived once from the node's title and description; a node has no
    /// author to ask and an existing statement keeps the pitch it has.
    pitch: String,
    description: Option<String>,
    annotations: Vec<String>,
    project: String,
    acceptance: Vec<String>,
    document_slug: String,
    priority: Option<wire::TaskPriority>,
    taste: Option<wire::TasteHold>,
    dependencies: Vec<wire::TaskStamp>,
    flow: Vec<wire::Phase>,
    initial: wire::Phase,
    released: Option<ReleasedPlan>,
}

pub(in crate::board_command) fn doc_apply(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let plan = match validate_doc_plan(bus, command, apply, projects, allocation) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    if let Some(outcome) = repeated_doc_apply(bus, apply, &plan.touched) {
        return outcome;
    }
    commit_doc_plan(bus, command, apply, repository, &plan)
}

fn validate_doc_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<DocPlan, Outcome<CommandAnswer>> {
    let preview = board::Preview::classify(&apply.document).map_err(|error| {
        refusal(
            "doc::classification-refused",
            Some(error.to_string()),
            "preview the current canonical document before applying it",
        )
    })?;
    if preview.digest() != &apply.preview {
        return Err(refusal(
            "doc::preview-moved",
            Some(apply.preview.to_string()),
            &format!("apply the document digest {}", preview.digest()),
        ));
    }
    let roles = principal_roles(bus)?;
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    require_doc_barrier(command, apply, &snapshot)?;
    let project = preview
        .document()
        .project
        .as_deref()
        .unwrap_or("task.document");
    projects.admit(project).map_err(|error| {
        refusal(
            "doc::project-refused",
            Some(error.to_string()),
            "author a project admitted by the captured task metadata",
        )
    })?;
    let touched = document_handles(
        preview.document(),
        &snapshot.answer,
        jiff::Timestamp::now().as_microsecond(),
    )?;
    if touched.is_empty() {
        return Err(refusal(
            "doc::empty",
            None,
            "author at least one task node before applying the document",
        ));
    }
    let handles = preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
        .map(|node| node.id)
        .zip(touched.iter().cloned())
        .collect();
    Ok(DocPlan {
        preview,
        snapshot,
        roles,
        touched,
        handles,
    })
}

fn require_doc_barrier(
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    snapshot: &ValidatedSnapshot,
) -> Result<(), Outcome<CommandAnswer>> {
    let barrier = board_barrier(&snapshot.answer);
    let preview = command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Preview { digest } if digest == &apply.preview));
    let board = command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Barrier { name, digest }
                if name.as_str() == "board" && digest == &barrier
        )
    });
    if command.expected().len() == 2 && preview && board {
        return Ok(());
    }
    Err(refusal(
        "doc::barrier-moved",
        Some(barrier.to_string()),
        "preview the current document against the current board and apply that exact digest",
    ))
}

fn repeated_doc_apply(
    bus: &Bus,
    apply: &DocApply,
    touched: &[TaskHandle],
) -> Option<Outcome<CommandAnswer>> {
    let snapshot = bus.task_snapshot().ok()?;
    let existing = snapshot.facts().iter().find(|envelope| {
        envelope.fact.kind() == wire::FactKind::DocApplied
            && envelope
                .fact
                .body()
                .get("digest")
                .and_then(serde_json::Value::as_str)
                == Some(apply.preview.as_str())
    })?;
    Some(Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: existing.seq,
            last: existing.seq,
        }),
        body: CommandAnswer::Doc(wire::intent::DocAnswer {
            preview: apply.preview.clone(),
            touched: touched.to_vec(),
        }),
    }))
}

fn commit_doc_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    repository: Option<&Repository>,
    plan: &DocPlan,
) -> Outcome<CommandAnswer> {
    let intrinsic = wire::TaskProvenance::Document(apply.preview.clone());
    let origin = match resolve_creation_origin(
        None,
        command.actor(),
        &plan.snapshot,
        Some(intrinsic),
        false,
    ) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let creates = plan.preview.document().nodes.iter().any(|node| {
        node.kind != dialect::NodeKind::SyntheticRoot
            && plan
                .handles
                .get(&node.id)
                .is_some_and(|handle| task_entry(&plan.snapshot.entries, handle).is_none())
    });
    let creation = if creates {
        match task_creation_capture(bus, command, &plan.snapshot) {
            Ok(creation) => Some(creation),
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    if let Some(creation) = &creation {
        creation.guard(&mut write);
    }
    let mut previous = None;
    let mut operations = 0_u32;
    for node in plan
        .preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
    {
        let spec = match doc_task_spec(plan, command, repository, node) {
            Ok(spec) => spec,
            Err(outcome) => return outcome,
        };
        previous = match write_doc_task(
            &mut write,
            bus,
            plan,
            spec,
            &origin,
            creation.as_ref(),
            previous,
        ) {
            Ok(fact) => Some(fact),
            Err(outcome) => return outcome,
        };
        operations = operations.saturating_add(1);
    }
    finish_doc_commit(bus, command, apply, plan, write, previous, operations)
}

fn doc_task_spec(
    plan: &DocPlan,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    node: &dialect::Node,
) -> Result<DocTaskSpec, Outcome<CommandAnswer>> {
    let task = plan
        .handles
        .get(&node.id)
        .expect("every applied node was assigned a handle")
        .clone();
    let dependencies = plan
        .preview
        .document()
        .edges
        .iter()
        .filter(|edge| edge.source == node.id)
        .filter_map(|edge| plan.handles.get(&edge.target))
        .map(|handle| {
            handle
                .born()
                .expect("document handles carry canonical task stamps")
        })
        .collect::<Vec<_>>();
    let project = plan
        .preview
        .document()
        .project
        .clone()
        .unwrap_or_else(|| "task.document".to_owned());
    validate_acceptance(&node.acceptance)?;
    let existing = task_entry(&plan.snapshot.entries, &task);
    let pitch = existing
        .map(|entry| entry.statement().pitch.clone())
        .filter(|pitch| !pitch.is_empty())
        .unwrap_or_else(|| wire::pitch::derive(&node.title, Some(&node.description)));
    let mut flow = if node.flow.is_empty() {
        task_flow(None, &node.acceptance)?
    } else {
        node.flow
            .iter()
            .map(|station| phase(station.as_str()))
            .collect()
    };
    if existing.is_none() {
        let _dependency_witnesses = derive_dependency_flow(
            &mut flow,
            !node.flow.is_empty(),
            &dependencies,
            &plan.snapshot,
        );
    }
    let taste_review = task_taste_review(
        repository,
        &plan.snapshot,
        command.actor(),
        &node.title,
        &pitch,
        None,
        &node.acceptance,
    )?;
    let was_required = existing.is_some_and(|entry| entry.statement().taste.is_some());
    let matched = taste_review.as_ref().filter(|review| review.required());
    if taste_review.is_none() && was_required {
        return Err(environment_fault(
            "task::taste-policy-unavailable",
            "an existing taste review cannot clear without repository policy authority",
        ));
    }
    if let Some(review) = matched
        && existing.is_some_and(|entry| {
            entry.statement().taste.is_none() && entry.workflow().phase.as_str() != "plan"
        })
    {
        return Err(review.refusal(
            task.to_string(),
            "revise the authored task statement so it passes taste review before applying it",
        ));
    }
    let mut taste = None;
    let released = resync_taste_hold(&plan.snapshot, existing, matched, &mut flow, &mut taste)?;
    let initial = flow[0].clone();
    Ok(DocTaskSpec {
        born: task
            .born()
            .expect("document handles carry canonical task stamps"),
        task,
        title: node.title.clone(),
        pitch,
        description: (!node.description.is_empty()).then(|| node.description.clone()),
        annotations: node.annotations.clone(),
        project,
        acceptance: node.acceptance.clone(),
        document_slug: node.slug.clone(),
        priority: node.priority.map(board::priority_from_document),
        taste,
        dependencies,
        flow,
        initial,
        released,
    })
}

fn write_doc_task(
    write: &mut JournalWrite,
    bus: &Bus,
    plan: &DocPlan,
    spec: DocTaskSpec,
    origin: &CreationCause,
    creation: Option<&TaskCreationCapture>,
    previous: Option<PendingFact>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let (task, born) = (spec.task.clone(), spec.born.clone());
    let annotations = spec.annotations.clone();
    let cause = previous.map_or(FactCause::Root, FactCause::Pending);
    let existing = plan
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == &spec.born);
    let written = match existing {
        Some(entry) => edit_doc_task(write, entry, spec, cause),
        None => add_doc_task(write, spec, origin, creation, cause),
    }?;
    append_doc_annotations(write, bus, plan, &task, &born, annotations, written)
}

/// Publish each annotation the task's room does not already hold as an
/// observation there, each under its own publication identity.
fn append_doc_annotations(
    write: &mut JournalWrite,
    bus: &Bus,
    plan: &DocPlan,
    task: &TaskHandle,
    born: &wire::TaskStamp,
    annotations: Vec<String>,
    mut cause: PendingFact,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let mut retained = room::room_publications(bus, born)?
        .iter()
        .filter_map(recorded_note)
        .map(|(_, note)| note)
        .collect::<std::collections::BTreeSet<_>>();
    retained.extend(
        ledger_step_annotations(
            &plan.snapshot.facts,
            &plan.snapshot.answer,
            born,
            &plan.roles,
        )
        .into_iter()
        .map(|(_, annotation)| annotation),
    );
    let mut micros = write.at().get().as_microsecond();
    for note in annotations {
        if !retained.insert(note.clone()) {
            continue;
        }
        let content = room::authored_observation(&note, task)?;
        let publication = room::mint_publication(bus, born, micros)?;
        micros = publication.micros().saturating_add(1);
        cause = write
            .fact(
                wire::rooms::RoomPublished {
                    room: born.clone(),
                    publication,
                    content,
                },
                FactCause::Pending(cause),
            )
            .map_err(|error| store_fault("doc::annotation-build", &error.to_string()))?;
    }
    Ok(cause)
}

fn edit_doc_task(
    write: &mut JournalWrite,
    existing: &Entry,
    spec: DocTaskSpec,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    if !existing.workflow().status.is_pending() {
        return Err(refusal(
            "doc::settled-task",
            Some(spec.task.to_string()),
            "apply authored edits only to pending task statements",
        ));
    }
    let revision = u32::try_from(existing.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let mut statement = existing.statement().clone();
    statement.title = spec.title;
    if statement.pitch.is_empty() {
        statement.pitch = spec.pitch;
    }
    statement.description = spec.description;
    statement.project = spec.project;
    statement.acceptance = spec.acceptance;
    statement.document_slug = Some(spec.document_slug);
    statement.priority = spec.priority;
    statement.taste = spec.taste;
    statement.dependencies = spec.dependencies;
    let dependencies = (statement.dependencies != existing.statement().dependencies)
        .then(|| statement.dependencies.clone());
    let edited = write
        .fact(
            body::TaskEdited {
                intent: None,
                task: existing.born().clone(),
                revision,
                dependencies,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::edit-build", &error.to_string()))?;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    match spec.released {
        Some(released) => released.write(write, existing.born(), edited),
        None => Ok(edited),
    }
}

fn add_doc_task(
    write: &mut JournalWrite,
    spec: DocTaskSpec,
    origin: &CreationCause,
    creation: Option<&TaskCreationCapture>,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let creation = creation.ok_or_else(|| {
        protocol_fault(
            "doc::creation-context-missing",
            "a newly authored document task has no creation-time transcript capture",
        )
    })?;
    let added = write
        .fact(
            body::TaskAdded {
                preparation: None,
                intent: None,
                task: spec.born.clone(),
                title: spec.title.clone(),
                pitch: spec.pitch.clone(),
                description: spec.description.clone(),
                acceptance: spec.acceptance.clone(),
                requirements: wire::EnvironmentRequirements::default(),
                project: spec.project.clone(),
                filing: wire::TaskFiling::Filed,
                priority: spec.priority,
                taste: spec.taste.clone(),
                document_slug: Some(spec.document_slug.clone()),
                taste_proposal: None,
                depends: spec.dependencies.clone(),
                flow: spec.flow.clone(),
                phase: spec.initial.clone(),
                revision: 1,
                creation: creation.context(),
            },
            cause,
        )
        .map_err(|error| store_fault("doc::add-build", &error.to_string()))?;
    let statement_task = spec.born.clone();
    let statement_origin = origin.provenance.clone();
    let statement_creation = creation.context();
    write.task_head(added, move |position| TaskHead {
        preparation: None,
        intent: None,
        born: statement_task,
        queue_origin: position,
        title: spec.title,
        pitch: spec.pitch,
        description: spec.description,
        acceptance: spec.acceptance,
        requirements: wire::EnvironmentRequirements::default(),
        project: spec.project,
        filing: wire::TaskFiling::Filed,
        priority: spec.priority,
        taste: spec.taste,
        document_slug: Some(spec.document_slug),
        taste_proposal: None,
        dependencies: spec.dependencies,
        provenance: statement_origin,
        creation: statement_creation,
        revision: position,
    });
    write.workflow_head(spec.born, added, move |_| WorkflowHead {
        flow: spec.flow,
        phase: spec.initial,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    Ok(added)
}

fn finish_doc_commit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    plan: &DocPlan,
    mut write: JournalWrite,
    previous: Option<PendingFact>,
    operations: u32,
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.fact(
        body::DocApplied {
            digest: apply.preview.clone(),
            barrier: plan.snapshot.cursor,
            operations,
        },
        previous.map_or(FactCause::Root, FactCause::Pending),
    ) {
        return store_fault("doc::apply-build", &error.to_string());
    }
    let answer = wire::intent::DocAnswer {
        preview: apply.preview.clone(),
        touched: plan.touched.clone(),
    };
    if let Err(error) = write.command_result(
        request(command, apply.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("doc::apply-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Doc(answer))
}
