use super::*;

mod migration;
use migration::{current_open_instrument_tasks, open_instrument_findings, open_instrument_tasks};

const GATE_FINDINGS_PROJECT: &str = "gate.findings";
const GATE_FINDING_TAG: &str = "gate-finding";
const GATE_FINDING_TAG_PREFIX: &str = "gate-finding:";
const GATE_INSTRUMENT_TAG_PREFIX: &str = "gate-instrument:";
const GATE_SEVERITY_TAG_PREFIX: &str = "gate-severity:";
const GATE_REPOSITORY_TAG_PREFIX: &str = "gate-repository:";
const GATE_SELECTION_TAG_PREFIX: &str = "gate-selection:";
const GATE_SOURCE_TREE_TAG_PREFIX: &str = "gate-source-tree:";
const GATE_SOURCE_ANCHOR_TAG_PREFIX: &str = "gate-source-anchor:";
const GATE_SOURCE_KIND_TAG_PREFIX: &str = "gate-source-kind:";
const GATE_MAINLINE_REF_TAG_PREFIX: &str = "gate-mainline-ref:";
const GATE_MAINLINE_BASE_TAG_PREFIX: &str = "gate-mainline-base:";

#[derive(Clone)]
pub(in crate::board_command) struct FindingRow {
    pub(in crate::board_command) identity: Digest,
    pub(in crate::board_command) severity: String,
    pub(in crate::board_command) authority: String,
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
            authority: finding.authority().to_owned(),
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
        heads: GateFindingHeads,
    },
    Updated {
        task: wire::TaskStamp,
        heads: GateFindingHeads,
        statement: TaskHead,
    },
    Created {
        task: wire::TaskStamp,
        finding: FindingRow,
        predecessor: Option<TaskHandle>,
    },
    Recovered {
        task: wire::TaskStamp,
        heads: GateFindingHeads,
        transition: board::Transition,
        claim: Box<ClaimHead>,
        source: body::SourceBoundary,
    },
    RecoveryPending {
        task: wire::TaskStamp,
        heads: GateFindingHeads,
    },
}

#[derive(Clone, Copy)]
struct GateFindingHeads {
    statement: Seq,
    workflow: Seq,
    claim: Option<Seq>,
}

