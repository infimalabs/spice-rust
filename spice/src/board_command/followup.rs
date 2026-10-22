use super::*;

// The follow-up tracking a review carries beside its verdict.
//
// The predecessor refuses an unclean review that names no follow-up, so a
// finding never lands with nothing tracking the change it asked for, and it
// reads an absent finding and the literal word clean as the same verdict. The
// second rule matters because the predecessor's own allocator hint emits
// --finding clean, which a presence test files as a finding named after the
// verdict it meant to record.

/// Fields one `--then` record may carry.
const FOLLOWUP_FIELDS: &str = "title, project, description, acceptance, flow or intent";

/// The `--then` fields that accrue rather than refusing a second value.
///
/// The predecessor also keeps `after` here, which this record has no place to
/// put: a correction's ordering is owned by the reviewed task.
const FOLLOWUP_REPEATS: &[&str] = &["acceptance", "flow"];

/// Whether a finding records a clean review rather than a requested change.
pub(super) fn review_is_clean(finding: &str) -> bool {
    let finding = finding.trim();
    finding.is_empty() || finding.eq_ignore_ascii_case("clean")
}

/// Parse one `--then` record into the statement it creates.
fn parse_followup(spec: &str) -> Result<TaskAdd, String> {
    let mut add = TaskAdd {
        preparation: None,
        intent: None,
        title: String::new(),
        pitch: String::new(),
        project: None,
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: None,
        flow: None,
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let mut seen: Vec<&str> = Vec::new();
    for field in spec.split('|') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let Some((key, value)) = field.split_once('=') else {
            return Err(format!("follow-up field {field:?} is not key=value"));
        };
        let value = value.trim().to_owned();
        let key = key.trim();
        if !FOLLOWUP_REPEATS.contains(&key) {
            if seen.contains(&key) {
                return Err(format!(
                    "follow-up field {key:?} is given twice; the repeatable fields are \
                     {}",
                    FOLLOWUP_REPEATS.join(" and ")
                ));
            }
            seen.push(key);
        }
        match key {
            "title" => add.title = value,
            "project" => add.project = Some(value),
            "description" => add.description = Some(value),
            "acceptance" => add.acceptance.push(value),
            "intent" => add.intent = Some(value.parse()?),
            // The predecessor accrues a repeated flow= into one phase route,
            // and the route this carries is the same comma-separated form.
            "flow" => {
                add.flow = Some(match add.flow.take() {
                    Some(prior) if !prior.is_empty() => format!("{prior},{value}"),
                    _ => value,
                });
            }
            other => {
                return Err(format!(
                    "unknown follow-up field {other:?}; name one of {FOLLOWUP_FIELDS}"
                ));
            }
        }
    }
    if add.title.trim().is_empty() {
        return Err(format!("follow-up {spec:?} carries no title=..."));
    }
    // A review follow-up has no author to ask for a pitch, so it derives one
    // once from the first description sentence, as a friction report does.
    add.pitch = wire::pitch::derive(&add.title, add.description.as_deref());
    Ok(add)
}

/// Mint one birth stamp per `--then` record and witness each intended absence.
///
/// Every record is parsed before the first stamp is minted, so a malformed
/// record later in the batch cannot leave earlier ones half-planned.
pub(super) fn plan_followups(
    board: &BoardAnswer,
    matches: &ArgMatches,
    expected: &mut Vec<Witness>,
) -> Result<Vec<ReviewFollowup>, Outcome<CommandAnswer>> {
    let mut parsed = Vec::new();
    for spec in matches.get_many::<String>("then").into_iter().flatten() {
        let add = parse_followup(spec)
            .map_err(|error| refusal("review::followup-invalid", Some(spec.clone()), &error))?;
        parsed.push(add);
    }
    let born = allocate_task_stamps(board, jiff::Timestamp::now().as_microsecond(), parsed.len())?;
    let mut planned: Vec<ReviewFollowup> = Vec::new();
    for (add, task) in parsed.into_iter().zip(born) {
        expected.push(Witness::Absence { task: task.clone() });
        planned.push(ReviewFollowup { task, add });
    }
    Ok(planned)
}

