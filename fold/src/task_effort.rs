//! Replayable claim-to-terminal task effort.
//!
//! Facts are consumed once, in journal order. Task lifecycle facts construct
//! phase attempts; transcript batches prove the byte ranges inside their
//! recorded source boundaries; lane session and lineage facts attribute those
//! ranges. Anything the evidence cannot prove stays visible as an unavailable
//! row and never contributes to an aggregate.

use std::collections::BTreeMap;

use wire::body::{self, SourceBoundary, TranscriptPosition};
use wire::intent::{EffortAnswer, EffortPhaseAnswer, EffortUnavailableCode};
use wire::{FactKind, Instant, Seq, TaskHandle};

use crate::{FactPlane, FoldId, Materialization, Reducer, Reduction, Source};

mod accounting;
mod state;
mod support;

use accounting::{Evidence, aggregate, finish};
use state::{CarryCut, Closing, OpenPhase, TaskState};
use support::{epoch, first_locus, push_reason, read, reason};

#[cfg(test)]
use support::zero_usage;
#[cfg(test)]
use wire::intent::{EffortAvailability, EffortIdentity, EffortUnavailable, EffortUsage};
#[cfg(test)]
use wire::{CanonicalUuid, Name, Phase};

const ORIGIN: TranscriptPosition = TranscriptPosition {
    member: 1,
    offset: 0,
};
const REDUCER_VERSION: u32 = 1;
const SOURCES: &[Source] = &[
    Source::Facts(FactPlane::Task),
    Source::Facts(FactPlane::Claim),
    Source::Facts(FactPlane::Lane),
    Source::Facts(FactPlane::Lineage),
    Source::Facts(FactPlane::Journal),
];

/// The sole reducer for `task::effort`.
#[derive(Debug, Default)]
pub struct TaskEffortReducer;

impl Reducer for TaskEffortReducer {
    fn fold(&self) -> FoldId {
        FoldId::TaskEffort
    }

    fn sources(&self) -> &'static [Source] {
        SOURCES
    }

    fn version(&self) -> u32 {
        REDUCER_VERSION
    }

    fn rebuild(&self, facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        let answer = derive(facts)?;
        let payload = serde_json::to_value(answer).map_err(|error| error.to_string())?;
        Ok(Reduction::complete(facts, payload))
    }
}

/// Decode one materialization as the task-effort answer its fold owns.
pub fn task_effort_answer(materialization: &Materialization) -> Result<EffortAnswer, String> {
    if materialization.fold != FoldId::TaskEffort {
        return Err(format!(
            "materialization {} is not task::effort",
            materialization.fold
        ));
    }
    if materialization.reducer_version != REDUCER_VERSION {
        return Err(format!(
            "task::effort materialization version {} is not reducer version {REDUCER_VERSION}",
            materialization.reducer_version
        ));
    }
    serde_json::from_value(materialization.payload().clone()).map_err(|error| error.to_string())
}

/// Apply the typed read filter after rebuilding the one complete materialization.
pub fn select_task_effort(
    mut answer: EffortAnswer,
    read: &wire::intent::EffortRead,
) -> Result<EffortAnswer, String> {
    if let Some(task) = &read.task {
        answer.phases.retain(|phase| &phase.task == task);
        answer.aggregates = aggregate(&answer.phases)?;
    }
    Ok(answer)
}

fn derive(facts: &journal::FactSnapshot) -> Result<EffortAnswer, String> {
    let evidence = Evidence::collect(facts);
    let mut tasks = BTreeMap::<TaskHandle, TaskState>::new();
    let mut phases = Vec::new();

    for envelope in facts.facts() {
        apply_lifecycle(envelope, &mut tasks, &mut phases, &evidence);
    }

    close_remaining(facts, &mut tasks, &mut phases, &evidence);
    phases.sort_by(phase_order);
    let aggregates = aggregate(&phases)?;
    Ok(EffortAnswer { phases, aggregates })
}