pub(in crate::board_command) fn gate_findings(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let report = match focused_report(dispatch) {
        Ok(report) => report,
        Err(outcome) => return outcome,
    };
    let Some(repository) = repository else {
        return protocol_fault(
            "gate::finding-repository-unavailable",
            "gate finding filing requires the resident repository authority",
        );
    };
    if let Err(error) = validate_focused_source(repository, dispatch, &report) {
        return error;
    }
    let instrument = report.gate_id();
    let admits = report.admits();
    let findings = report
        .findings()
        .into_iter()
        .map(FindingRow::from)
        .collect::<Vec<_>>();
    gate_findings_folded(
        bus, command, dispatch, &findings, instrument, admits, repository, projects, allocation,
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
    repository: &Repository,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    gate_findings_folded_with_relation(
        bus,
        command,
        dispatch,
        findings,
        instrument,
        admits,
        repository,
        projects,
        allocation,
        &require_gate_finding_source_relation,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::board_command) fn gate_findings_folded_in_process(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    instrument: gate::GateId,
    admits: bool,
    repository: &Repository,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    gate_findings_folded_with_relation(
        bus,
        command,
        dispatch,
        findings,
        instrument,
        admits,
        repository,
        projects,
        allocation,
        &require_in_process_source_relation,
    )
}

type SourceRelation = dyn Fn(
    &Repository,
    &wire::intent::GateFindingSource,
    &wire::intent::GateFindingSource,
    bool,
) -> Result<(), Outcome<CommandAnswer>>;

#[allow(clippy::too_many_arguments)]
fn gate_findings_folded_with_relation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    instrument: gate::GateId,
    admits: bool,
    repository: &Repository,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    source_relation: &SourceRelation,
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = validate_finding_fold(findings, admits) {
        return outcome;
    }
    let prepared = match prepare_finding_fold(
        bus,
        command,
        dispatch,
        findings,
        instrument,
        repository,
        projects,
        allocation,
        source_relation,
    ) {
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
    creation: Option<TaskCreationCapture>,
    plans: Vec<GateFindingPlan>,
    instrument: gate::GateId,
    source: wire::intent::GateFindingSource,
    instrument_tasks: std::collections::BTreeSet<wire::TaskStamp>,
    created: usize,
    updated: usize,
    reused: usize,
    cleared: usize,
    advanced: usize,
    recovery_pending: usize,
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

#[allow(clippy::too_many_arguments)]
fn prepare_finding_fold(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    dispatch: &GateFindings,
    findings: &[FindingRow],
    instrument: gate::GateId,
    repository: &Repository,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    source_relation: &SourceRelation,
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
    let birth_micros = jiff::Timestamp::now().as_microsecond();
    let until = gate_finding_defer_until()?;
    let origin = gate_findings_origin(command.actor(), &snapshot, &dispatch.report)?;
    let instrument_tasks = open_instrument_tasks(&snapshot.entries, instrument)?;
    let plans = plan_gate_findings(
        bus,
        findings,
        &snapshot,
        at,
        birth_micros,
        instrument,
        repository,
        &dispatch.source,
        &dispatch.report,
        command.actor(),
        source_relation,
    )?;
    let created = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::Created { .. }))
        .count();
    let creation = if created == 0 {
        None
    } else {
        Some(task_creation_capture(bus, command, &snapshot)?)
    };
    let updated = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::Updated { .. }))
        .count();
    let reused = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::Reused { .. }))
        .count();
    let cleared = plans
        .iter()
        .filter(|plan| {
            matches!(
                plan.action,
                GateFindingAction::Recovered {
                    ref transition,
                    ..
                } if matches!(transition.kind(), board::TransitionKind::Complete { .. })
            )
        })
        .count();
    let advanced = plans
        .iter()
        .filter(|plan| {
            matches!(
                plan.action,
                GateFindingAction::Recovered {
                    ref transition,
                    ..
                } if matches!(transition.kind(), board::TransitionKind::Advance { .. })
            )
        })
        .count();
    let recovery_pending = plans
        .iter()
        .filter(|plan| matches!(plan.action, GateFindingAction::RecoveryPending { .. }))
        .count();
    Ok(PreparedFindingFold {
        project,
        at,
        until,
        origin,
        creation,
        plans,
        instrument,
        source: dispatch.source.clone(),
        instrument_tasks,
        created,
        updated,
        reused,
        cleared,
        advanced,
        recovery_pending,
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
    if let Some(creation) = &prepared.creation {
        creation.guard(&mut write);
    }
    guard_gate_finding_plan(
        &mut write,
        prepared.plans.clone(),
        prepared.instrument,
        prepared.instrument_tasks.clone(),
    );
    let filed = match write.fact(
        body::GateFindingsFiled {
            report: dispatch.report.clone(),
            findings: findings.len() as u32,
            created: prepared.created as u32,
            reused: prepared.updated.saturating_add(prepared.reused) as u32,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("gate::findings-build", &error.to_string()),
    };
    for plan in &prepared.plans {
        let result = match &plan.action {
            GateFindingAction::Reused { .. } => Ok(()),
            GateFindingAction::Updated {
                task, statement, ..
            } => write_gate_finding_update(&mut write, filed, task, statement),
            GateFindingAction::Created {
                task,
                finding,
                predecessor,
            } => write_gate_finding_task(
                &mut write,
                filed,
                &prepared.project,
                task,
                finding,
                predecessor.as_ref(),
                &prepared.origin,
                prepared.creation.as_ref(),
                prepared.until,
                &dispatch.report,
                instrument,
                &prepared.source,
            ),
            GateFindingAction::Recovered {
                task,
                transition,
                claim,
                source,
                ..
            } => write_gate_finding_recovery(
                &mut write,
                filed,
                task,
                transition,
                claim,
                source,
                &dispatch.report,
                instrument,
                &plan.identity,
            ),
            GateFindingAction::RecoveryPending { .. } => Ok(()),
        };
        if let Err(outcome) = result {
            return outcome;
        }
    }
    let text = format!(
        concat!(
            "gate findings instrument={} state={} report={} findings={} created={} ",
            "updated={} reused={} advanced={} cleared={} recovery_pending={}"
        ),
        instrument,
        if admits { "clean" } else { "findings" },
        dispatch.report,
        findings.len(),
        prepared.created,
        prepared.updated,
        prepared.reused,
        prepared.advanced,
        prepared.cleared,
        prepared.recovery_pending
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

fn validate_focused_source(
    repository: &Repository,
    dispatch: &GateFindings,
    report: &gate::FocusedArmReport,
) -> Result<(), Outcome<CommandAnswer>> {
    if report.context().source() != &dispatch.source.tree {
        return Err(refusal(
            "gate::finding-source-mismatch",
            Some(format!(
                "report={} dispatch={}",
                report.context().source(),
                dispatch.source.tree
            )),
            "dispatch the typed source envelope captured with this focused report",
        ));
    }
    let expected_scope = match dispatch.source.kind {
        wire::intent::GateFindingSourceKind::StagedIndex => "staged-index",
        wire::intent::GateFindingSourceKind::Revision => "revision",
    };
    if report.context().scope().name() != expected_scope {
        return Err(refusal(
            "gate::finding-source-kind-mismatch",
            Some(format!(
                "report={} dispatch={expected_scope}",
                report.context().scope().name()
            )),
            "retain the typed selector kind captured with this focused report",
        ));
    }
    spice::gate::validate_gate_finding_source(repository, &dispatch.source).map_err(|error| {
        refusal(
            error.reason(),
            Some(error.detail().to_owned()),
            "rerun the focused instrument against the current resident repository source",
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
    Ok(spice::claim_lease::indefinite_horizon())
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

#[allow(clippy::too_many_arguments)]
fn plan_gate_findings(
    bus: &Bus,
    findings: &[FindingRow],
    snapshot: &ValidatedSnapshot,
    at: Instant,
    birth_micros: i64,
    instrument: gate::GateId,
    repository: &Repository,
    source: &wire::intent::GateFindingSource,
    report: &Digest,
    actor: &Actor,
    source_relation: &SourceRelation,
) -> Result<Vec<GateFindingPlan>, Outcome<CommandAnswer>> {
    let open_instrument = open_instrument_findings(snapshot, instrument)?;
    let current = findings
        .iter()
        .map(|finding| finding.identity.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut planned = Vec::with_capacity(findings.len().saturating_add(open_instrument.len()));
    let matches = matched_gate_findings(findings, snapshot)?;
    let creation_count = matches
        .iter()
        .filter(|matched| !matched.iter().any(|entry| gate_finding_task_open(entry)))
        .count();
    let mut minted =
        allocate_task_stamps(&snapshot.answer, birth_micros, creation_count)?.into_iter();
    for (finding, matches) in findings.iter().zip(matches) {
        let tag = gate_finding_tag(&finding.identity);
        let open = matches
            .iter()
            .copied()
            .filter(|entry| gate_finding_task_open(entry))
            .collect::<Vec<_>>();
        let predecessor = matches
            .iter()
            .filter(|entry| {
                entry.workflow().status == "completed" || entry.workflow().completion.is_some()
            })
            .max_by_key(|entry| entry.statement().revision)
            .map(|entry| entry.handle().clone());
        let action = if let Some(entry) = open.first() {
            let prior = gate_finding_task_source(entry)?;
            source_relation(repository, &prior, source, false)?;
            let statement = desired_gate_finding_statement(
                entry,
                finding,
                predecessor.as_ref(),
                report,
                instrument,
                source,
            );
            let heads = gate_finding_heads(snapshot, entry)?;
            if statement == *entry.statement() {
                GateFindingAction::Reused {
                    task: entry.born().clone(),
                    heads,
                }
            } else {
                require_gate_finding_mutation_owner(entry, actor)?;
                GateFindingAction::Updated {
                    task: entry.born().clone(),
                    heads,
                    statement,
                }
            }
        } else {
            let task = minted
                .next()
                .expect("every new gate finding owns one allocated task stamp");
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
    for (identity, entry) in open_instrument {
        if current.contains(&identity) {
            continue;
        }
        let prior = gate_finding_task_source(entry)?;
        source_relation(repository, &prior, source, true)?;
        let authority = gate_finding_recovery_authority(bus, entry, actor, at)?;
        let heads = gate_finding_heads(snapshot, entry)?;
        let action = if let Some((claim, source, transition)) = authority {
            GateFindingAction::Recovered {
                task: entry.born().clone(),
                heads,
                transition,
                claim: Box::new(claim),
                source,
            }
        } else {
            GateFindingAction::RecoveryPending {
                task: entry.born().clone(),
                heads,
            }
        };
        planned.push(GateFindingPlan {
            tag: gate_finding_tag(&identity),
            identity,
            action,
        });
    }
    Ok(planned)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::board_command) fn plan_gate_finding_creations(
    bus: &Bus,
    findings: &[FindingRow],
    snapshot: &ValidatedSnapshot,
    birth_micros: i64,
    instrument: gate::GateId,
    repository: &Repository,
    source: &wire::intent::GateFindingSource,
    report: &Digest,
    actor: &Actor,
) -> Result<Vec<(Digest, wire::TaskStamp)>, Outcome<CommandAnswer>> {
    Ok(plan_gate_findings(
        bus,
        findings,
        snapshot,
        current_instant()?,
        birth_micros,
        instrument,
        repository,
        source,
        report,
        actor,
        &require_in_process_source_relation,
    )?
    .into_iter()
    .filter_map(|plan| match plan.action {
        GateFindingAction::Created { task, .. } => Some((plan.identity, task)),
        _ => None,
    })
    .collect())
}

fn matching_gate_finding_tasks<'a>(snapshot: &'a ValidatedSnapshot, tag: &str) -> Vec<&'a Entry> {
    snapshot
        .entries
        .iter()
        .filter(|entry| {
            gate_finding_task(entry)
                && entry
                    .statement()
                    .tags
                    .iter()
                    .any(|candidate| candidate == tag)
        })
        .collect()
}

fn gate_finding_task_identity(entry: &Entry) -> Result<Digest, Outcome<CommandAnswer>> {
    let identities = entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(GATE_FINDING_TAG_PREFIX))
        .collect::<Vec<_>>();
    let [identity] = identities.as_slice() else {
        return Err(refusal(
            "gate::finding-task-malformed",
            Some(entry.handle().to_string()),
            "keep exactly one canonical gate-finding identity tag on system-filed work",
        ));
    };
    identity.parse::<Digest>().map_err(|error| {
        refusal(
            "gate::finding-task-malformed",
            Some(format!("{}: {error}", entry.handle())),
            "keep exactly one canonical gate-finding identity tag on system-filed work",
        )
    })
}

fn gate_finding_task_source(
    entry: &Entry,
) -> Result<wire::intent::GateFindingSource, Outcome<CommandAnswer>> {
    let repository = gate_finding_source_tag(entry, GATE_REPOSITORY_TAG_PREFIX, "repository")?
        .parse::<Digest>()
        .map_err(|error| gate_finding_source_refusal(entry, "repository", &error.to_string()))?;
    let selection = gate_finding_source_tag(entry, GATE_SELECTION_TAG_PREFIX, "selection")?
        .parse::<Digest>()
        .map_err(|error| gate_finding_source_refusal(entry, "selection", &error.to_string()))?;
    let tree = gate_finding_source_tag(entry, GATE_SOURCE_TREE_TAG_PREFIX, "tree")?
        .parse::<Digest>()
        .map_err(|error| gate_finding_source_refusal(entry, "tree", &error.to_string()))?;
    let anchor = gate_finding_source_tag(entry, GATE_SOURCE_ANCHOR_TAG_PREFIX, "anchor")?;
    let kind = match gate_finding_source_tag(entry, GATE_SOURCE_KIND_TAG_PREFIX, "kind")? {
        "staged-index" => wire::intent::GateFindingSourceKind::StagedIndex,
        "revision" => wire::intent::GateFindingSourceKind::Revision,
        observed => {
            return Err(gate_finding_source_refusal(
                entry,
                "kind",
                &format!("unrecognized value {observed:?}"),
            ));
        }
    };
    let mainline = gate_finding_mainline(entry)?;
    Ok(wire::intent::GateFindingSource {
        repository,
        selection,
        tree,
        anchor: anchor.to_owned(),
        kind,
        mainline: Some(mainline),
    })
}

fn gate_finding_mainline(
    entry: &Entry,
) -> Result<wire::intent::GateFindingMainline, Outcome<CommandAnswer>> {
    let reference =
        gate_finding_optional_source_tag(entry, GATE_MAINLINE_REF_TAG_PREFIX, "mainline-ref")?;
    let merge_base =
        gate_finding_optional_source_tag(entry, GATE_MAINLINE_BASE_TAG_PREFIX, "mainline-base")?;
    match (reference, merge_base) {
        (None, None) => Err(gate_finding_source_refusal(
            entry,
            "mainline",
            "missing gate-finding-mainline-v1 lineage tags",
        )),
        (Some(reference), Some(merge_base)) => Ok(wire::intent::GateFindingMainline {
            reference: reference.to_owned(),
            merge_base: merge_base.to_owned(),
        }),
        _ => Err(gate_finding_source_refusal(
            entry,
            "mainline",
            "expected both canonical lineage tags or neither",
        )),
    }
}

fn gate_finding_source_tag<'a>(
    entry: &'a Entry,
    prefix: &str,
    label: &str,
) -> Result<&'a str, Outcome<CommandAnswer>> {
    let values = entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(prefix))
        .collect::<Vec<_>>();
    let [value] = values.as_slice() else {
        return Err(gate_finding_source_refusal(
            entry,
            label,
            &format!("found {} canonical tags; expected one", values.len()),
        ));
    };
    Ok(value)
}

fn gate_finding_optional_source_tag<'a>(
    entry: &'a Entry,
    prefix: &str,
    label: &str,
) -> Result<Option<&'a str>, Outcome<CommandAnswer>> {
    let values = entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(prefix))
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [value] if !value.is_empty() => Ok(Some(*value)),
        _ => Err(gate_finding_source_refusal(
            entry,
            label,
            &format!(
                "found {} canonical tags; expected at most one",
                values.len()
            ),
        )),
    }
}

fn gate_finding_source_refusal(entry: &Entry, field: &str, detail: &str) -> Outcome<CommandAnswer> {
    refusal(
        "gate::finding-source-migration-required",
        Some(format!("{} source {field}: {detail}", entry.handle())),
        "recreate legacy repair work from a current typed focused report before reconciliation",
    )
}

fn require_gate_finding_source_relation(
    repository: &Repository,
    prior: &wire::intent::GateFindingSource,
    candidate: &wire::intent::GateFindingSource,
    recovery: bool,
) -> Result<(), Outcome<CommandAnswer>> {
    spice::gate::require_gate_finding_source_forward(repository, prior, candidate, recovery)
        .map_err(|error| {
            refusal(
                error.reason(),
                Some(error.detail().to_owned()),
                "rerun the exact instrument from the repair repository at a current descendant source",
            )
        })
}

#[cfg(test)]
fn require_in_process_source_relation(
    _repository: &Repository,
    prior: &wire::intent::GateFindingSource,
    candidate: &wire::intent::GateFindingSource,
    recovery: bool,
) -> Result<(), Outcome<CommandAnswer>> {
    if prior.repository != candidate.repository {
        return Err(refusal(
            "gate::finding-repository-mismatch",
            None,
            "keep in-process finding evidence within one repository authority",
        ));
    }
    if recovery && prior.tree == candidate.tree {
        return Err(refusal(
            "gate::finding-recovery-unproven",
            None,
            "move the selected source before reporting a finding as recovered",
        ));
    }
    Ok(())
}

fn gate_finding_task(entry: &Entry) -> bool {
    entry.statement().project == GATE_FINDINGS_PROJECT
        && entry
            .statement()
            .tags
            .iter()
            .any(|tag| tag == GATE_FINDING_TAG)
}

fn require_gate_finding_mutation_owner(
    entry: &Entry,
    actor: &Actor,
) -> Result<(), Outcome<CommandAnswer>> {
    if let Some(claim) = entry.claim()
        && claim.disposition == ClaimDisposition::Active
        && &claim.actor != actor
    {
        return Err(refusal(
            "gate::finding-owned",
            Some(format!("{} held by {}", entry.handle(), claim.actor)),
            "rerun the exact focused instrument through the lane that owns this repair task",
        ));
    }
    Ok(())
}

fn gate_finding_recovery_authority(
    bus: &Bus,
    entry: &Entry,
    actor: &Actor,
    at: Instant,
) -> Result<Option<(ClaimHead, body::SourceBoundary, board::Transition)>, Outcome<CommandAnswer>> {
    let Ok(active) = board::Active::admit(entry, actor, at) else {
        return Ok(None);
    };
    let source = bus.source_boundary(&active.lane()).map_err(|error| {
        store_fault(
            "gate::finding-recovery-boundary-unreadable",
            &error.to_string(),
        )
    })?;
    let claim = entry
        .claim()
        .expect("active recovery authority carries a claim")
        .clone();
    let current = entry
        .workflow()
        .flow
        .iter()
        .position(|phase| phase == &entry.workflow().phase)
        .expect("validated gate finding workflow contains its current phase");
    let transition = match entry.workflow().flow.get(current + 1) {
        Some(next) => board::Transition::advance(entry, &active, next.clone()),
        None => board::Transition::complete(entry, &active, at),
    }
    .map_err(|error| {
        refusal(
            "gate::finding-recovery-phase-refused",
            Some(format!("{}: {error}", entry.handle())),
            "recover only the current claimed phase of this repair task",
        )
    })?;
    Ok(Some((claim, source, transition)))
}

fn gate_finding_heads(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
) -> Result<GateFindingHeads, Outcome<CommandAnswer>> {
    let heads = snapshot
        .answer
        .tasks
        .iter()
        .find(|task| task.statement.body.born == *entry.born())
        .ok_or_else(|| {
            protocol_fault(
                "gate::finding-snapshot-incoherent",
                "validated board snapshot lost a gate finding task",
            )
        })?;
    Ok(GateFindingHeads {
        statement: heads.statement.position,
        workflow: heads.workflow.position,
        claim: heads.claim.as_ref().map(|claim| claim.position),
    })
}

fn gate_finding_task_open(entry: &Entry) -> bool {
    entry.workflow().status == "pending" && entry.workflow().completion.is_none()
}

fn matched_gate_findings<'a>(
    findings: &[FindingRow],
    snapshot: &'a ValidatedSnapshot,
) -> Result<Vec<Vec<&'a Entry>>, Outcome<CommandAnswer>> {
    findings
        .iter()
        .map(|finding| {
            let matched =
                matching_gate_finding_tasks(snapshot, &gate_finding_tag(&finding.identity));
            if matched
                .iter()
                .filter(|entry| gate_finding_task_open(entry))
                .count()
                > 1
            {
                return Err(refusal(
                    "gate::finding-ambiguous",
                    Some(finding.identity.to_string()),
                    "settle duplicate open gate finding tasks before redispatching",
                ));
            }
            Ok(matched)
        })
        .collect()
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
    creation: Option<&TaskCreationCapture>,
    until: Instant,
    report: &Digest,
    instrument: gate::GateId,
    source: &wire::intent::GateFindingSource,
) -> Result<(), Outcome<CommandAnswer>> {
    let creation = creation.ok_or_else(|| {
        protocol_fault(
            "gate::finding-creation-context-missing",
            "a new gate finding task has no creation-time transcript capture",
        )
    })?;
    let flow = phases(Some("todo,review"))?;
    let phase = flow[0].clone();
    let description = gate_finding_description(finding, predecessor, report, instrument, source);
    let tags = gate_finding_tags(&[], finding, instrument, source);
    let added = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                title: finding.title.clone(),
                description: Some(description.clone()),
                acceptance: finding.acceptance.clone(),
                requirements: wire::EnvironmentRequirements::default(),
                project: project.as_str().to_owned(),
                tags: tags.clone(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: phase.clone(),
                revision: 1,
                creation: creation.context(),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("gate::finding-task-build", &error.to_string()))?;
    let statement = TaskHead {
        born: task.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: finding.title.clone(),
        description: Some(description),
        acceptance: finding.acceptance.clone(),
        requirements: wire::EnvironmentRequirements::default(),
        project: project.as_str().to_owned(),
        tags,
        dependencies: Vec::new(),
        provenance: origin.clone(),
        creation: creation.context(),
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
    instrument: gate::GateId,
    source: &wire::intent::GateFindingSource,
) -> String {
    let mut description = format!(
        concat!(
            "{}\n\nGate report: {report}\nFinding identity: {}\n",
            "Instrument: {instrument}\nAffected authority: {}\nSeverity: {}\nLocus: {}\n",
            "Repository authority: {}\nSelected tree: {}\nSource anchor: {}\nSource kind: {}"
        ),
        finding.description,
        finding.identity,
        finding.authority,
        finding.severity,
        finding.locus,
        source.repository,
        source.tree,
        source.anchor,
        gate_finding_source_kind(source.kind),
        report = report,
        instrument = instrument,
    );
    if let Some(mainline) = &source.mainline {
        description.push_str(&format!(
            "\nMainline reference: {}\nMainline merge base: {}",
            mainline.reference, mainline.merge_base,
        ));
    }
    if let Some(predecessor) = predecessor {
        description.push_str(&format!("\nPrevious completed task: {predecessor}"));
    }
    description
}

fn desired_gate_finding_statement(
    entry: &Entry,
    finding: &FindingRow,
    predecessor: Option<&TaskHandle>,
    report: &Digest,
    instrument: gate::GateId,
    source: &wire::intent::GateFindingSource,
) -> TaskHead {
    TaskHead {
        title: finding.title.clone(),
        description: Some(gate_finding_description(
            finding,
            predecessor,
            report,
            instrument,
            source,
        )),
        acceptance: finding.acceptance.clone(),
        tags: gate_finding_tags(&entry.statement().tags, finding, instrument, source),
        ..entry.statement().clone()
    }
}

fn gate_finding_tags(
    existing: &[String],
    finding: &FindingRow,
    instrument: gate::GateId,
    source: &wire::intent::GateFindingSource,
) -> Vec<String> {
    let mut tags = vec![
        GATE_FINDING_TAG.to_owned(),
        gate_finding_tag(&finding.identity),
        gate_instrument_tag(instrument),
        format!("{GATE_SEVERITY_TAG_PREFIX}{}", finding.severity),
        format!("{GATE_REPOSITORY_TAG_PREFIX}{}", source.repository),
        format!("{GATE_SELECTION_TAG_PREFIX}{}", source.selection),
        format!("{GATE_SOURCE_TREE_TAG_PREFIX}{}", source.tree),
        format!("{GATE_SOURCE_ANCHOR_TAG_PREFIX}{}", source.anchor),
        format!(
            "{GATE_SOURCE_KIND_TAG_PREFIX}{}",
            gate_finding_source_kind(source.kind)
        ),
    ];
    if let Some(mainline) = &source.mainline {
        tags.extend([
            format!("{GATE_MAINLINE_REF_TAG_PREFIX}{}", mainline.reference),
            format!("{GATE_MAINLINE_BASE_TAG_PREFIX}{}", mainline.merge_base),
        ]);
    }
    tags.extend(
        existing
            .iter()
            .filter(|tag| {
                tag.as_str() != GATE_FINDING_TAG
                    && !tag.starts_with(GATE_FINDING_TAG_PREFIX)
                    && !tag.starts_with(GATE_INSTRUMENT_TAG_PREFIX)
                    && !tag.starts_with(GATE_SEVERITY_TAG_PREFIX)
                    && !tag.starts_with(GATE_REPOSITORY_TAG_PREFIX)
                    && !tag.starts_with(GATE_SELECTION_TAG_PREFIX)
                    && !tag.starts_with(GATE_SOURCE_TREE_TAG_PREFIX)
                    && !tag.starts_with(GATE_SOURCE_ANCHOR_TAG_PREFIX)
                    && !tag.starts_with(GATE_SOURCE_KIND_TAG_PREFIX)
                    && !tag.starts_with(GATE_MAINLINE_REF_TAG_PREFIX)
                    && !tag.starts_with(GATE_MAINLINE_BASE_TAG_PREFIX)
            })
            .cloned(),
    );
    tags
}

const fn gate_finding_source_kind(kind: wire::intent::GateFindingSourceKind) -> &'static str {
    match kind {
        wire::intent::GateFindingSourceKind::StagedIndex => "staged-index",
        wire::intent::GateFindingSourceKind::Revision => "revision",
    }
}

