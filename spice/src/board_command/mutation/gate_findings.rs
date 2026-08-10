use super::*;

const GATE_FINDINGS_PROJECT: &str = "gate.findings";
const GATE_FINDING_TAG: &str = "gate-finding";
const GATE_FINDING_TAG_PREFIX: &str = "gate-finding:";
const GATE_FINDING_DEFER_UNTIL: &str = "2099-12-31T23:59:59.999Z";

#[derive(Clone)]
struct GateFindingPlan {
    identity: Digest,
    tag: String,
    action: GateFindingAction,
}

#[derive(Clone)]
enum GateFindingAction {
    Reused {
        task: TaskHandle,
    },
    Created {
        task: TaskHandle,
        finding: wire::intent::GateFindingWork,
        predecessor: Option<TaskHandle>,
    },
}

pub(in crate::board_command) fn gate_findings(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let project = match projects.admit_manual_creation(GATE_FINDINGS_PROJECT) {
        Ok(project) => project,
        Err(error) => {
            return refusal(
                "gate::findings-project-refused",
                Some(error.to_string()),
                "configure the gate public project stem before filing gate findings",
            );
        }
    };
    if let Err(identity) = unique_gate_finding_identities(dispatch) {
        return refusal(
            "gate::finding-duplicate",
            Some(identity.to_string()),
            "dispatch each projected gate finding identity at most once",
        );
    }
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let until = match gate_finding_defer_until() {
        Ok(until) => until,
        Err(outcome) => return outcome,
    };
    let origin = match gate_findings_origin(command.actor(), &snapshot, &dispatch.report) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let plans = match plan_gate_findings(dispatch, &snapshot, &project, at) {
        Ok(plans) => plans,
        Err(outcome) => return outcome,
    };
    let created = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::Created { .. }))
        .count();
    let reused = plans.len().saturating_sub(created);
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard_gate_finding_plan(&mut write, plans.clone());
    let filed = match write.fact(
        body::GateFindingsFiled {
            report: dispatch.report.clone(),
            findings: dispatch.findings.len() as u32,
            created: created as u32,
            reused: reused as u32,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("gate::findings-build", &error.to_string()),
    };
    for plan in &plans {
        if let GateFindingAction::Created {
            task,
            finding,
            predecessor,
        } = &plan.action
            && let Err(outcome) = write_gate_finding_task(
                &mut write,
                filed,
                &project,
                task,
                finding,
                predecessor.as_ref(),
                &origin,
                until,
                &dispatch.report,
            )
        {
            return outcome;
        }
    }
    let text = format!(
        "gate findings report={} findings={} created={} reused={}",
        dispatch.report,
        dispatch.findings.len(),
        created,
        reused
    );
    let answer = CommandAnswer::Text(TextAnswer { text });
    if let Err(error) = write.command_result(
        request(command, dispatch.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("gate::findings-receipt", &error.to_string());
    }
    committed(bus, write, answer)
}

fn unique_gate_finding_identities(dispatch: &GateFindings) -> Result<(), Digest> {
    let mut seen = std::collections::BTreeSet::new();
    for finding in &dispatch.findings {
        if !seen.insert(finding.identity.clone()) {
            return Err(finding.identity.clone());
        }
    }
    Ok(())
}

fn gate_finding_defer_until() -> Result<Instant, Outcome<CommandAnswer>> {
    GATE_FINDING_DEFER_UNTIL
        .parse::<Instant>()
        .map_err(|error| {
            Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "gate::finding-deferral-invalid".to_owned(),
                detail: error.to_string(),
            })
        })
}

fn gate_findings_origin(
    actor: &Actor,
    snapshot: &ValidatedSnapshot,
    report: &Digest,
) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    if let Some(entry) = snapshot.entries.iter().find(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == *actor && claim.disposition == ClaimDisposition::Active
        })
    }) {
        return task_origin(&entry.statement().handle);
    }
    format!("gate:{report}").parse().map_err(|error| {
        protocol_fault(
            "gate::origin-invalid",
            &format!("gate report digest did not form an origin: {error}"),
        )
    })
}

fn plan_gate_findings(
    dispatch: &GateFindings,
    snapshot: &ValidatedSnapshot,
    project: &Project,
    at: Instant,
) -> Result<Vec<GateFindingPlan>, Outcome<CommandAnswer>> {
    let mut planned = Vec::with_capacity(dispatch.findings.len());
    let mut minted = std::collections::BTreeSet::new();
    for (index, finding) in dispatch.findings.iter().enumerate() {
        let tag = gate_finding_tag(&finding.identity);
        let matches = matching_gate_finding_tasks(snapshot, &tag);
        let open = matches
            .iter()
            .copied()
            .filter(|entry| gate_finding_task_open(entry))
            .collect::<Vec<_>>();
        if open.len() > 1 {
            return Err(refusal(
                "gate::finding-ambiguous",
                Some(finding.identity.to_string()),
                "settle duplicate open gate finding tasks before redispatching",
            ));
        }
        let action = if let Some(entry) = open.first() {
            GateFindingAction::Reused {
                task: entry.statement().handle.clone(),
            }
        } else {
            let predecessor = matches
                .iter()
                .filter(|entry| {
                    entry.workflow().status == "completed" || entry.workflow().completion.is_some()
                })
                .max_by_key(|entry| entry.statement().revision)
                .map(|entry| entry.statement().handle.clone());
            let task =
                mint_gate_finding_handle(snapshot, &mut minted, project, &finding.title, at, index);
            GateFindingAction::Created {
                task,
                finding: finding.clone(),
                predecessor,
            }
        };
        planned.push(GateFindingPlan {
            identity: finding.identity.clone(),
            tag,
            action,
        });
    }
    Ok(planned)
}

