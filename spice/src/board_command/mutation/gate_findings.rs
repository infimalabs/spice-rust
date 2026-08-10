use super::*;

const GATE_FINDINGS_PROJECT: &str = "gate.findings";
const GATE_FINDING_TAG: &str = "gate-finding";
const GATE_FINDING_TAG_PREFIX: &str = "gate-finding:";
const GATE_FINDING_DEFER_UNTIL: &str = "2099-12-31T23:59:59.999Z";

#[derive(Clone)]
pub(in crate::board_command) struct FindingRow {
    pub(in crate::board_command) identity: Digest,
    pub(in crate::board_command) severity: String,
    pub(in crate::board_command) locus: String,
    pub(in crate::board_command) title: String,
    pub(in crate::board_command) description: String,
    pub(in crate::board_command) acceptance: Vec<String>,
}

impl From<gate::GateFinding> for FindingRow {
    fn from(finding: gate::GateFinding) -> Self {
        Self {
            identity: finding.identity().clone(),
            severity: finding.severity().to_owned(),
            locus: finding.locus().to_owned(),
            title: finding.title().to_owned(),
            description: finding.description().to_owned(),
            acceptance: finding.acceptance().to_vec(),
        }
    }
}

#[derive(Clone)]
struct GateFindingPlan {
    identity: Digest,
    tag: String,
    action: GateFindingAction,
}

