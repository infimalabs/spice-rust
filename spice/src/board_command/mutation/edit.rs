use super::*;

/// Replace an unsettled task statement in place, at a fresh revision.
///
/// A correction is the same statement-head publication `task depends` performs,
/// so the durable record is a new revision rather than an overwrite: the prior
/// text stays readable in the journal and the edit carries the actor that made
/// it. Only fields the caller named move; the rest ride through untouched.
pub(in crate::board_command) fn task_edit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&edit.task),
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &edit.task) else {
        return refusal(
            "task::unknown",
            Some(edit.task.to_string()),
            "read board and name one current task handle",
        );
    };
    // Correcting a row that has already left the board would publish a
    // revision nobody reaches through the working board.
    if !entry.workflow().status.is_pending() {
        return refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "correct a statement only while the task is pending",
        );
    }
    // The OOPS generation names one open opportunistic Design in its fixed
    // project; its statement text amends like any Design's.
    if (edit.intent.is_some() || edit.project.is_some())
        && entry
            .statement()
            .intent
            .is_some_and(wire::TaskIntent::is_opportunistic_design)
    {
        return refusal(
            "task::oops-design-internal",
            Some(entry.handle().to_string()),
            "the OOPS Design keeps its intent and project; amend its text instead",
        );
    }
    let (statement, publication) =
        match prepare_correction(&snapshot, entry, command, edit, repository) {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
    publish_correction(bus, command, edit, statement, publication, &snapshot)
}

fn prepare_correction(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    repository: Option<Host<'_>>,
) -> Result<(TaskHead, CorrectionPublication), Outcome<CommandAnswer>> {
    validate_acceptance(&edit.acceptance)?;
    if let Some(pitch) = &edit.pitch {
        validate_pitch(pitch)?;
    }
    let (mut statement, flow) = corrected_statement(entry, edit)?;
    // A Draft asserts nothing yet; taste policy binds when it is filed.
    if !entry.is_draft() {
        review_taste(repository, snapshot, command, &mut statement)?;
    }
    let at = current_instant()?;
    let requirement_change = requirements::prepare(
        entry,
        command,
        edit,
        &statement.requirements,
        &statement.resources,
        &snapshot.answer.environment(),
        at,
    )?;
    let workflow = flow.map_or_else(
        || entry.workflow().clone(),
        |flow| wire::WorkflowHead {
            phase: flow[0].clone(),
            flow,
            ..entry.workflow().clone()
        },
    );
    let post = Entry::new(
        statement.clone(),
        workflow.clone(),
        entry.claim().cloned(),
        requirement_change
            .lease()
            .cloned()
            .or_else(|| entry.lease().cloned()),
    )
    .map_err(|error| {
        refusal(
            "task::edit-refused",
            Some(error.to_string()),
            "submit a statement the board admits",
        )
    })?
    .with_grouped(entry.grouped())
    .with_design(entry.design());
    planning::refuse_disposed_construction(snapshot, &statement, &workflow)?;
    // A Draft route replaces the workflow it read, so a concurrent workflow
    // change refuses.
    let workflow = if workflow != *entry.workflow() {
        let heads = snapshot
            .answer
            .tasks
            .iter()
            .find(|heads| &heads.statement.body.born == entry.born())
            .expect("validated snapshot entries and answer share task authority");
        Some((workflow, workflow_witness(heads)))
    } else {
        None
    };
    let publication = CorrectionPublication {
        next: next_action(snapshot, &post),
        status: edit_status(entry.statement().taste.as_ref(), statement.taste.as_ref()),
        at,
        requirement_change,
        workflow,
    };
    Ok((statement, publication))
}

/// Read a corrected statement against the taste policy and move its hold.
///
/// A match records the hold, or counts one more recheck of a standing one; a
/// clear read clears it. Only the statement moves: the task's phase, flow,
/// and claim stay exactly as they were.
fn review_taste(
    repository: Option<Host<'_>>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    let review = task_taste_review(
        repository,
        snapshot,
        command.actor(),
        &statement.title,
        &statement.pitch,
        statement.description.as_deref(),
        &statement.acceptance,
    )?;
    resync_taste_hold(&mut statement.taste, review.as_ref())
}

/// The edit answer's status, naming where the edit left the taste hold.
fn edit_status(before: Option<&wire::TasteHold>, after: Option<&wire::TasteHold>) -> String {
    match (before, after) {
        (_, Some(_)) => "edited; taste_review required".to_owned(),
        (Some(_), None) => "edited; taste_review cleared".to_owned(),
        (None, None) => "edited".to_owned(),
    }
}

