use super::*;
fn oops_snapshot(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    projects: &ProjectPolicy,
) -> Result<(ValidatedSnapshot, bool, wire::OopsGenerationHead), Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("task::oops-heads-unreadable", &error.to_string()))?;
    let (generation_head, witnessed_design) = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::OopsGeneration { head, design } => Some((*head, design.clone())),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "task::oops-generation-witness-missing",
                "OOPS intake requires one repository generation witness",
            )
        })?;
    if generation_head.is_none() && witnessed_design.is_some() {
        return Err(protocol_fault(
            "task::oops-generation-witness-invalid",
            "an OOPS generation cannot name a Design before its first head",
        ));
    }
    if witnessed_design
        .as_ref()
        .is_some_and(|design| design != &add.design)
    {
        return Err(protocol_fault(
            "task::oops-generation-design-mismatch",
            "the OOPS request must name the Design carried by its generation witness",
        ));
    }
    let generation = observed
        .oops_generation()
        .filter(|head| {
            Some(head.position()) == generation_head && head.body().design == witnessed_design
        })
        .map(|head| head.body().clone())
        .unwrap_or(wire::OopsGenerationHead {
            revision: 0,
            design: witnessed_design.clone(),
        });
    let design = witnessed_design
        .as_ref()
        .map(|design| {
            observed.task_heads(design).ok_or_else(|| {
                protocol_fault(
                    "task::oops-generation-task-missing",
                    "the witnessed OOPS generation names a task absent from the board",
                )
            })
        })
        .transpose()?;
    if let Some(heads) = design
        && (heads.statement().body().intent != Some(wire::TaskIntent::OpportunisticDesign)
            || !wire::intent::is_open_design(
                heads.statement().body().intent,
                heads.workflow().body(),
            ))
    {
        return Err(protocol_fault(
            "task::oops-generation-invalid",
            "the witnessed OOPS generation does not name one open opportunistic Design",
        ));
    }
    let snapshot = validated_working_snapshot_for_tasks(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&add.design),
        projects,
    )?;
    if design.is_some_and(|heads| heads.statement().body().born != add.design) {
        return Err(protocol_fault(
            "task::oops-generation-design-mismatch",
            "the OOPS request must name the Design carried by its generation witness",
        ));
    }
    if let Some(heads) = design {
        require_oops_design_witnesses(command, heads)?;
    }
    let creates_design = design.is_none();
    if creates_design
        && !command
            .expected()
            .iter()
            .any(|witness| matches!(witness, Witness::Absence { task } if task == &add.design))
    {
        return Err(protocol_fault(
            "task::oops-design-witness-missing",
            "opening an opportunistic OOPS Design requires its intended absent birth stamp",
        ));
    }
    Ok((snapshot, creates_design, generation))
}