fn matching_gate_finding_tasks<'a>(snapshot: &'a ValidatedSnapshot, tag: &str) -> Vec<&'a Entry> {
    snapshot
        .entries
        .iter()
        .filter(|entry| {
            entry
                .statement()
                .tags
                .iter()
                .any(|candidate| candidate == tag)
        })
        .collect()
}

fn gate_finding_task_open(entry: &Entry) -> bool {
    entry.workflow().status == "pending" && entry.workflow().completion.is_none()
}

fn mint_gate_finding_handle(
    snapshot: &ValidatedSnapshot,
    minted: &mut std::collections::BTreeSet<TaskHandle>,
    project: &Project,
    title: &str,
    at: Instant,
    index: usize,
) -> TaskHandle {
    let occupied = snapshot
        .entries
        .iter()
        .map(|entry| entry.statement().handle.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut offset = index as i64;
    loop {
        let candidate = mint_handle_at(
            Some(project.as_str()),
            title,
            at.get().as_millisecond().saturating_add(offset),
        );
        if !occupied.contains(&candidate) && minted.insert(candidate.clone()) {
            return candidate;
        }
        offset = offset.saturating_add(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn write_gate_finding_task(
    write: &mut JournalWrite,
    cause: PendingFact,
    project: &Project,
    task: &TaskHandle,
    finding: &wire::intent::GateFindingWork,
    predecessor: Option<&TaskHandle>,
    origin: &wire::Origin,
    until: Instant,
    report: &Digest,
) -> Result<(), Outcome<CommandAnswer>> {
    let flow = phases(Some("todo,review"))?;
    let phase = flow[0].clone();
    let description = gate_finding_description(finding, predecessor, report);
    let tags = vec![
        GATE_FINDING_TAG.to_owned(),
        gate_finding_tag(&finding.identity),
        format!("gate-severity:{}", severity_text(finding.severity)),
    ];
    let uuid = wire::CanonicalUuid::new(uuid::Uuid::new_v4());
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                uuid,
                title: finding.title.clone(),
                description: Some(description.clone()),
                acceptance: finding.acceptance.clone(),
                project: project.as_str().to_owned(),
                tags: tags.clone(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("gate::finding-task-build", &error.to_string()))?;
    let statement = TaskHead {
        handle: task.clone(),
        uuid,
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: finding.title.clone(),
        description: Some(description),
        acceptance: finding.acceptance.clone(),
        project: project.as_str().to_owned(),
        tags,
        dependencies: Vec::new(),
        provenance: origin.clone(),
        revision: Seq::new(1).expect("placeholder revision is non-zero"),
    };
    write.task_head(added, move |position| TaskHead {
        queue_origin: position,
        revision: position,
        ..statement
    });
    let deferred = write
        .fact(
            body::TaskDeferred {
                task: task.clone(),
                until,
            },
            FactCause::Pending(added),
        )
        .map_err(|error| store_fault("gate::finding-defer-build", &error.to_string()))?;
    let workflow = WorkflowHead {
        flow,
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: Some(until),
        completion: None,
    };
    write.workflow_head(task.clone(), deferred, move |_| workflow);
    Ok(())
}

fn gate_finding_description(
    finding: &wire::intent::GateFindingWork,
    predecessor: Option<&TaskHandle>,
    report: &Digest,
) -> String {
    let mut description = format!(
        "{}\n\nGate report: {report}\nFinding identity: {}\nSeverity: {}\nLocus: {}",
        finding.description,
        finding.identity,
        severity_text(finding.severity),
        finding.locus
    );
    if let Some(predecessor) = predecessor {
        description.push_str(&format!("\nPrevious completed task: {predecessor}"));
    }
    description
}

fn guard_gate_finding_plan(write: &mut JournalWrite, plans: Vec<GateFindingPlan>) {
    write.guard(JournalGuard::new(move |read| {
        let heads = read.task_heads()?;
        for plan in &plans {
            match &plan.action {
                GateFindingAction::Reused { task } => {
                    let open = current_open_gate_finding_tasks(&heads, &plan.tag);
                    if open.len() != 1 || &open[0] != task {
                        return Err(GuardError::refused(format!(
                            "gate::finding-moved:{}",
                            plan.identity
                        )));
                    }
                }
                GateFindingAction::Created { task, .. } => {
                    if read.task_head(task)?.is_some()
                        || !current_open_gate_finding_tasks(&heads, &plan.tag).is_empty()
                    {
                        return Err(GuardError::refused(format!(
                            "gate::finding-moved:{}",
                            plan.identity
                        )));
                    }
                }
            }
        }
        Ok(())
    }));
}

fn current_open_gate_finding_tasks(heads: &[journal::TaskHeads], tag: &str) -> Vec<TaskHandle> {
    heads
        .iter()
        .filter(|heads| {
            heads
                .statement()
                .body()
                .tags
                .iter()
                .any(|candidate| candidate == tag)
        })
        .filter(|heads| {
            heads.workflow().body().status == "pending"
                && heads.workflow().body().completion.is_none()
        })
        .map(|heads| heads.statement().body().handle.clone())
        .collect()
}

fn gate_finding_tag(identity: &Digest) -> String {
    format!("{GATE_FINDING_TAG_PREFIX}{identity}")
}

fn severity_text(severity: body::Severity) -> &'static str {
    match severity {
        body::Severity::Low => "low",
        body::Severity::Medium => "medium",
        body::Severity::High => "high",
        body::Severity::Critical => "critical",
    }
}
