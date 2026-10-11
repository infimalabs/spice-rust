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
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&edit.task),
        projects,
        allocation,
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
    // A withdrawn task is settled exactly as a completed one is, and both stamp
    // the same instant. Correcting a row that has already left the board would
    // publish a revision nobody reaches through the working board.
    if entry.workflow().completion.is_some() {
        return refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "correct a statement only while the task is pending",
        );
    }
    let (statement, publication) =
        match prepare_correction(&snapshot, entry, command, edit, repository, projects) {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
    publish_correction(bus, command, edit, statement, publication)
}

fn prepare_correction(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Result<(TaskHead, CorrectionPublication), Outcome<CommandAnswer>> {
    validate_acceptance(&edit.acceptance)?;
    let mut statement = corrected_statement(entry, edit, projects)?;
    let (taste_review, was_required, released) =
        review_taste(repository, snapshot, command, edit, entry, &mut statement)?;
    let at = current_instant()?;
    let requirement_change = requirements::prepare(
        entry,
        command,
        edit,
        &statement.requirements,
        &snapshot.answer.environment(),
        at,
    )?;
    let post = Entry::new(
        statement.clone(),
        released.as_ref().map_or_else(
            || entry.workflow().clone(),
            |released| released.workflow().clone(),
        ),
        entry.claim().cloned(),
        requirement_change
            .lease()
            .cloned()
            .or_else(|| entry.lease().cloned()),
        projects,
    )
    .map_err(|error| {
        refusal(
            "task::edit-refused",
            Some(error.to_string()),
            "submit a statement the board admits",
        )
    })?;
    let publication = CorrectionPublication {
        next: next_action(snapshot, &post),
        at,
        requirement_change,
        taste_review: taste_review.filter(|review| was_required || review.required()),
        released,
    };
    Ok((statement, publication))
}

fn review_taste(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    entry: &Entry,
    statement: &mut TaskHead,
) -> Result<(Option<TasteReview>, bool, Option<ReleasedPlan>), Outcome<CommandAnswer>> {
    let review = task_taste_review(
        repository,
        snapshot,
        command.actor(),
        &statement.title,
        statement.description.as_deref(),
        &statement.acceptance,
    )?;
    let was_required = has_taste_review(&statement.tags);
    let mut released = None;
    match review.as_ref().map(TasteReview::required) {
        Some(false) => {
            if let Some(workflow) = clear_taste_review(&mut statement.tags, entry.workflow()) {
                released = Some(ReleasedPlan::observe(snapshot, entry.born(), workflow)?);
            }
        }
        Some(true) if was_required || entry.workflow().phase.as_str() == "plan" => {
            require_taste_review(&mut statement.tags);
        }
        Some(true) => {
            return Err(refusal(
                "task::taste-review-required",
                Some(edit.task.to_string()),
                "revise the proposed task statement so it passes taste review before applying it",
            ));
        }
        None if was_required => {
            return Err(environment_fault(
                "task::taste-policy-unavailable",
                "an existing taste review cannot clear without repository policy authority",
            ));
        }
        None => {}
    }
    Ok((review, was_required, released))
}

/// Build the statement a correction publishes, or the refusal it earns.
///
/// Only fields the caller named move; the rest ride through from the filed
/// statement untouched. A field this refuses is refused before anything is
/// written, so a correction that names two fields and gets one wrong publishes
/// neither.
fn corrected_statement(
    entry: &Entry,
    edit: &TaskEdit,
    projects: &ProjectPolicy,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    if edit.title.is_none()
        && edit.description.is_none()
        && edit.acceptance.is_empty()
        && edit.requirements.is_none()
        && edit.priority.is_none()
        && edit.project.is_none()
    {
        return Err(refusal(
            "task::edit-empty",
            Some(edit.task.to_string()),
            "name at least one statement field or an environmental requirement edit",
        ));
    }
    let mut statement = entry.statement().clone();
    if let Some(title) = &edit.title {
        statement.title = title.clone();
    }
    if let Some(description) = &edit.description {
        statement.description = Some(description.clone());
    }
    if !edit.acceptance.is_empty() {
        statement.acceptance = edit.acceptance.clone();
    }
    if let Some(requirements) = &edit.requirements {
        statement.requirements = requirements.requirements();
    }
    if let Some(priority) = &edit.priority {
        correct_priority(&mut statement.tags, priority)?;
    }
    if let Some(requested) = &edit.project {
        statement.project = corrected_project(entry, projects, requested)?;
    }
    Ok(statement)
}

fn correct_priority(tags: &mut Vec<String>, priority: &str) -> Result<(), Outcome<CommandAnswer>> {
    // Refuse rather than store a priority the allocator would silently ignore.
    let Some(declared) = board::priority_from_authored(priority) else {
        return Err(refusal(
            "task::edit-priority-unknown",
            Some(priority.to_owned()),
            "name one of critical, high, medium, low, or none, or the letter the board prints",
        ));
    };
    tags.retain(|tag| !tag.starts_with(board::PRIORITY_TAG_PREFIX));
    tags.push(board::priority_tag(declared));
    Ok(())
}

fn corrected_project(
    entry: &Entry,
    projects: &ProjectPolicy,
    requested: &str,
) -> Result<String, Outcome<CommandAnswer>> {
    if is_oops(entry) {
        return Err(refusal(
            "task::promotion-required",
            Some(oops_state(entry)),
            &format!(
                "promote the parked row with spice task wake {} --into {requested}",
                entry.handle(),
            ),
        ));
    }
    // The destination goes through the same public creation door as `task add`.
    let project = projects.admit_manual_creation(requested).map_err(|error| {
        let reason = if matches!(error, ProjectError::SystemReserved(_)) {
            "task::project-system-reserved"
        } else {
            "task::edit-project-refused"
        };
        refusal(
            reason,
            Some(error.to_string()),
            "name a public project this board admits for manual filing",
        )
    })?;
    Ok(project.as_str().to_owned())
}

/// Write one corrected statement as a fresh revision attributed to its editor.
struct CorrectionPublication {
    next: Option<Box<str>>,
    at: Instant,
    requirement_change: requirements::Change,
    taste_review: Option<TasteReview>,
    /// The authored workflow a cleared hold returns the task to.
    released: Option<ReleasedPlan>,
}

fn publish_correction(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    statement: TaskHead,
    publication: CorrectionPublication,
) -> Outcome<CommandAnswer> {
    let CorrectionPublication {
        next,
        at,
        requirement_change,
        taste_review,
        released,
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
            task: statement.born.clone(),
            revision,
            dependencies: None,
            notice: None,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::edit-build", &error.to_string()),
    };
    if let Some(review) = taste_review.as_ref()
        && let Err(outcome) = append_taste_review_audit(
            &mut write,
            &statement.born,
            review,
            FactCause::Pending(edited),
        )
    {
        return outcome;
    }
    if let Some(released) = released
        && let Err(outcome) = released.write(&mut write, &statement.born, edited)
    {
        return outcome;
    }
    let rendered = TaskHandle::for_project(&statement.project, &statement.born);
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    requirement_change.write_lease(&mut write, edited);
    let answer = wire::intent::TaskAnswer {
        task: rendered,
        status: "edited".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        claim: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, edit.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::edit-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}
