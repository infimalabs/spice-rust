use super::*;

#[derive(Clone)]
struct ProposalObservation {
    project: String,
    heads: Vec<(TaskStamp, Seq, Seq)>,
}

enum ProposalPlan {
    Reuse {
        index: usize,
        completed: bool,
    },
    Create {
        predecessor: Option<TaskHandle>,
        observation: ProposalObservation,
    },
}

pub(in crate::board_command) fn taste_proposal_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TasteProposalAdd,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = validate_request(add) {
        return outcome;
    }
    let project = match projects.admit_system_creation(&SystemTaskRoute::proposal([
        "taste".to_owned(),
        add.finding.to_string(),
    ])) {
        Ok(project) => project,
        Err(error) => {
            return protocol_fault(
                "taste::proposal-route-invalid",
                &format!("resident-owned taste proposal route is not admitted: {error}"),
            );
        }
    };
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let plan = match proposal_plan(&snapshot, project.as_str(), add) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    let (predecessor, observation) = match plan {
        ProposalPlan::Reuse { index, completed } => {
            return reused(&snapshot, index, completed);
        }
        ProposalPlan::Create {
            predecessor,
            observation,
        } => (predecessor, observation),
    };
    let expected_depends = predecessor.iter().cloned().collect::<Vec<_>>();
    if add.depends != expected_depends {
        return refusal(
            "taste::proposal-predecessor-moved",
            Some(format!(
                "expected={expected_depends:?} supplied={:?}",
                add.depends
            )),
            "mine again against the current completed proposal team",
        );
    }
    let normalized = TaskAdd {
        preparation: None,
        intent: None,
        title: add.title.clone(),
        pitch: wire::pitch::derive(&add.title, Some(&add.description)),
        project: None,
        description: Some(add.description.clone()),
        acceptance: add.acceptance.clone(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some(add.origin.clone()),
        flow: Some("todo,review".to_owned()),
        depends: expected_depends,
        claim: false,
        draft: false,
    };
    let origin = match resolve_creation_origin(
        normalized.origin.as_ref(),
        command.actor(),
        &snapshot,
        None,
        false,
    ) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let creation = match task_creation_capture(bus, command, &snapshot) {
        Ok(creation) => creation,
        Err(outcome) => return outcome,
    };
    let (flow, dependency_witnesses) =
        match task_flow_for_dependencies(command, &normalized, &snapshot) {
            Ok(observation) => observation,
            Err(outcome) => return outcome,
        };
    let mut prepared = match prepare_task(
        command,
        &normalized,
        &project,
        projects,
        origin,
        creation,
        flow,
        wire::TaskFiling::Filed,
    ) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    prepared.statement.taste_proposal = Some(wire::TasteProposal {
        finding: add.finding.clone(),
        occurrence: add.occurrence.clone(),
        recurrence: add.recurrence,
    });
    let post = prepared_entry(&prepared, projects);
    commit_prepared_task(
        bus,
        command,
        &normalized,
        add.clone(),
        &snapshot,
        prepared,
        post,
        dependency_witnesses,
        move |write| guard_proposal_observation(write, observation),
        None,
    )
}

fn validate_request(add: &TasteProposalAdd) -> Result<(), Outcome<CommandAnswer>> {
    if add.recurrence < 2 {
        return Err(refusal(
            "taste::proposal-recurrence-invalid",
            Some(add.recurrence.to_string()),
            "file only a recurring correction supported by independent loci",
        ));
    }
    if crate::taste_command::contains_sensitive_text(&add.description) {
        return Err(refusal(
            "taste::proposal-evidence-sensitive",
            None,
            "exclude secret-bearing excerpts and NAK reasons before filing review evidence",
        ));
    }
    let described_finding = description_value(&add.description, "Finding identity: ")
        .and_then(|value| value.parse::<Digest>().ok());
    let described_occurrence = description_value(&add.description, "Occurrence identity: ")
        .and_then(|value| value.parse::<Digest>().ok());
    let described_recurrence = description_value(&add.description, "Recurrence count: ")
        .and_then(|value| value.parse::<usize>().ok());
    if described_finding.as_ref() != Some(&add.finding)
        || described_occurrence.as_ref() != Some(&add.occurrence)
        || described_recurrence != Some(add.recurrence)
    {
        return Err(refusal(
            "taste::proposal-evidence-mismatch",
            None,
            "keep the durable finding, occurrence, and recurrence evidence aligned with the typed request",
        ));
    }
    Ok(())
}