fn write_gate_finding_update(
    write: &mut JournalWrite,
    cause: PendingFact,
    task: &wire::TaskStamp,
    statement: &TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    let revision = u32::try_from(statement.revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                task: task.clone(),
                revision,
                notice: None,
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("gate::finding-update-build", &error.to_string()))?;
    let statement = statement.clone();
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_gate_finding_recovery(
    write: &mut JournalWrite,
    cause: PendingFact,
    task: &wire::TaskStamp,
    transition: &board::Transition,
    claim: &ClaimHead,
    source: &body::SourceBoundary,
    report: &Digest,
    instrument: gate::GateId,
    identity: &Digest,
) -> Result<(), Outcome<CommandAnswer>> {
    let validation =
        format!("Focused {instrument} report {report} no longer projects gate finding {identity}.");
    let done = write
        .fact(
            body::TaskDone {
                task: task.clone(),
                validation,
                terminal: None,
                git: None,
                source: source.clone(),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("gate::finding-recovery-build", &error.to_string()))?;
    let workflow_fact = match transition.kind() {
        board::TransitionKind::Advance { to, .. } => write
            .fact(
                body::TaskPhased {
                    task: task.clone(),
                    phase: to.clone(),
                },
                FactCause::Pending(done),
            )
            .map_err(|error| store_fault("gate::finding-phase-build", &error.to_string()))?,
        board::TransitionKind::Complete { .. } => done,
    };
    let workflow = transition.next().clone();
    write.workflow_head(task.clone(), workflow_fact, move |_| workflow);
    let mut claim = claim.clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(task.clone(), done, move |fact| ClaimHead { fact, ..claim });
    Ok(())
}

fn guard_gate_finding_plan(
    write: &mut JournalWrite,
    plans: Vec<GateFindingPlan>,
    instrument: gate::GateId,
    instrument_tasks: std::collections::BTreeSet<wire::TaskStamp>,
) {
    write.guard(JournalGuard::new(move |read| {
        let page = read.task_head_page(None, journal::FactReadBudget::RETAINED)?;
        if let Some(continuation) = page.continuation {
            return Err(GuardError::refused(format!(
                "gate finding guard requires task continuation after {continuation}"
            )));
        }
        let heads = page.rows;
        if current_open_instrument_tasks(&heads, instrument)? != instrument_tasks {
            return Err(GuardError::refused(
                "gate::finding-instrument-moved".to_owned(),
            ));
        }
        for plan in &plans {
            match &plan.action {
                GateFindingAction::Reused {
                    task,
                    heads: observed,
                }
                | GateFindingAction::Updated {
                    task,
                    heads: observed,
                    ..
                }
                | GateFindingAction::Recovered {
                    task,
                    heads: observed,
                    ..
                }
                | GateFindingAction::RecoveryPending {
                    task,
                    heads: observed,
                } => {
                    let current = heads
                        .iter()
                        .find(|candidate| candidate.statement().body().born == *task);
                    if current.is_none_or(|current| {
                        current.statement().position() != observed.statement
                            || current.workflow().position() != observed.workflow
                            || current.claim().map(journal::JournalHead::position) != observed.claim
                    }) {
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
        .map(|heads| heads.statement().body().born.clone())
        .collect()
}

fn gate_finding_tag(identity: &Digest) -> String {
    format!("{GATE_FINDING_TAG_PREFIX}{identity}")
}

fn gate_instrument_tag(instrument: gate::GateId) -> String {
    format!("{GATE_INSTRUMENT_TAG_PREFIX}{instrument}")
}