pub(in crate::board_command) fn task_oops_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let (snapshot, creates_design, generation) = match oops_snapshot(bus, command, add, projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let report_born = command
        .expected()
        .iter()
        .filter_map(|witness| match witness {
            Witness::Absence { task } if task != &add.design => Some(task.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [report_born] = report_born.as_slice() else {
        return protocol_fault(
            "task::oops-report-witness-invalid",
            "OOPS intake requires exactly one intended absent report birth stamp",
        );
    };
    let project = match add.project.as_deref().map(|project| {
        projects.admit_manual_creation(project).map_err(|error| {
            refusal(
                "task::oops-project-refused",
                Some(error.to_string()),
                "name a public project this board admits for manual task creation",
            )
        })
    }) {
        Some(Ok(project)) => project,
        Some(Err(outcome)) => return outcome,
        None => match oops_claim_project(command.actor(), &snapshot.entries) {
            Ok(project) => project,
            Err(outcome) => return outcome,
        },
    };
    let design_project = match projects.admit_manual_creation(wire::OOPS_DESIGN_PROJECT) {
        Ok(project) => project,
        Err(error) => {
            return store_fault(
                "task::oops-design-project-refused",
                &format!("{}: {error}", wire::OOPS_DESIGN_PROJECT),
            );
        }
    };
    let origin =
        match resolve_creation_origin(add.origin.as_ref(), command.actor(), &snapshot, None, false)
        {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        };
    let creation = match task_creation_capture(bus, command, &snapshot) {
        Ok(creation) => creation,
        Err(outcome) => return outcome,
    };
    let design = if creates_design {
        match prepare_oops_design(
            add,
            &design_project,
            projects,
            origin.clone(),
            creation.clone(),
        ) {
            Ok(design) => Some(design),
            Err(outcome) => return outcome,
        }
    } else {
        None
    };
    let report = match prepare_oops_report(add, &project, projects, origin, creation, report_born) {
        Ok(report) => report,
        Err(outcome) => return outcome,
    };
    commit_oops_intake(
        bus,
        command,
        add,
        &snapshot,
        OopsRows { design, report },
        generation,
        projects,
    )
}

/// The rows one oops intake is born as: its Plan report and, when the
/// current generation has no open Design, the Design it opens.
struct OopsRows {
    design: Option<PreparedTask>,
    report: PreparedTask,
}

fn prepare_oops_design(
    add: &TaskOopsAdd,
    design_project: &Project,
    projects: &ProjectPolicy,
    cause: CreationCause,
    creation: TaskCreationCapture,
) -> Result<PreparedTask, Outcome<CommandAnswer>> {
    let design_add = TaskAdd {
        preparation: None,
        intent: Some(wire::TaskIntent::OpportunisticDesign),
        title: wire::OOPS_DESIGN_TITLE.to_owned(),
        pitch: wire::OOPS_DESIGN_PITCH.to_owned(),
        project: Some(wire::OOPS_DESIGN_PROJECT.to_owned()),
        description: Some(wire::OOPS_DESIGN_DESCRIPTION.to_owned()),
        acceptance: wire::OOPS_DESIGN_ACCEPTANCE.map(str::to_owned).to_vec(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: add.origin.clone(),
        flow: Some("design".to_owned()),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    prepare_task_for(
        &design_add,
        design_project,
        projects,
        TaskSeed {
            cause,
            creation,
            flow: vec!["design".parse().expect("design is a phase")],
            filing: wire::TaskFiling::Filed,
        },
        add.design.clone(),
    )
}

fn prepare_oops_report(
    add: &TaskOopsAdd,
    project: &Project,
    projects: &ProjectPolicy,
    cause: CreationCause,
    creation: TaskCreationCapture,
    report_born: &wire::TaskStamp,
) -> Result<PreparedTask, Outcome<CommandAnswer>> {
    let report_add = TaskAdd {
        preparation: None,
        intent: Some(wire::TaskIntent::Plan),
        title: add.title.clone(),
        // A friction report must not be refused, so its pitch derives once
        // from the report text instead of being asked for.
        pitch: wire::pitch::derive(&add.title, None),
        project: Some(project.as_str().to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: add.origin.clone(),
        flow: Some("plan,review".to_owned()),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let mut report = prepare_task_for(
        &report_add,
        project,
        projects,
        TaskSeed {
            cause,
            creation,
            flow: ["plan", "review"]
                .map(|phase| phase.parse().expect("fixed OOPS Plan phase"))
                .to_vec(),
            filing: wire::TaskFiling::Filed,
        },
        report_born.clone(),
    )?;
    report.statement.preparation = Some(wire::TaskPreparation {
        parent: add.design.clone(),
        controller: add.design.clone(),
        construction: report_born.clone(),
    });
    report.statement.dependencies.push(add.design.clone());
    Ok(report)
}

fn require_oops_design_witnesses(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    planning::require_expected(
        command,
        &Witness::Task {
            task: heads.statement().body().born.clone(),
            head: heads.statement().position(),
        },
    )?;
    planning::require_expected(
        command,
        &Witness::Workflow {
            task: heads.statement().body().born.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
    )
}
/// Commit one oops intake: the report, the Design it opens when the current
/// generation has none, and the generation advance binding them.
fn commit_oops_intake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &TaskOopsAdd,
    snapshot: &ValidatedSnapshot,
    rows: OopsRows,
    generation: wire::OopsGenerationHead,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let OopsRows { design, report } = rows;
    let post = prepared_entry(&report, projects);
    let next = next_action(snapshot, &post);
    let task = report.task.clone();
    let origin = report.origin.clone();
    let at = report.at;
    let creation = report.creation.clone();
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    creation.guard(&mut write);
    let cause = match design {
        Some(design) => {
            let design_stamp = design.statement.born.clone();
            let added = match append_prepared_task(&mut write, design, FactCause::Root) {
                Ok(fact) => fact,
                Err(outcome) => return outcome,
            };
            let Some(revision) = generation.revision.checked_add(1) else {
                return store_fault(
                    "task::oops-generation-exhausted",
                    "the OOPS generation revision cannot advance",
                );
            };
            let advanced = wire::body::OopsGenerationAdvanced {
                revision,
                from: generation.design,
                to: Some(design_stamp),
            };
            match write.fact(advanced, FactCause::Pending(added)) {
                Ok(fact) => FactCause::Pending(fact),
                Err(error) => {
                    return store_fault("task::oops-generation-build", &error.to_string());
                }
            }
        }
        None => FactCause::Root,
    };
    if let Err(outcome) = append_prepared_task(&mut write, report, cause) {
        return outcome;
    }
    let answer = wire::intent::TaskAnswer {
        task,
        status: "pending".to_owned(),
        completion: None,
        claim: None,
        closed: Vec::new(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::oops-add-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(Box::new(answer)))
}