/// Build the statement a correction publishes, or the refusal it earns.
///
/// Only fields the caller named move; the rest ride through from the filed
/// statement untouched. A field this refuses is refused before anything is
/// written, so a correction that names two fields and gets one wrong publishes
/// neither. A Draft's replacement route returns beside its statement.
fn corrected_statement(
    entry: &Entry,
    edit: &TaskEdit,
) -> Result<(TaskHead, Option<Vec<wire::Phase>>), Outcome<CommandAnswer>> {
    if edit.title.is_none()
        && edit.pitch.is_none()
        && edit.description.is_none()
        && edit.acceptance.is_empty()
        && edit.requirements.is_none()
        && edit.intent.is_none()
        && edit.priority.is_none()
        && edit.project.is_none()
        && edit.flow.is_none()
    {
        return Err(refusal(
            "task::edit-empty",
            Some(edit.task.to_string()),
            "name at least one statement field, an environmental requirement edit, an intent edit, or a Draft flow",
        ));
    }
    let flow = edit
        .flow
        .as_deref()
        .map(|flow| corrected_flow(entry, flow))
        .transpose()?;
    let mut statement = entry.statement().clone();
    if let Some(title) = &edit.title {
        statement.title = title.clone();
    }
    if let Some(pitch) = &edit.pitch {
        statement.pitch = pitch.trim().to_owned();
    }
    if let Some(description) = &edit.description {
        statement.description = (!description.is_empty()).then(|| description.clone());
    }
    if !edit.acceptance.is_empty() {
        statement.acceptance = edit.acceptance.clone();
    }
    if let Some(requirements) = &edit.requirements {
        let authored = requirements.requirements();
        statement.resources = authored.compiled_resources();
        statement.requirements = authored.without_resources();
    }
    if let Some(intent) = edit.intent {
        if statement.preparation.is_some() {
            return Err(refusal(
                "task::preparation-intent-fixed",
                Some(entry.handle().to_string()),
                "an associated preparation or controlled output keeps the intent it was created with",
            ));
        }
        // A Draft has not declared any purpose yet; its author may revise it.
        if statement.intent != intent.intent() && statement.intent.is_some() && !entry.is_draft() {
            return Err(refusal(
                "task::planning-intent-stable",
                Some(entry.handle().to_string()),
                "retain the declared planning purpose through construction and review",
            ));
        }
        statement.intent = intent.intent();
    }
    if let Some(priority) = &edit.priority {
        statement.priority = Some(declared_priority(priority)?);
    }
    if let Some(requested) = &edit.project {
        statement.project = corrected_project(requested)?;
    }
    // A Design walks one phase: judge the pair this edit leaves, so an intent
    // and a Draft route edited together are checked as they will stand.
    if edit.intent.is_some() || flow.is_some() {
        planning::admit_design_flow(
            statement.intent,
            flow.as_deref().unwrap_or(&entry.workflow().flow),
        )?;
    }
    Ok((statement, flow))
}

/// A Draft's replacement route; filed work keeps the route it was filed with.
fn corrected_flow(entry: &Entry, flow: &str) -> Result<Vec<wire::Phase>, Outcome<CommandAnswer>> {
    if !entry.is_draft() {
        return Err(refusal(
            "task::flow-fixed",
            Some(filing_state(entry)),
            "a route moves only while the task is a Draft; filed work keeps its flow",
        ));
    }
    phases(Some(flow))
}

fn declared_priority(priority: &str) -> Result<wire::TaskPriority, Outcome<CommandAnswer>> {
    board::priority_from_authored(priority).ok_or_else(|| {
        refusal(
            "task::edit-priority-unknown",
            Some(priority.to_owned()),
            "name one of critical, high, medium, low, or none, or the letter the board prints",
        )
    })
}

fn corrected_project(requested: &str) -> Result<String, Outcome<CommandAnswer>> {
    let project = board::Project::admit(requested).map_err(|error| {
        refusal(
            "task::edit-project-refused",
            Some(error.to_string()),
            PROJECT_REPAIR,
        )
    })?;
    Ok(project.as_str().to_owned())
}

/// Write one corrected statement as a fresh revision attributed to its editor.
struct CorrectionPublication {
    next: Option<Box<str>>,
    /// The answer's status: the edit, plus how it moved the taste hold.
    status: String,
    at: Instant,
    requirement_change: requirements::Change,
    /// The replacement workflow a Draft's route edit publishes, with the
    /// workflow witness it replaces.
    workflow: Option<(wire::WorkflowHead, Witness)>,
}

fn publish_correction(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    statement: TaskHead,
    publication: CorrectionPublication,
    snapshot: &ValidatedSnapshot,
) -> Outcome<CommandAnswer> {
    let CorrectionPublication {
        next,
        status,
        at,
        requirement_change,
        workflow,
    } = publication;
    let revision = u32::try_from(statement.revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let origin = match task_origin(&statement.born) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    requirement_change.guard(&mut write);
    let edited = match write.fact(
        body::TaskEdited {
            intent: edit.intent,
            task: statement.born.clone(),
            revision,
            dependencies: None,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::edit-build", &error.to_string()),
    };
    if let Some((workflow, read)) = workflow {
        guard(&mut write, vec![read]);
        write.workflow_head(statement.born.clone(), edited, move |_| workflow);
    }
    let rendered = TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
    let taste = answered_taste(command.actor(), statement.taste.as_ref());
    let completion_contract = Some(
        completion_contract(
            &statement.acceptance,
            &statement.born,
            &statement.dependencies,
            statement.intent,
            snapshot.entries.iter(),
        )
        .into_boxed_str(),
    );
    write.task_head(edited, move |position| {
        published_statement(
            TaskHead {
                revision: position,
                ..statement
            },
            position,
        )
    });
    requirement_change.write_lease(&mut write, edited);
    let answer = wire::intent::TaskAnswer {
        taste,
        completion_contract,
        task: rendered,
        status,
        completion: None,
        claim: None,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, edit.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::edit-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}