#[derive(Clone)]
enum GateFindingAction {
    Reused {
        task: wire::TaskStamp,
    },
    Created {
        task: wire::TaskStamp,
        finding: FindingRow,
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
    let report = match focused_report(dispatch) {
        Ok(report) => report,
        Err(outcome) => return outcome,
    };
    let instrument = report.gate_id();
    let admits = report.admits();
    let findings = report
        .findings()
        .into_iter()
        .map(FindingRow::from)
        .collect::<Vec<_>>();
    gate_findings_folded(
        bus, command, dispatch, &findings, instrument, admits, projects, allocation,
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::board_command) fn gate_findings_folded(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    instrument: gate::GateId,
    admits: bool,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = validate_finding_fold(findings, admits) {
        return outcome;
    }
    let prepared =
        match prepare_finding_fold(bus, command, dispatch, findings, projects, allocation) {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
    commit_finding_fold(
        bus, command, dispatch, findings, instrument, admits, prepared,
    )
}

struct PreparedFindingFold {
    project: Project,
    at: Instant,
    until: Instant,
    origin: wire::Origin,
    plans: Vec<GateFindingPlan>,
    created: usize,
    reused: usize,
}

fn validate_finding_fold(
    findings: &[FindingRow],
    admits: bool,
) -> Result<(), Outcome<CommandAnswer>> {
    if admits != findings.is_empty() {
        return Err(refusal(
            "gate::finding-report-fold-mismatch",
            Some(format!(
                "state={} findings={}",
                if admits { "clean" } else { "findings" },
                findings.len()
            )),
            "derive every finding and the admission state from the same focused report",
        ));
    }
    if let Err(identity) = unique_gate_finding_identities(findings) {
        return Err(refusal(
            "gate::finding-duplicate",
            Some(identity.to_string()),
            "dispatch each folded gate finding identity at most once",
        ));
    }
    Ok(())
}

fn prepare_finding_fold(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<PreparedFindingFold, Outcome<CommandAnswer>> {
    let project = match projects.admit_manual_creation(GATE_FINDINGS_PROJECT) {
        Ok(project) => project,
        Err(error) => {
            return Err(refusal(
                "gate::findings-project-refused",
                Some(error.to_string()),
                "configure the gate public project stem before filing gate findings",
            ));
        }
    };
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    let at = current_instant()?;
    let until = gate_finding_defer_until()?;
    let origin = gate_findings_origin(command.actor(), &snapshot, &dispatch.report)?;
    let plans = plan_gate_findings(findings, &snapshot, at)?;
    let created = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::Created { .. }))
        .count();
    let reused = plans.len().saturating_sub(created);
    Ok(PreparedFindingFold {
        project,
        at,
        until,
        origin,
        plans,
        created,
        reused,
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_finding_fold(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    instrument: gate::GateId,
    admits: bool,
    prepared: PreparedFindingFold,
) -> Outcome<CommandAnswer> {
    let mut write = JournalWrite::new(
        command.actor().clone(),
        prepared.origin.clone(),
        prepared.at,
        Producer::CommandDoor,
    );
    guard_gate_finding_plan(&mut write, prepared.plans.clone());
    let filed = match write.fact(
        body::GateFindingsFiled {
            report: dispatch.report.clone(),
            findings: findings.len() as u32,
            created: prepared.created as u32,
            reused: prepared.reused as u32,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("gate::findings-build", &error.to_string()),
    };
    for plan in &prepared.plans {
        if let GateFindingAction::Created {
            task,
            finding,
            predecessor,
        } = &plan.action
            && let Err(outcome) = write_gate_finding_task(
                &mut write,
                filed,
                &prepared.project,
                task,
                finding,
                predecessor.as_ref(),
                &prepared.origin,
                prepared.until,
                &dispatch.report,
            )
        {
            return outcome;
        }
    }
    let text = format!(
        "gate findings instrument={} state={} report={} findings={} created={} reused={}",
        instrument,
        if admits { "clean" } else { "findings" },
        dispatch.report,
        findings.len(),
        prepared.created,
        prepared.reused
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

fn focused_report(
    dispatch: &GateFindings,
) -> Result<gate::FocusedArmReport, Outcome<CommandAnswer>> {
    let observed = Digest::compute(Domain::GateFocusedReport, dispatch.evidence.as_bytes());
    if observed != dispatch.report {
        return Err(refusal(
            "gate::finding-report-digest-mismatch",
            Some(format!("expected={} observed={observed}", dispatch.report)),
            "dispatch the unchanged canonical focused report and its computed digest",
        ));
    }
    gate::FocusedArmReport::decode(dispatch.evidence.as_bytes()).map_err(|error| {
        refusal(
            "gate::finding-report-invalid",
            Some(error.to_string()),
            "rerun the registered gate instrument and dispatch its canonical focused report",
        )
    })
}

fn unique_gate_finding_identities(findings: &[FindingRow]) -> Result<(), Digest> {
    let mut seen = std::collections::BTreeSet::new();
    for finding in findings {
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
    let intrinsic = format!("gate:{report}").parse().map_err(|error| {
        protocol_fault(
            "gate::origin-invalid",
            &format!("gate report digest did not form an origin: {error}"),
        )
    })?;
    resolve_creation_origin(None, actor, snapshot, Some(intrinsic))
}

fn plan_gate_findings(
    findings: &[FindingRow],
    snapshot: &ValidatedSnapshot,
    at: Instant,
) -> Result<Vec<GateFindingPlan>, Outcome<CommandAnswer>> {
    let mut planned = Vec::with_capacity(findings.len());
    let mut minted = std::collections::BTreeSet::new();
    for (index, finding) in findings.iter().enumerate() {
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
                task: entry.incepted().clone(),
            }
        } else {
            let predecessor = matches
                .iter()
                .filter(|entry| {
                    entry.workflow().status == "completed" || entry.workflow().completion.is_some()
                })
                .max_by_key(|entry| entry.statement().revision)
                .map(|entry| entry.handle().clone());
            let task = mint_gate_finding_stamp(snapshot, &mut minted, at, index);
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

fn mint_gate_finding_stamp(
    snapshot: &ValidatedSnapshot,
    minted: &mut std::collections::BTreeSet<wire::TaskStamp>,
    at: Instant,
    index: usize,
) -> wire::TaskStamp {
    let occupied = snapshot
        .entries
        .iter()
        .map(|entry| entry.incepted().clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut offset = index as i64;
    loop {
        let candidate =
            wire::TaskStamp::for_millis(at.get().as_millisecond().saturating_add(offset))
                .expect("the current timestamp fits the task stamp codec");
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
    task: &wire::TaskStamp,
    finding: &FindingRow,
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
        format!("gate-severity:{}", finding.severity),
    ];
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
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
        incepted: task.clone(),
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
    finding: &FindingRow,
    predecessor: Option<&TaskHandle>,
    report: &Digest,
) -> String {
    let mut description = format!(
        "{}\n\nGate report: {report}\nFinding identity: {}\nSeverity: {}\nLocus: {}",
        finding.description, finding.identity, finding.severity, finding.locus
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

fn current_open_gate_finding_tasks(
    heads: &[journal::TaskHeads],
    tag: &str,
) -> Vec<wire::TaskStamp> {
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
        .map(|heads| heads.statement().body().incepted.clone())
        .collect()
}

fn gate_finding_tag(identity: &Digest) -> String {
    format!("{GATE_FINDING_TAG_PREFIX}{identity}")
}