/// Resolve every `--followup` handle and witness the reviewed edge it will gain.
pub(super) fn plan_followup_edges(
    board: &BoardAnswer,
    matches: &ArgMatches,
    task: &TaskHandle,
    expected: &mut Vec<Witness>,
) -> Result<Vec<TaskHandle>, Outcome<CommandAnswer>> {
    let mut linked = Vec::new();
    let reviewed = task_heads(board, task)?;
    for written in matches.get_many::<String>("followup").into_iter().flatten() {
        let handle = written.parse::<TaskHandle>().map_err(|error| {
            refusal(
                "task::handle-invalid",
                Some(written.clone()),
                &error.to_string(),
            )
        })?;
        let heads = named_task_heads(board, &handle)?;
        let handle = heads.statement.body.handle.clone();
        if linked.contains(&handle) {
            continue;
        }
        expected.push(statement_witness(heads));
        expected.push(workflow_witness(heads));
        if !reviewed
            .statement
            .body
            .dependencies
            .contains(&heads.statement.body.born)
        {
            expected.push(Witness::Edge {
                task: reviewed.statement.body.born.clone(),
                after: heads.statement.body.born.clone(),
            });
            append_dependency_reachability_witnesses(
                board,
                std::slice::from_ref(&heads.statement.body.born),
                expected,
            )?;
        }
        linked.push(handle);
    }
    Ok(linked)
}

/// Validate and project the dependencies one finding leaves on the reviewed task.
pub(super) fn correction_statement(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
    review: &TaskReview,
    projects: &ProjectPolicy,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    let mut statement = entry.statement().clone();
    for followup in &review.then {
        require_absence_witness(command, &followup.task)?;
        if !statement.dependencies.contains(&followup.task) {
            statement.dependencies.push(followup.task.clone());
        }
    }
    let mut seen = Vec::new();
    for handle in &review.followup {
        let correction = task_entry(&snapshot.entries, handle).ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(handle.to_string()),
                "name an existing task as the review correction",
            )
        })?;
        if seen.contains(&correction.born()) {
            continue;
        }
        seen.push(correction.born());
        require_correction_witnesses(snapshot, command, handle)?;
        if let Some(required) =
            planning::correction_preparation(task_heads(&snapshot.answer, entry.handle())?)
        {
            if correction.statement().intent != entry.statement().intent
                || correction
                    .statement()
                    .preparation
                    .as_ref()
                    .is_none_or(|preparation| {
                        preparation.controller != required.controller
                            || preparation.construction != required.construction
                    })
            {
                return Err(refusal(
                    "review::planning-correction-context",
                    Some(correction.handle().to_string()),
                    "use a correction carrying the same construction owner, or create it with --then",
                ));
            }
        }
        let already_related = statement.dependencies.contains(correction.born());
        if correction.workflow().status.is_completed() && !already_related {
            return Err(refusal(
                "task::settled",
                Some(correction.handle().to_string()),
                "track a review finding with a pending correction task",
            ));
        }
        if !already_related {
            require_correction_edge(command, entry.born(), correction.born())?;
            require_acyclic_correction(snapshot, command, correction, entry.born())?;
            statement.dependencies.push(correction.born().clone());
        }
    }
    Entry::new(
        statement.clone(),
        entry.workflow().clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    )
    .map_err(|error| {
        refusal(
            "task::dependency-refused",
            Some(error.to_string()),
            "submit distinct correction tasks that keep the review graph acyclic",
        )
    })?;
    Ok(statement)
}

fn require_absence_witness(
    command: &wire::AdmittedCommand,
    task: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    command
        .expected()
        .contains(&Witness::Absence { task: task.clone() })
        .then_some(())
        .ok_or_else(|| {
            protocol_fault(
                "review::followup-witness-invalid",
                "each new correction requires its own intended absent birth stamp",
            )
        })
}

fn require_correction_witnesses(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    correction: &TaskHandle,
) -> Result<(), Outcome<CommandAnswer>> {
    let heads = task_heads(&snapshot.answer, correction)?;
    let statement = statement_witness(heads);
    let workflow = workflow_witness(heads);
    if command.expected().contains(&statement) && command.expected().contains(&workflow) {
        Ok(())
    } else {
        Err(protocol_fault(
            "review::followup-witness-invalid",
            "each existing correction requires its current statement and workflow witnesses",
        ))
    }
}

