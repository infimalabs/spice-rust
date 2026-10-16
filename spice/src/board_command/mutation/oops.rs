use super::*;
fn oops_snapshot(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<(ValidatedSnapshot, bool), Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("task::oops-heads-unreadable", &error.to_string()))?;
    let current = observed
        .tasks()
        .iter()
        .filter(|heads| {
            heads.statement().body().intent == Some(wire::TaskIntent::OpportunisticDesign)
                && wire::intent::is_open_design(
                    heads.statement().body().intent,
                    heads.workflow().body(),
                )
        })
        .collect::<Vec<_>>();
    if current.len() > 1 {
        return Err(protocol_fault(
            "task::oops-design-ambiguous",
            "the board contains more than one current opportunistic OOPS Design",
        ));
    }
    let roots = std::iter::once(add.design.clone())
        .chain(
            current
                .iter()
                .map(|heads| heads.statement().body().born.clone()),
        )
        .collect::<Vec<_>>();
    let snapshot = validated_working_snapshot_for_tasks(
        bus,
        command.actor().clone(),
        &roots,
        projects,
        allocation,
    )?;
    let design = current.first().copied();
    if design.is_some_and(|heads| heads.statement().body().born != add.design) {
        return Err(refusal(
            "task::oops-design-moved",
            Some(add.design.to_string()),
            "read the current board and retry under its current opportunistic OOPS Design",
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
    Ok((snapshot, creates_design))
}

pub(in crate::board_command) fn task_oops_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (snapshot, creates_design) = match oops_snapshot(bus, command, add, projects, allocation) {
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
        match resolve_creation_origin(add.origin.as_ref(), command.actor(), &snapshot, None) {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        };
    let creation = match task_creation_capture(bus, command, &snapshot) {
        Ok(creation) => creation,
        Err(outcome) => return outcome,
    };
    let (design_prepared, report) = match prepare_oops_rows(
        command,
        add,
        &project,
        &design_project,
        projects,
        origin,
        creation,
        report_born,
        creates_design,
    ) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    commit_oops_intake(
        bus,
        command,
        add,
        &snapshot,
        design_prepared,
        report,
        projects,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_oops_rows(
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    project: &Project,
    design_project: &Project,
    projects: &ProjectPolicy,
    origin: CreationCause,
    creation: TaskCreationCapture,
    report_born: &wire::TaskStamp,
    creates_design: bool,
) -> Result<(Option<PreparedTask>, PreparedTask), Outcome<CommandAnswer>> {
    let design_add = TaskAdd {
        preparation: None,
        intent: Some(wire::TaskIntent::OpportunisticDesign),
        title: wire::OOPS_DESIGN_TITLE.to_owned(),
        project: Some(wire::OOPS_DESIGN_PROJECT.to_owned()),
        description: Some(wire::OOPS_DESIGN_DESCRIPTION.to_owned()),
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: add.origin.clone(),
        flow: Some("design".to_owned()),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let report_add = TaskAdd {
        preparation: None,
        intent: Some(wire::TaskIntent::Plan),
        title: add.title.clone(),
        project: Some(project.as_str().to_owned()),
        description: Some(add.description.clone()),
        acceptance: wire::OOPS_LINE_ACCEPTANCE.map(str::to_owned).to_vec(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: add.origin.clone(),
        flow: Some("plan,review".to_owned()),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let design = creates_design
        .then(|| {
            prepare_task_for(
                command,
                &design_add,
                design_project,
                projects,
                origin.clone(),
                creation.clone(),
                vec!["design".parse().expect("design is a phase")],
                wire::TaskFiling::Filed,
                add.design.clone(),
            )
        })
        .transpose()?;
    let mut report = prepare_task_for(
        command,
        &report_add,
        project,
        projects,
        origin,
        creation,
        ["plan", "review"]
            .map(|phase| phase.parse().expect("fixed OOPS Plan phase"))
            .to_vec(),
        wire::TaskFiling::Filed,
        report_born.clone(),
    )?;
    report.statement.preparation = Some(wire::TaskPreparation {
        parent: add.design.clone(),
        controller: add.design.clone(),
        construction: report_born.clone(),
    });
    Ok((design, report))
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
fn commit_oops_intake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &TaskOopsAdd,
    snapshot: &ValidatedSnapshot,
    design: Option<PreparedTask>,
    report: PreparedTask,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let post = prepared_entry(&report, projects);
    let next = next_action(snapshot, &post);
    let task = report.task.clone();
    let origin = report.origin.clone();
    let at = report.at;
    let creation = report.creation.clone();
    let report_add = TaskAdd {
        preparation: None,
        intent: Some(wire::TaskIntent::Plan),
        title: request_body.title.clone(),
        project: Some(report.project.clone()),
        description: Some(request_body.description.clone()),
        acceptance: wire::OOPS_LINE_ACCEPTANCE.map(str::to_owned).to_vec(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: request_body.origin.clone(),
        flow: Some("plan,review".to_owned()),
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    creation.guard(&mut write);
    let cause = match design {
        Some(design) => {
            let design_add = TaskAdd {
                preparation: None,
                intent: Some(wire::TaskIntent::OpportunisticDesign),
                title: wire::OOPS_DESIGN_TITLE.to_owned(),
                project: Some(wire::OOPS_DESIGN_PROJECT.to_owned()),
                description: Some(wire::OOPS_DESIGN_DESCRIPTION.to_owned()),
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                origin: request_body.origin.clone(),
                flow: Some("design".to_owned()),
                depends: Vec::new(),
                claim: false,
                draft: false,
            };
            match append_prepared_task(&mut write, &design_add, design, FactCause::Root) {
                Ok(fact) => FactCause::Pending(fact),
                Err(outcome) => return outcome,
            }
        }
        None => FactCause::Root,
    };
    if let Err(outcome) = append_prepared_task(&mut write, &report_add, report, cause) {
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
    committed(bus, write, CommandAnswer::Task(answer))
}
