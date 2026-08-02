fn claim_free(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskRef,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == task.task)
    else {
        return refusal(
            "task::unknown",
            Some(task.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let Some(claim) = entry.claim() else {
        return refusal(
            "claim::head-missing",
            Some(task.task.to_string()),
            "free only a task carrying an active claim",
        );
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *command.actor() {
        return refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "free the task through the capability that currently owns it",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&task.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let freed = match write.fact(
        body::ClaimFreed {
            task: task.task.clone(),
            actor: command.actor().clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("claim::free-build", &error.to_string()),
    };
    let mut next = claim.clone();
    next.disposition = ClaimDisposition::Freed;
    write.claim_head(task.task.clone(), freed, move |fact| ClaimHead {
        fact,
        ..next
    });
    let answer = wire::intent::ClaimAnswer {
        task: task.task.clone(),
        holder: command.actor().clone(),
    };
    if let Err(error) = write.command_result(
        request(command, task.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::free-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

fn claim_reclaim(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match reclaim_entry(&snapshot, renew) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let (at, until, lease) = match reclaim_window(entry, &snapshot, command, renew) {
        Ok(window) => window,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&renew.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::BusTimer);
    guard(&mut write, command.expected().to_vec());
    let renewed = match write.fact(
        body::ClaimRenewed {
            task: renew.task.clone(),
            epoch: command.epoch(),
            until,
            maximum: lease.maximum,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("claim::reclaim-build", &error.to_string()),
    };
    let mut next = lease.clone();
    next.renewal = at;
    next.expiry = until;
    write.lease_head(renew.task.clone(), renewed, move |_| next);
    let answer = wire::intent::ClaimAnswer {
        task: renew.task.clone(),
        holder: command.actor().clone(),
    };
    if let Err(error) = write.command_result(
        request(command, renew.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::reclaim-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

fn reclaim_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    renew: &ClaimRenew,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == renew.task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(renew.task.to_string()),
                "read board and reclaim one current task handle",
            )
        })
}

fn reclaim_window(
    entry: &Entry,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
) -> Result<(Instant, Instant, LeaseHead), Outcome<CommandAnswer>> {
    let claim = entry.claim().ok_or_else(|| {
        refusal(
            "claim::head-missing",
            Some(renew.task.to_string()),
            "reclaim only the current lane's active task",
        )
    })?;
    let lease = entry.lease().ok_or_else(|| {
        protocol_fault(
            "claim::lease-missing",
            "an active claim head has no durable lease head",
        )
    })?;
    let Actor::Lane(lane) = command.actor() else {
        return Err(refusal(
            "claim::lane-required",
            Some(command.actor().to_string()),
            "reclaim through the lane capability that owns the claim",
        ));
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *command.actor() {
        return Err(refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
            "reclaim only an active claim owned by this lane capability",
        ));
    }
    let at = current_instant()?;
    board::Active::admit(entry, command.actor(), at).map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            "reclaim before lapse; use task next after reallocation",
        )
    })?;
    admit_reclaim_residency(snapshot, command, renew, claim, lease, *lane)?;
    let seconds = i64::try_from(renew.lease_seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            refusal(
                "claim::lease-seconds-invalid",
                Some(renew.lease_seconds.to_string()),
                "request a positive representable lease duration",
            )
        })?;
    let until = std::cmp::min(instant_after(at, seconds)?, lease.maximum);
    if until <= lease.expiry {
        return Err(refusal(
            "claim::lease-nonextending",
            Some(lease.expiry.to_string()),
            "request an extension before the durable maximum",
        ));
    }
    Ok((at, until, lease.clone()))
}

fn admit_reclaim_residency(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    claim: &ClaimHead,
    lease: &LeaseHead,
    lane: wire::CanonicalUuid,
) -> Result<(), Outcome<CommandAnswer>> {
    let residency = &snapshot.answer.residency.body;
    let seated = residency.lanes.iter().find(|seat| seat.lane == lane);
    let exact = claim.lane == lane
        && claim.epoch == command.epoch()
        && lease.epoch == command.epoch()
        && residency.epoch == command.epoch()
        && seated.is_some_and(|seat| {
            seat.disposition == "occupied" && seat.occupant == Some(claim.occupant)
        });
    if exact {
        return Ok(());
    }
    Err(refusal(
        "claim::residency-moved",
        Some(renew.task.to_string()),
        "read current lane residency and reclaim its exact active claim",
    ))
}
fn task_done(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    done: &TaskDone,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(
        bus,
        command.actor().clone(),
        projects,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match done_entry(&snapshot, done) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let (at, transition) = match done_transition(
        entry,
        command,
    ) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&done.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let git = match publish_task_boundary(
        repository,
        &snapshot,
        command,
        entry,
    ) {
        Ok(git) => git,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin,
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    let answer = match append_done(
        &mut write,
        done,
        entry,
        &transition,
        git,
    ) {
        Ok(answer) => answer,
        Err(outcome) => return outcome,
    };
    if let Err(error) = write.command_result(
        request(command, done.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault(
            "task::done-receipt",
            &error.to_string(),
        );
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn done_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    done: &TaskDone,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == done.task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(done.task.to_string()),
                concat!(
                    "read board and complete the task held by ",
                    "this capability",
                ),
            )
        })
}

fn done_transition(
    entry: &Entry,
    command: &wire::AdmittedCommand,
) -> Result<(Instant, board::Transition), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let active = board::Active::admit(
        entry,
        command.actor(),
        at,
    )
    .map_err(|error| {
        refusal(
            "claim::active-refused",
            Some(error.to_string()),
            concat!(
                "renew or win a current claim before ",
                "completing its phase",
            ),
        )
    })?;
    if active.epoch() != command.epoch() {
        return Err(refusal(
            "claim::epoch-moved",
            Some(active.epoch().to_string()),
            concat!(
                "win the task again under the current resident ",
                "generation",
            ),
        ));
    }
    let current = entry
        .workflow()
        .flow
        .iter()
        .position(|phase| phase == &entry.workflow().phase)
        .expect("validated workflow contains its phase");
    let transition = match entry.workflow().flow.get(current + 1) {
        Some(next) => {
            board::Transition::advance(entry, &active, next.clone())
        }
        None => board::Transition::complete(entry, &active, at),
    }
    .map_err(|error| {
        refusal(
            "task::phase-refused",
            Some(error.to_string()),
            "complete only the currently held workflow phase",
        )
    })?;
    Ok((at, transition))
}

fn append_done(
    write: &mut JournalWrite,
    done: &TaskDone,
    entry: &Entry,
    transition: &board::Transition,
    git: Option<GitBoundary>,
) -> Result<wire::intent::TaskAnswer, Outcome<CommandAnswer>> {
    let completed = write
        .fact(
            body::TaskDone {
                task: done.task.clone(),
                validation: done.validation.clone(),
                git,
            },
            FactCause::Root,
        )
        .map_err(|error| {
            store_fault("task::done-build", &error.to_string())
        })?;
    let workflow_fact = match transition.kind() {
        board::TransitionKind::Advance { to, .. } => write
            .fact(
                body::TaskPhased {
                    task: done.task.clone(),
                    phase: to.clone(),
                },
                FactCause::Pending(completed),
            )
            .map_err(|error| {
                store_fault(
                    "task::phase-build",
                    &error.to_string(),
                )
            })?,
        board::TransitionKind::Complete { .. } => completed,
    };
    let workflow = transition.next().clone();
    write.workflow_head(
        done.task.clone(),
        workflow_fact,
        move |_| workflow,
    );
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(
        done.task.clone(),
        completed,
        move |fact| ClaimHead { fact, ..claim },
    );
    Ok(wire::intent::TaskAnswer {
        task: done.task.clone(),
        status: transition.next().status.clone(),
    })
}

fn task_review(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    review: &TaskReview,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == review.task)
    else {
        return refusal(
            "task::unknown",
            Some(review.task.to_string()),
            "read board and name one current review task",
        );
    };
    if entry.workflow().phase.as_str() != "review"
        || entry.workflow().assignment.as_ref() != Some(command.actor())
    {
        return refusal(
            "review::assignment-refused",
            Some(review.task.to_string()),
            "win the review phase with task next before recording its verdict",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let active = match board::Active::admit(entry, command.actor(), at) {
        Ok(active) => active,
        Err(error) => {
            return refusal(
                "claim::active-refused",
                Some(error.to_string()),
                "win a current review claim before recording its verdict",
            );
        }
    };
    let transition = match board::Transition::complete(entry, &active, at) {
        Ok(transition) => transition,
        Err(error) => {
            return refusal(
                "review::phase-refused",
                Some(error.to_string()),
                "review only the final assigned workflow phase",
            );
        }
    };
    let verdict = if review.finding.trim().is_empty() {
        body::Verdict::Clean
    } else {
        body::Verdict::Finding
    };
    let finding = (verdict == body::Verdict::Finding).then(|| match &review.note {
        Some(note) if !note.trim().is_empty() => format!("{}\n\n{note}", review.finding),
        _ => review.finding.clone(),
    });
    let origin = match task_origin(&review.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let git = match publish_task_boundary(repository, &snapshot, command, entry) {
        Ok(git) => git,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let reviewed = match write.fact(
        body::TaskReviewed {
            task: review.task.clone(),
            reviewer: command.actor().clone(),
            verdict,
            finding,
            git,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("review::build", &error.to_string()),
    };
    let mut workflow = transition.next().clone();
    workflow.review = match verdict {
        body::Verdict::Clean => "clean",
        body::Verdict::Finding => "finding",
    }
    .to_owned();
    write.workflow_head(review.task.clone(), reviewed, move |_| workflow);
    let mut claim = entry
        .claim()
        .expect("Active proof requires a claim head")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(review.task.clone(), reviewed, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let answer = wire::intent::TaskAnswer {
        task: review.task.clone(),
        status: "completed".to_owned(),
    };
    if let Err(error) = write.command_result(
        request(command, review.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("review::receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

struct DocPlan {
    preview: board::Preview,
    snapshot: ValidatedSnapshot,
    touched: Vec<TaskHandle>,
    handles: std::collections::BTreeMap<usize, TaskHandle>,
}

struct DocTaskSpec {
    task: TaskHandle,
    title: String,
    project: String,
    tags: Vec<String>,
    dependencies: Vec<TaskHandle>,
    flow: Vec<wire::Phase>,
    initial: wire::Phase,
}

fn doc_apply(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let plan = match validate_doc_plan(bus, command, apply, projects) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    if let Some(outcome) = repeated_doc_apply(bus, apply, &plan.touched) {
        return outcome;
    }
    commit_doc_plan(bus, command, apply, &plan)
}

fn validate_doc_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    apply: &DocApply,
    projects: &ProjectPolicy,
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
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects)?;
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
    let touched = document_handles(preview.document());
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
    let preview = command.expected().iter().any(
        |witness| matches!(witness, Witness::Preview { digest } if digest == &apply.preview),
    );
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
    let facts = bus.fact_snapshot().ok()?;
    let existing = facts.facts().iter().find(|envelope| {
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
    plan: &DocPlan,
) -> Outcome<CommandAnswer> {
    let origin = match task_origin(&plan.touched[0]) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    let mut previous = None;
    let mut operations = 0_u32;
    for node in plan
        .preview
        .document()
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
    {
        let spec = doc_task_spec(plan, node);
        previous = match write_doc_task(&mut write, plan, spec, &origin, previous) {
            Ok(fact) => Some(fact),
            Err(outcome) => return outcome,
        };
        operations = operations.saturating_add(1);
    }
    finish_doc_commit(bus, command, apply, plan, write, previous, operations)
}

fn doc_task_spec(plan: &DocPlan, node: &dialect::Node) -> DocTaskSpec {
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
        .filter_map(|edge| plan.handles.get(&edge.target).cloned())
        .collect();
    let project = plan
        .preview
        .document()
        .project
        .clone()
        .unwrap_or_else(|| "task.document".to_owned());
    let flow = if node.flow.is_empty() {
        vec![phase("todo"), phase("review")]
    } else {
        node.flow
            .iter()
            .map(|station| phase(station.as_str()))
            .collect()
    };
    let initial = flow[0].clone();
    let mut tags = node.tags.clone();
    if let Some(priority) = node.priority {
        tags.push(format!("priority:{priority}"));
    }
    DocTaskSpec {
        task,
        title: node.title.clone(),
        project,
        tags,
        dependencies,
        flow,
        initial,
    }
}

fn write_doc_task(
    write: &mut JournalWrite,
    plan: &DocPlan,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    previous: Option<PendingFact>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let cause = previous.map_or(FactCause::Root, FactCause::Pending);
    let existing = plan
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == spec.task);
    match existing {
        Some(entry) => edit_doc_task(write, entry, spec, cause),
        None => add_doc_task(write, spec, origin, cause),
    }
}

fn edit_doc_task(
    write: &mut JournalWrite,
    existing: &Entry,
    spec: DocTaskSpec,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    if existing.workflow().status == "completed" {
        return Err(refusal(
            "doc::settled-task",
            Some(spec.task.to_string()),
            "apply authored edits only to pending task statements",
        ));
    }
    let revision = u32::try_from(existing.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                task: spec.task,
                revision,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::edit-build", &error.to_string()))?;
    let mut statement = existing.statement().clone();
    statement.title = spec.title;
    statement.project = spec.project;
    statement.tags = spec.tags;
    statement.dependencies = spec.dependencies;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    Ok(edited)
}

fn add_doc_task(
    write: &mut JournalWrite,
    spec: DocTaskSpec,
    origin: &wire::Origin,
    cause: FactCause,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    let identity = wire::CanonicalUuid::new(uuid::Uuid::new_v4());
    let added = write
        .fact(
            body::TaskAdded {
                task: spec.task.clone(),
                uuid: identity,
                title: spec.title.clone(),
                project: spec.project.clone(),
                tags: spec.tags.clone(),
                depends: spec.dependencies.clone(),
                flow: spec.flow.clone(),
                phase: spec.initial.clone(),
                revision: 1,
            },
            cause,
        )
        .map_err(|error| store_fault("doc::add-build", &error.to_string()))?;
    let statement_task = spec.task.clone();
    let statement_origin = origin.clone();
    write.task_head(added, move |position| TaskHead {
        handle: statement_task,
        uuid: identity,
        title: spec.title,
        project: spec.project,
        tags: spec.tags,
        dependencies: spec.dependencies,
        provenance: statement_origin,
        revision: position,
    });
    write.workflow_head(spec.task, added, move |_| WorkflowHead {
        flow: spec.flow,
        phase: spec.initial,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
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