fn apply_lifecycle(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
    phases: &mut Vec<EffortPhaseAnswer>,
    evidence: &Evidence,
) {
    match envelope.fact.kind() {
        FactKind::TaskAdded => record_task_added(envelope, tasks),
        FactKind::TaskPhased => record_task_phased(envelope, tasks),
        FactKind::ClaimWon => record_claim_won(envelope, tasks, phases, evidence),
        FactKind::ClaimRenewed => record_claim_renewed(envelope, tasks),
        FactKind::ClaimCarried => record_claim_carried(envelope, tasks),
        FactKind::TaskDone => {
            let body: body::TaskDone = read(envelope);
            close_terminal(
                tasks,
                phases,
                body.task,
                envelope.seq,
                envelope.at,
                body.source,
                evidence,
            );
        }
        FactKind::TaskReviewed => {
            let body: body::TaskReviewed = read(envelope);
            close_terminal(
                tasks,
                phases,
                body.task,
                envelope.seq,
                envelope.at,
                body.source,
                evidence,
            );
        }
        FactKind::ClaimFreed => {
            let body: body::ClaimFreed = read(envelope);
            interrupt(
                tasks,
                phases,
                &body.task,
                envelope.seq,
                envelope.at,
                EffortUnavailableCode::ClaimFreed,
                "claim was freed",
                evidence,
            );
        }
        FactKind::ClaimLapsed => {
            let body: body::ClaimLapsed = read(envelope);
            interrupt(
                tasks,
                phases,
                &body.task,
                envelope.seq,
                envelope.at,
                EffortUnavailableCode::ClaimLapsed,
                "claim lapsed",
                evidence,
            );
        }
        FactKind::TaskDeleted => {
            let body: body::TaskDeleted = read(envelope);
            interrupt(
                tasks,
                phases,
                &body.task,
                envelope.seq,
                envelope.at,
                EffortUnavailableCode::TaskDeleted,
                "task was deleted",
                evidence,
            );
        }
        _ => {}
    }
}

fn record_task_added(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
) {
    let body: body::TaskAdded = read(envelope);
    tasks.insert(
        body.task,
        TaskState {
            phase: body.phase,
            phase_index: 0,
            phase_index_overflow: false,
            open: None,
        },
    );
}

fn record_task_phased(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
) {
    let body: body::TaskPhased = read(envelope);
    let state = tasks.entry(body.task).or_insert_with(TaskState::unknown);
    state.phase = body.phase;
    match state.phase_index.checked_add(1) {
        Some(next) => state.phase_index = next,
        None => state.phase_index_overflow = true,
    }
}

fn record_claim_won(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
    phases: &mut Vec<EffortPhaseAnswer>,
    evidence: &Evidence,
) {
    let body: body::ClaimWon = read(envelope);
    let state = tasks
        .entry(body.task.clone())
        .or_insert_with(TaskState::unknown);
    if let Some(previous) = state.open.take() {
        phases.push(finish(
            previous,
            None,
            Some(reason(
                EffortUnavailableCode::ClaimStolen,
                format!("a new ClaimWon arrived at {}", envelope.seq),
            )),
            envelope.seq,
            envelope.at,
            evidence,
        ));
    }
    let lineage = evidence.lineage_at(body.lane, envelope.seq);
    let mut reasons = Vec::new();
    if state.phase_index_overflow {
        push_reason(
            &mut reasons,
            EffortUnavailableCode::ArithmeticOverflow,
            "task phase index overflowed u32",
        );
    }
    let lineage = lineage.unwrap_or_else(|| {
        push_reason(
            &mut reasons,
            EffortUnavailableCode::LineageDisagreement,
            format!("lane {} had no lineage at {}", body.lane, envelope.seq),
        );
        body.lane
    });
    state.open = Some(OpenPhase {
        task: body.task,
        phase: state.phase.clone(),
        phase_index: state.phase_index,
        opening: envelope.seq,
        opened_at: envelope.at,
        opening_boundary: body.source,
        lane: body.lane,
        occupant: body.occupant,
        lineage,
        renewals: 0,
        carries: Vec::new(),
        reasons,
    });
}

