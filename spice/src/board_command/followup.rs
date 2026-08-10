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
const FOLLOWUP_FIELDS: &str = "title, project, description, acceptance or flow";

/// The `--then` fields that accrue rather than refusing a second value.
///
/// The predecessor keeps `after` and `tags` here too, which this record has no
/// place to put: a follow-up depends on the reviewed task and carries no tags.
const FOLLOWUP_REPEATS: &[&str] = &["acceptance", "flow"];

/// Whether a finding records a clean review rather than a requested change.
pub(super) fn review_is_clean(finding: &str) -> bool {
    let finding = finding.trim();
    finding.is_empty() || finding.eq_ignore_ascii_case("clean")
}

/// Parse one `--then` record into the statement it creates.
fn parse_followup(spec: &str) -> Result<TaskAdd, String> {
    let mut add = TaskAdd {
        title: String::new(),
        project: None,
        description: None,
        acceptance: Vec::new(),
        origin: None,
        flow: None,
        depends: Vec::new(),
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
    Ok(add)
}

/// Mint one inception stamp per `--then` record and witness each intended absence.
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
    let born = allocate_inceptions(board, jiff::Timestamp::now().as_millisecond(), parsed.len());
    let mut planned: Vec<ReviewFollowup> = Vec::new();
    for (add, task) in parsed.into_iter().zip(born) {
        expected.push(Witness::Absence { task: task.clone() });
        planned.push(ReviewFollowup { task, add });
    }
    Ok(planned)
}

/// Resolve every `--followup` handle and witness the edge it will gain.
pub(super) fn plan_followup_edges(
    board: &BoardAnswer,
    matches: &ArgMatches,
    task: &TaskHandle,
    expected: &mut Vec<Witness>,
) -> Result<Vec<TaskHandle>, Outcome<CommandAnswer>> {
    let mut linked = Vec::new();
    for written in matches.get_many::<String>("followup").into_iter().flatten() {
        let handle = written.parse::<TaskHandle>().map_err(|error| {
            refusal(
                "task::handle-invalid",
                Some(written.clone()),
                &error.to_string(),
            )
        })?;
        let heads = task_heads(board, &handle)?;
        expected.push(statement_witness(heads));
        expected.push(workflow_witness(heads));
        expected.push(Witness::Edge {
            task: heads.statement.body.born.clone(),
            after: task_heads(board, task)?.statement.body.born.clone(),
        });
        linked.push(handle);
    }
    Ok(linked)
}

/// Create the `--then` statements and wire the `--followup` edges.
pub(super) fn record_followups(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    snapshot: &ValidatedSnapshot,
    review: &TaskReview,
    reviewed: PendingFact,
    origin: &wire::Origin,
    projects: &ProjectPolicy,
) -> Result<(), Outcome<CommandAnswer>> {
    let context = FollowupContext {
        command,
        snapshot,
        review,
        reviewed,
        origin,
        projects,
    };
    for followup in &review.then {
        create_followup(write, &context, followup)?;
    }
    for handle in &review.followup {
        wire_followup_edge(write, snapshot, review, handle, reviewed, projects)?;
    }
    Ok(())
}

struct FollowupContext<'a> {
    command: &'a wire::AdmittedCommand,
    snapshot: &'a ValidatedSnapshot,
    review: &'a TaskReview,
    reviewed: PendingFact,
    origin: &'a wire::Origin,
    projects: &'a ProjectPolicy,
}

/// Add one statement that must follow the reviewed task.
fn create_followup(
    write: &mut JournalWrite,
    context: &FollowupContext<'_>,
    followup: &ReviewFollowup,
) -> Result<(), Outcome<CommandAnswer>> {
    let witnessed = context
        .command
        .expected()
        .iter()
        .any(|witness| matches!(witness, Witness::Absence { task } if *task == followup.task));
    if !witnessed {
        return Err(protocol_fault(
            "review::followup-witness-invalid",
            "each review follow-up requires its own intended absent inception stamp",
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
    let reviewed_entry = context
        .snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &context.review.task)
        .ok_or_else(|| {
            protocol_fault(
                "review::task-missing",
                "reviewed task disappeared from the validated snapshot",
            )
        })?;
    let flow = task_flow(followup.add.flow.as_deref(), &followup.add.acceptance)?;
    let phase = flow[0].clone();
    let statement = TaskHead {
        born: followup.task.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: followup.add.title.clone(),
        description: followup.add.description.clone(),
        acceptance: followup.add.acceptance.clone(),
        project: project.as_str().to_owned(),
        tags: Vec::new(),
        dependencies: vec![reviewed_entry.born().clone()],
        provenance: context.origin.clone(),
        revision: Seq::new(1).expect("placeholder revision is non-zero"),
    };
    let workflow = WorkflowHead {
        flow: flow.clone(),
        phase: phase.clone(),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
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
                task: followup.task.clone(),
                title: statement.title.clone(),
                description: statement.description.clone(),
                acceptance: statement.acceptance.clone(),
                project: statement.project.clone(),
                tags: Vec::new(),
                depends: statement.dependencies.clone(),
                flow,
                phase,
                revision: 1,
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

/// Wire one existing task to follow the reviewed task.
fn wire_followup_edge(
    write: &mut JournalWrite,
    snapshot: &ValidatedSnapshot,
    review: &TaskReview,
    handle: &TaskHandle,
    reviewed: PendingFact,
    projects: &ProjectPolicy,
) -> Result<(), Outcome<CommandAnswer>> {
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == handle)
    else {
        return Err(refusal(
            "task::unknown",
            Some(handle.to_string()),
            "name an existing task as the review follow-up",
        ));
    };
    if entry.workflow().status == "completed" {
        return Err(refusal(
            "task::settled",
            Some(handle.to_string()),
            "track a review finding with a pending follow-up task",
        ));
    }
    let mut statement = entry.statement().clone();
    let reviewed_entry = snapshot
        .entries
        .iter()
        .find(|candidate| candidate.handle() == &review.task)
        .expect("reviewed task belongs to the validated snapshot");
    if entry.born() == reviewed_entry.born()
        || statement.dependencies.contains(reviewed_entry.born())
    {
        return Err(refusal(
            "task::dependency-refused",
            Some(format!("{handle} after {}", review.task)),
            "name a distinct follow-up that does not already follow the reviewed task",
        ));
    }
    statement.dependencies.push(reviewed_entry.born().clone());
    if let Err(error) = Entry::new(
        statement.clone(),
        entry.workflow().clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    ) {
        return Err(refusal(
            "task::dependency-refused",
            Some(error.to_string()),
            "submit a valid acyclic follow-up edge",
        ));
    }
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                task: entry.born().clone(),
                revision,
            },
            FactCause::Pending(reviewed),
        )
        .map_err(|error| store_fault("review::followup-edge", &error.to_string()))?;
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
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