fn require_correction_edge(
    command: &wire::AdmittedCommand,
    task: &TaskStamp,
    after: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    command
        .expected()
        .contains(&Witness::Edge {
            task: task.clone(),
            after: after.clone(),
        })
        .then_some(())
        .ok_or_else(|| {
            protocol_fault(
                "review::followup-witness-invalid",
                "each new correction edge requires its current absence witness",
            )
        })
}

fn require_acyclic_correction(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    correction: &Entry,
    reviewed: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    let mut pending = vec![correction.born().clone()];
    let mut visited = Vec::new();
    while let Some(task) = pending.pop() {
        if &task == reviewed {
            return Err(refusal(
                "task::dependency-refused",
                Some(format!("{reviewed} after {}", correction.handle())),
                "name corrections that do not already depend on the reviewed task",
            ));
        }
        if visited.contains(&task) {
            continue;
        }
        visited.push(task.clone());
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.born() == &task)
            .ok_or_else(|| {
                protocol_fault(
                    "task::board-incoherent",
                    "a correction dependency has no current board entry",
                )
            })?;
        let witness = Witness::Task {
            task: task.clone(),
            head: entry.statement().revision,
        };
        if !command.expected().contains(&witness) {
            return Err(protocol_fault(
                "review::followup-witness-invalid",
                "correction reachability requires every contributing statement witness",
            ));
        }
        pending.extend(entry.statement().dependencies.iter().cloned());
    }
    Ok(())
}

/// Create each new correction in the same transaction as its finding.
#[allow(
    clippy::too_many_arguments,
    reason = "review follow-ups retain the enclosing transaction's exact authority inputs"
)]
pub(super) fn record_followups(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
    review: &TaskReview,
    reviewed: PendingFact,
    origin: &CreationCause,
    creation: Option<&TaskCreationCapture>,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Result<(), Outcome<CommandAnswer>> {
    let reviewed_task = task_heads(&snapshot.answer, &review.task)?;
    let context = FollowupContext {
        command,
        snapshot,
        reviewed_task,
        reviewed,
        origin,
        creation,
        repository,
        projects,
    };
    for followup in &review.then {
        create_followup(write, &context, followup)?;
    }
    Ok(())
}

struct FollowupContext<'a> {
    command: &'a wire::AdmittedCommand,
    snapshot: &'a ValidatedSnapshot,
    reviewed_task: &'a BoardTask,
    reviewed: PendingFact,
    origin: &'a CreationCause,
    creation: Option<&'a TaskCreationCapture>,
    repository: Option<&'a Repository>,
    projects: &'a ProjectPolicy,
}

type FollowupPolicy = (Vec<wire::Phase>, wire::Phase, Option<wire::TasteHold>);

fn prepare_followup_policy(
    context: &FollowupContext<'_>,
    add: &TaskAdd,
) -> Result<FollowupPolicy, Outcome<CommandAnswer>> {
    validate_acceptance(&add.acceptance)?;
    let reviewed_statement = &context.reviewed_task.statement.body;
    let reviewed_workflow = &context.reviewed_task.workflow.body;
    let design_intake = reviewed_statement.intent == Some(wire::TaskIntent::Design)
        && reviewed_workflow.status.is_pending()
        && reviewed_workflow.phase.as_str() == "review"
        && reviewed_workflow
            .flow
            .iter()
            .position(|phase| phase == &reviewed_workflow.phase)
            .and_then(|current| reviewed_workflow.flow.get(current + 1))
            .is_some_and(|next| next.as_str() == "design");
    let intent = if design_intake {
        add.intent
    } else {
        add.intent.or(reviewed_statement.intent)
    };
    if design_intake && intent.is_some() {
        return Err(refusal(
            "review::planning-correction-intent",
            None,
            "a Design intake finding creates an ordinary correction for re-review",
        ));
    }
    if !design_intake && reviewed_statement.intent.is_some() && intent != reviewed_statement.intent
    {
        return Err(refusal(
            "review::planning-correction-intent",
            None,
            "a planning correction must retain the reviewed declared intent",
        ));
    }
    let preparation = (!design_intake)
        .then(|| planning::correction_preparation(context.reviewed_task))
        .flatten();
    let mut flow = if add.flow.is_none() && preparation.is_some() {
        context.reviewed_task.workflow.body.flow.clone()
    } else {
        task_flow(
            planning::default_flow(intent, add.flow.as_deref(), &add.acceptance),
            &add.acceptance,
        )?
    };
    planning::admit_design_flow(intent, &flow)?;
    let reviewed = &context.reviewed_task.workflow.body;
    // Ordinary corrections may rely on their parent's re-review. Planning
    // corrections retain their own planning route and construction owner.
    if add.flow.is_none() && preparation.is_none() {
        omit_review_phase(
            &mut flow,
            reviewed.phase.as_str() == "review" && reviewed.status.is_pending(),
        );
    }
    let taste_review = task_taste_review(
        context.repository,
        context.snapshot,
        context.command.actor(),
        &add.title,
        &add.pitch,
        add.description.as_deref(),
        &add.acceptance,
    )?;
    let mut taste = None;
    if let Some(review) = taste_review.filter(TasteReview::required) {
        hold_for_taste(&mut flow, &mut taste, &review);
    }
    let phase = flow[0].clone();
    Ok((flow, phase, taste))
}