fn record_claim_renewed(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
) {
    let body: body::ClaimRenewed = read(envelope);
    if let Some(open) = tasks
        .get_mut(&body.task)
        .and_then(|state| state.open.as_mut())
    {
        match open.renewals.checked_add(1) {
            Some(next) => open.renewals = next,
            None => push_reason(
                &mut open.reasons,
                EffortUnavailableCode::ArithmeticOverflow,
                "claim renewal count overflowed u32",
            ),
        }
    }
}

fn record_claim_carried(
    envelope: &wire::Envelope<wire::AnyFact>,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
) {
    let body: body::ClaimCarried = read(envelope);
    let Some(open) = tasks
        .get_mut(&body.task)
        .and_then(|state| state.open.as_mut())
    else {
        return;
    };
    let expected_ancestor = open
        .carries
        .last()
        .map_or(open.occupant, |carry| carry.occupant);
    if body.lane != open.lane || body.ancestor != expected_ancestor {
        push_reason(
            &mut open.reasons,
            EffortUnavailableCode::BrokenCarry,
            format!(
                concat!(
                    "carry at {} names lane {} ancestor {}, ",
                    "expected lane {} occupant {}"
                ),
                envelope.seq, body.lane, body.ancestor, open.lane, expected_ancestor
            ),
        );
    }
    open.carries.push(CarryCut {
        seq: envelope.seq,
        boundary: body.source,
        ancestor: body.ancestor,
        occupant: body.occupant,
    });
}

fn close_remaining(
    facts: &journal::FactSnapshot,
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
    phases: &mut Vec<EffortPhaseAnswer>,
    evidence: &Evidence,
) {
    let terminal_seq = facts
        .horizon()
        .unwrap_or_else(|| Seq::new(1).expect("one is nonzero"));
    let terminal_at = facts
        .facts()
        .last()
        .map_or_else(epoch, |envelope| envelope.at);
    for state in tasks.values_mut() {
        if let Some(open) = state.open.take() {
            phases.push(finish(
                open,
                None,
                None,
                terminal_seq,
                terminal_at,
                evidence,
            ));
        }
    }
}

fn phase_order(left: &EffortPhaseAnswer, right: &EffortPhaseAnswer) -> std::cmp::Ordering {
    (
        left.opening,
        &left.task,
        left.phase_index,
        first_locus(left),
    )
        .cmp(&(
            right.opening,
            &right.task,
            right.phase_index,
            first_locus(right),
        ))
}

fn close_terminal(
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
    phases: &mut Vec<EffortPhaseAnswer>,
    task: TaskHandle,
    seq: Seq,
    at: Instant,
    boundary: SourceBoundary,
    evidence: &Evidence,
) {
    let state = tasks.entry(task.clone()).or_insert_with(TaskState::unknown);
    if let Some(open) = state.open.take() {
        phases.push(finish(
            open,
            Some(Closing { seq, at, boundary }),
            None,
            seq,
            at,
            evidence,
        ));
    } else {
        let mut open = OpenPhase::missing(task, state.phase.clone(), state.phase_index, seq, at);
        push_reason(
            &mut open.reasons,
            EffortUnavailableCode::MissingStart,
            format!("terminal fact {seq} had no opening claim"),
        );
        phases.push(finish(
            open,
            Some(Closing { seq, at, boundary }),
            None,
            seq,
            at,
            evidence,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn interrupt(
    tasks: &mut BTreeMap<TaskHandle, TaskState>,
    phases: &mut Vec<EffortPhaseAnswer>,
    task: &TaskHandle,
    seq: Seq,
    at: Instant,
    code: EffortUnavailableCode,
    detail: &str,
    evidence: &Evidence,
) {
    if let Some(open) = tasks.get_mut(task).and_then(|state| state.open.take()) {
        phases.push(finish(
            open,
            None,
            Some(reason(code, format!("{detail} at {seq}"))),
            seq,
            at,
            evidence,
        ));
    }
}

#[cfg(test)]
#[path = "task_effort_tests.rs"]
mod tests;