fn proposal_plan(
    snapshot: &ValidatedSnapshot,
    project: &str,
    add: &TasteProposalAdd,
) -> Result<ProposalPlan, Outcome<CommandAnswer>> {
    let mut matches = Vec::new();
    for (index, entry) in snapshot.entries.iter().enumerate() {
        if entry.statement().project == project {
            matches.push((index, proposal_metadata(entry)?));
        }
    }
    let open = matches
        .iter()
        .filter(|(index, _)| proposal_open(&snapshot.entries[*index]))
        .collect::<Vec<_>>();
    if open.len() > 1 {
        return Err(state_fault(project, "more than one active proposal"));
    }
    if let Some((index, _)) = open.first() {
        return Ok(ProposalPlan::Reuse {
            index: *index,
            completed: false,
        });
    }
    let identical = matches
        .iter()
        .filter(|(_, metadata)| metadata.occurrence == add.occurrence)
        .collect::<Vec<_>>();
    if identical.len() > 1 {
        return Err(state_fault(project, "more than one identical occurrence"));
    }
    if let Some((index, _)) = identical.first() {
        return Ok(ProposalPlan::Reuse {
            index: *index,
            completed: true,
        });
    }
    let predecessor = matches
        .iter()
        .max_by(|(left_index, left), (right_index, right)| {
            left.recurrence.cmp(&right.recurrence).then_with(|| {
                snapshot.entries[*left_index]
                    .handle()
                    .cmp(snapshot.entries[*right_index].handle())
            })
        });
    if let Some((index, metadata)) = predecessor
        && metadata.recurrence >= add.recurrence
    {
        return Ok(ProposalPlan::Reuse {
            index: *index,
            completed: true,
        });
    }
    Ok(ProposalPlan::Create {
        predecessor: predecessor.map(|(index, _)| snapshot.entries[*index].handle().clone()),
        observation: proposal_observation(snapshot, project),
    })
}

fn proposal_metadata(entry: &Entry) -> Result<&wire::TasteProposal, Outcome<CommandAnswer>> {
    entry
        .statement()
        .taste_proposal
        .as_ref()
        .filter(|proposal| proposal.recurrence >= 2)
        .ok_or_else(|| {
            state_fault(
                entry.statement().project.as_str(),
                "proposal metadata is missing or invalid",
            )
        })
}

fn description_value<'a>(description: &'a str, prefix: &str) -> Option<&'a str> {
    description
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
}

fn proposal_open(entry: &Entry) -> bool {
    entry.workflow().status.is_pending()
}

fn state_fault(project: &str, detail: &str) -> Outcome<CommandAnswer> {
    store_fault(
        "taste::proposal-state-invalid",
        &format!("hidden proposal project {project} {detail}"),
    )
}

fn reused(snapshot: &ValidatedSnapshot, index: usize, completed: bool) -> Outcome<CommandAnswer> {
    let entry = &snapshot.entries[index];
    let position = entry.statement().queue_origin;
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: position,
            last: position,
        }),
        body: CommandAnswer::Task(Box::new(wire::intent::TaskAnswer {
            task: entry.handle().clone(),
            status: if completed {
                "reused-completed"
            } else {
                "reused-active"
            }
            .to_owned(),
            completion: None,
            claim: None,
            closed: Vec::new(),
            next: next_action(snapshot, entry),
        })),
    })
}

fn proposal_observation(snapshot: &ValidatedSnapshot, project: &str) -> ProposalObservation {
    let heads = snapshot
        .answer
        .tasks
        .iter()
        .filter(|proposal_task| proposal_task.statement.body.project == project)
        .map(|proposal_task| {
            (
                proposal_task.statement.body.born.clone(),
                proposal_task.statement.position,
                proposal_task.workflow.position,
            )
        })
        .collect();
    ProposalObservation {
        project: project.to_owned(),
        heads,
    }
}

fn guard_proposal_observation(write: &mut JournalWrite, observed: ProposalObservation) {
    write.guard(JournalGuard::new(move |read| {
        let page = read.task_project_head_page(
            &observed.project,
            None,
            journal::FactReadBudget::RETAINED,
        )?;
        if let Some(continuation) = page.continuation {
            return Err(GuardError::refused(format!(
                "taste proposal project guard requires continuation after {continuation}"
            )));
        }
        let current = page
            .rows
            .into_iter()
            .map(|proposal_heads| {
                (
                    proposal_heads.statement().body().born.clone(),
                    proposal_heads.statement().position(),
                    proposal_heads.workflow().position(),
                )
            })
            .collect::<Vec<_>>();
        if current == observed.heads {
            Ok(())
        } else {
            Err(GuardError::refused(format!(
                "taste::proposal-moved:{}",
                observed.project
            )))
        }
    }));
}