/// Whether the command witnesses `task`'s intended absent birth stamp.
fn absence_witnessed(context: &FollowupContext<'_>, task: &wire::TaskStamp) -> bool {
    context
        .command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Absence { task: absent } if absent == task))
}

/// Add one statement that must follow the reviewed task.
fn create_followup(
    write: &mut JournalWrite,
    context: &FollowupContext<'_>,
    followup: &ReviewFollowup,
) -> Result<(), Outcome<CommandAnswer>> {
    if !absence_witnessed(context, &followup.task) {
        return Err(protocol_fault(
            "review::followup-witness-invalid",
            "each review follow-up requires its own intended absent birth stamp",
        ));
    }
    let requested = followup.add.project.as_deref().unwrap_or("task.private");
    let project = context
        .projects
        .admit_manual_creation(requested)
        .map_err(|error| {
            refusal(
                "review::followup-refused",
                Some(error.to_string()),
                "name a public project for each review follow-up, or leave it private",
            )
        })?;
    let creation = context.creation.ok_or_else(|| {
        protocol_fault(
            "review::followup-context-missing",
            "a review follow-up has no creation-time transcript capture",
        )
    })?;
    let (flow, phase, taste) = prepare_followup_policy(context, &followup.add)?;
    let statement = TaskHead {
        preparation: planning::correction_preparation(context.reviewed_task),
        intent: followup
            .add
            .intent
            .or(context.reviewed_task.statement.body.intent),
        born: followup.task.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: followup.add.title.clone(),
        pitch: followup.add.pitch.clone(),
        description: followup.add.description.clone(),
        acceptance: followup.add.acceptance.clone(),
        requirements: followup.add.requirements.clone(),
        project: project.as_str().to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: context.origin.provenance.clone(),
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
    if let Err(error) = Entry::new(
        statement.clone(),
        workflow.clone(),
        None,
        None,
        context.projects,
    ) {
        return Err(refusal(
            "review::followup-refused",
            Some(error.to_string()),
            "correct the follow-up project, dependencies, or phase flow and retry",
        ));
    }
    let added = write
        .fact(
            body::TaskAdded {
                preparation: statement.preparation.clone(),
                intent: statement.intent,
                task: followup.task.clone(),
                title: statement.title.clone(),
                pitch: statement.pitch.clone(),
                description: statement.description.clone(),
                acceptance: statement.acceptance.clone(),
                requirements: statement.requirements.clone(),
                project: statement.project.clone(),
                filing: statement.filing,
                priority: None,
                taste: statement.taste.clone(),
                document_slug: None,
                taste_proposal: None,
                depends: statement.dependencies.clone(),
                flow,
                phase,
                revision: 1,
                creation: creation.context(),
            },
            FactCause::Pending(context.reviewed),
        )
        .map_err(|error| store_fault("review::followup-build", &error.to_string()))?;
    write.task_head(added, move |position| TaskHead {
        queue_origin: position,
        revision: position,
        ..statement
    });
    write.workflow_head(followup.task.clone(), added, move |_| workflow);
    Ok(())
}

/// The clean sentinel and the `--then` grammar, pinned away from the fixtures.
#[cfg(test)]
mod followup_tests {
    use super::*;

    #[test]
    fn a_clean_verdict_is_the_word_and_the_absence_alike() {
        // The predecessor CLI defaults --finding to clean and its allocator
        // hint prints that exact form, so the word has to name the verdict
        // rather than file itself as a finding called clean.
        assert!(review_is_clean(""), "an absent finding reviews clean");
        assert!(review_is_clean("   "), "whitespace is an absent finding");
        assert!(review_is_clean("clean"), "the predecessor default is clean");
        assert!(
            review_is_clean("Clean"),
            "letter case cannot move a verdict"
        );
        assert!(
            review_is_clean("CLEAN"),
            "letter case cannot move a verdict"
        );
        assert!(review_is_clean("  clean  "), "the predecessor strips first");
    }

    #[test]
    fn a_finding_that_merely_contains_the_word_stays_a_finding() {
        // A containment test would read every one of these as clean and drop
        // the change the reviewer asked for.
        assert!(
            !review_is_clean("cleanup the fixture"),
            "a finding that opens with the word is still a finding"
        );
        assert!(
            !review_is_clean("unclean"),
            "a finding that ends with the word is still a finding"
        );
        assert!(
            !review_is_clean("not clean yet"),
            "a finding that embeds the word is still a finding"
        );
        assert!(
            !review_is_clean("missing negative-path evidence"),
            "ordinary review prose is a finding"
        );
    }

    #[test]
    fn a_then_record_carries_the_predecessor_fields() {
        let add = parse_followup(
            "title=Cover the refusal | project=task.cli | description=why \
             | acceptance=first | acceptance=second | flow=todo,review",
        )
        .expect("a well-formed record parses");
        assert_eq!(add.title, "Cover the refusal");
        assert_eq!(add.project.as_deref(), Some("task.cli"));
        assert_eq!(add.description.as_deref(), Some("why"));
        assert_eq!(add.acceptance, vec!["first", "second"]);
        assert_eq!(add.flow.as_deref(), Some("todo,review"));
    }

    #[test]
    fn a_then_record_without_a_title_is_refused() {
        let error = parse_followup("project=task.cli").expect_err("no title is refused");
        assert!(
            error.contains("title"),
            "the refusal names the field: {error}"
        );
    }

    #[test]
    fn a_then_record_repeating_a_single_valued_field_is_refused() {
        // The predecessor refuses a repeated scalar field rather than letting
        // the last one win, so a reviewer who meant two statements is told to
        // write two records instead of silently losing one value.
        for spec in [
            "title=First | title=Second",
            "title=A | project=task.cli | project=task.board",
            "title=A | description=one | description=two",
        ] {
            let Err(error) = parse_followup(spec) else {
                panic!("a repeated scalar field is refused: {spec}");
            };
            assert!(
                error.contains("twice") && error.contains("acceptance"),
                "the refusal names the fault and the fields that may repeat: {error}"
            );
        }
        // The predecessor keeps acceptance and flow repeatable, and accrues
        // both in input order rather than refusing the second value.
        let repeated = parse_followup(
            "title=A | acceptance=first | flow=todo | acceptance=second | flow=review",
        )
        .expect("the collection fields accrue");
        assert_eq!(
            repeated.acceptance,
            vec!["first", "second"],
            "a repeated acceptance accrues in input order"
        );
        assert_eq!(
            repeated.flow.as_deref(),
            Some("todo,review"),
            "a repeated flow accrues into one phase route"
        );
    }

    #[test]
    fn a_then_record_with_an_unreadable_field_is_refused() {
        let unknown = parse_followup("title=A | priority=H").expect_err("unknown key is refused");
        assert!(
            unknown.contains("priority") && unknown.contains("acceptance"),
            "the refusal names the bad key and the accepted ones: {unknown}"
        );
        let shapeless =
            parse_followup("title=A | project").expect_err("a field with no value is refused");
        assert!(
            shapeless.contains("key=value"),
            "the refusal names the grammar: {shapeless}"
        );
    }
}
