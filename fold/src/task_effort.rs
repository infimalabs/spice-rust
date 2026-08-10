//! Replayable claim-to-terminal task effort.
//!
//! Facts are consumed once, in journal order. Task lifecycle facts construct
//! phase attempts; transcript batches prove the byte ranges inside their
//! recorded source boundaries; lane session and lineage facts attribute those
//! ranges. Anything the evidence cannot prove stays visible as an unavailable
//! row and never contributes to an aggregate.

use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use wire::Phase;
use wire::body::{self, SourceBoundary, TranscriptPosition};
use wire::intent::{
    EffortAnswer, EffortAvailability, EffortIdentity, EffortPhaseAnswer, EffortSegmentAnswer,
    EffortUnavailable, EffortUnavailableCode, EffortUsage,
};
use wire::{CanonicalUuid, FactKind, Instant, Seq, TaskHandle};

use crate::{FactPlane, FoldId, Materialization, Reducer, Reduction, Source};

#[path = "task_effort_aggregate.rs"]
mod aggregate;

use aggregate::aggregate;

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
    let lineage = evidence.lineages.at(body.lane, envelope.seq);
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

fn finish(
    open: OpenPhase,
    closing: Option<Closing>,
    interruption: Option<EffortUnavailable>,
    terminal_seq: Seq,
    terminal_at: Instant,
    evidence: &Evidence,
) -> EffortPhaseAnswer {
    let mut reasons = open.reasons.clone();
    if let Some(interruption) = interruption {
        reasons.push(interruption);
    }
    let closing_boundary = closing.as_ref().map(|closing| closing.boundary.clone());
    let closing_seq = closing.as_ref().map(|closing| closing.seq);
    let end_seq = closing_seq.unwrap_or(terminal_seq);
    let end_at = closing.as_ref().map_or(terminal_at, |closing| closing.at);
    let mut segments = Vec::new();

    if let Some(closing) = &closing {
        let cuts = cuts(&open, closing, evidence, &mut reasons);
        for pair in cuts.windows(2) {
            segments.extend(segments_between(
                &pair[0],
                &pair[1],
                open.lane,
                evidence,
                &mut reasons,
            ));
        }
    } else {
        // The cut list is what reads each boundary, and there is none to build
        // without a closing fact, so the opening bound is recorded here. This
        // is the only path that reads it directly: once a cut list exists the
        // opening is its first cut and recording it here as well would leave
        // one bound checked twice, where severing either check still withholds.
        record_incomplete_boundary(&open.opening_boundary, open.opening, &mut reasons);
        push_reason(
            &mut reasons,
            EffortUnavailableCode::MissingEnd,
            format!(
                "phase opened at {} has no terminal source boundary",
                open.opening
            ),
        );
    }

    let identity = one_identity(&segments, &open, end_seq, evidence, &mut reasons);
    let usage = sum_segments(&segments, &mut reasons);
    let wall_ms = wall_millis(open.opened_at, end_at, &mut reasons);
    reasons.sort_by(|left, right| (left.code, &left.detail).cmp(&(right.code, &right.detail)));
    reasons.dedup_by(|left, right| left.code == right.code && left.detail == right.detail);

    let availability = match (reasons.is_empty(), identity, usage, wall_ms) {
        (true, Some(identity), Some(usage), Some(wall_ms)) => EffortAvailability::Available {
            identity,
            usage,
            wall_ms,
        },
        _ => EffortAvailability::Unavailable { reasons },
    };
    EffortPhaseAnswer {
        task: open.task,
        phase: open.phase,
        phase_index: open.phase_index,
        opening: open.opening,
        closing: closing_seq,
        renewals: open.renewals,
        opening_boundary: open.opening_boundary,
        closing_boundary,
        segments,
        availability,
    }
}

fn record_incomplete_boundary(
    boundary: &SourceBoundary,
    seq: Seq,
    reasons: &mut Vec<EffortUnavailable>,
) {
    if let SourceBoundary::Incomplete { reason: detail } = boundary {
        push_reason(
            reasons,
            EffortUnavailableCode::IncompleteBoundary,
            format!("boundary at {seq}: {detail}"),
        );
    }
}

fn cuts(
    open: &OpenPhase,
    closing: &Closing,
    evidence: &Evidence,
    reasons: &mut Vec<EffortUnavailable>,
) -> Vec<Cut> {
    let mut events = open
        .carries
        .iter()
        .map(|carry| CutEvent::Carry(carry.clone()))
        .chain(
            evidence
                .lineages
                .between(open.lane, open.opening, closing.seq)
                .into_iter()
                .map(CutEvent::Membership),
        )
        .collect::<Vec<_>>();
    events.sort_by_key(CutEvent::seq);

    let mut occupant = open.occupant;
    let mut lineage = open.lineage;
    let mut cuts = vec![Cut {
        seq: open.opening,
        boundary: open.opening_boundary.clone(),
        occupant,
        lineage,
    }];
    for event in events {
        match event {
            CutEvent::Carry(carry) => {
                if carry.ancestor != occupant {
                    push_reason(
                        reasons,
                        EffortUnavailableCode::OccupantDisagreement,
                        format!(
                            "carry at {} expected ancestor {}, found {}",
                            carry.seq, occupant, carry.ancestor
                        ),
                    );
                }
                occupant = carry.occupant;
                lineage = evidence
                    .lineages
                    .at(open.lane, carry.seq)
                    .unwrap_or(lineage);
                cuts.push(Cut {
                    seq: carry.seq,
                    boundary: carry.boundary,
                    occupant,
                    lineage,
                });
            }
            CutEvent::Membership(member) => {
                lineage = member.lineage;
                cuts.push(Cut {
                    seq: member.seq,
                    boundary: member.boundary,
                    occupant,
                    lineage,
                });
            }
        }
    }
    cuts.push(Cut {
        seq: closing.seq,
        boundary: closing.boundary.clone(),
        occupant,
        lineage,
    });
    cuts
}

fn segments_between(
    left: &Cut,
    right: &Cut,
    lane: CanonicalUuid,
    evidence: &Evidence,
    reasons: &mut Vec<EffortUnavailable>,
) -> Vec<EffortSegmentAnswer> {
    let Some(left_heads) = heads(&left.boundary, left.seq, reasons) else {
        return Vec::new();
    };
    let Some(right_heads) = heads(&right.boundary, right.seq, reasons) else {
        return Vec::new();
    };
    let Some(left_by_key) = keyed(left_heads, left.seq, reasons) else {
        return Vec::new();
    };
    let Some(right_by_key) = keyed(right_heads, right.seq, reasons) else {
        return Vec::new();
    };
    let keys = left_by_key
        .keys()
        .chain(right_by_key.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut segments = Vec::new();

    for key in keys {
        let left_head = left_by_key.get(&key);
        let Some(right_head) = right_by_key.get(&key) else {
            push_reason(
                reasons,
                EffortUnavailableCode::TranscriptMissing,
                format!("stream {key} disappeared before {}", right.seq),
            );
            continue;
        };
        let (source, from) = match left_head {
            Some(head) if head.source == right_head.source => (head.source, head.next),
            Some(head) if evidence.supersedes_between(right_head, head, left.seq, right.seq) => {
                (right_head.source, ORIGIN)
            }
            Some(head) => {
                push_reason(
                    reasons,
                    EffortUnavailableCode::SourceReplaced,
                    format!(
                        "stream {key} changed generation from {} to {}",
                        head.source, right_head.source
                    ),
                );
                continue;
            }
            None => (right_head.source, ORIGIN),
        };
        if position(right_head.next) < position(from) {
            push_reason(
                reasons,
                EffortUnavailableCode::SourceGap,
                format!("stream {key} retreated across the phase boundary"),
            );
            continue;
        }
        segments.extend(evidence.account(
            AccountRequest {
                lane,
                occupant: left.occupant,
                lineage: left.lineage,
                vendor: &right_head.vendor,
                stream: &right_head.stream,
                source,
                from,
                to: right_head.next,
                after: left.seq,
                before: right.seq,
            },
            reasons,
        ));
    }
    segments
}

fn heads<'a>(
    boundary: &'a SourceBoundary,
    seq: Seq,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<&'a [wire::TranscriptSourceHead]> {
    match boundary {
        SourceBoundary::Proven { heads } => Some(heads),
        SourceBoundary::Incomplete { reason: detail } => {
            push_reason(
                reasons,
                EffortUnavailableCode::IncompleteBoundary,
                format!("boundary at {seq}: {detail}"),
            );
            None
        }
    }
}

fn keyed<'a>(
    heads: &'a [wire::TranscriptSourceHead],
    seq: Seq,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<BTreeMap<String, &'a wire::TranscriptSourceHead>> {
    let keys = heads
        .iter()
        .map(|head| head.key().as_str().to_owned())
        .collect::<Vec<_>>();
    if !keys.windows(2).all(|pair| pair[0] < pair[1]) {
        push_reason(
            reasons,
            EffortUnavailableCode::SourceGap,
            format!("boundary at {seq} has duplicate or unordered stream heads"),
        );
        return None;
    }
    Some(keys.into_iter().zip(heads).collect())
}

fn one_identity(
    segments: &[EffortSegmentAnswer],
    open: &OpenPhase,
    before: Seq,
    evidence: &Evidence,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<EffortIdentity> {
    if segments.iter().any(|segment| segment.session.is_none()) {
        push_reason(
            reasons,
            EffortUnavailableCode::SessionDisagreement,
            "at least one source segment has no exact session declaration",
        );
    }
    let mut identities = segments
        .iter()
        .filter_map(|segment| segment.identity.clone())
        .collect::<BTreeSet<_>>();
    if segments.iter().any(|segment| segment.identity.is_none()) {
        push_reason(
            reasons,
            EffortUnavailableCode::UntaggedLaunch,
            "at least one source segment has no complete vendor/model/effort declaration",
        );
    }
    let mut occupant = open.occupant;
    for (occupant_end, next_occupant) in open
        .carries
        .iter()
        .map(|carry| (carry.seq, Some(carry.occupant)))
        .chain(std::iter::once((before, None)))
    {
        match evidence.latest_session(open.lane, occupant, None, occupant_end) {
            Some(session) => match session.identity() {
                Some(identity) => {
                    identities.insert(identity);
                }
                None => push_reason(
                    reasons,
                    EffortUnavailableCode::UntaggedLaunch,
                    format!("occupant {occupant} has no complete launch identity"),
                ),
            },
            None => push_reason(
                reasons,
                EffortUnavailableCode::SessionDisagreement,
                format!("occupant {occupant} has no exact session declaration"),
            ),
        }
        if let Some(next_occupant) = next_occupant {
            occupant = next_occupant;
        }
    }
    match identities.len() {
        1 => identities.into_iter().next(),
        0 => {
            push_reason(
                reasons,
                EffortUnavailableCode::UntaggedLaunch,
                "phase has no complete vendor/model/effort declaration",
            );
            None
        }
        _ => {
            push_reason(
                reasons,
                EffortUnavailableCode::MixedLaunch,
                "phase crosses more than one vendor/model/effort identity",
            );
            None
        }
    }
}

fn sum_segments(
    segments: &[EffortSegmentAnswer],
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<EffortUsage> {
    let mut total = zero_usage();
    for segment in segments {
        let Some(next) = add_usage(&total, &segment.usage) else {
            push_reason(
                reasons,
                EffortUnavailableCode::ArithmeticOverflow,
                "usage counters overflowed u64",
            );
            return None;
        };
        total = next;
    }
    Some(total)
}

fn wall_millis(
    opening: Instant,
    closing: Instant,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<u64> {
    let Some(delta) = closing
        .get()
        .as_millisecond()
        .checked_sub(opening.get().as_millisecond())
    else {
        push_reason(
            reasons,
            EffortUnavailableCode::ArithmeticOverflow,
            "wall-time subtraction overflowed",
        );
        return None;
    };
    match u64::try_from(delta) {
        Ok(value) => Some(value),
        Err(_) => {
            push_reason(
                reasons,
                EffortUnavailableCode::MalformedUsage,
                "closing instant precedes opening instant",
            );
            None
        }
    }
}

// Keep evidence traversal and usage arithmetic below the reducer orchestration seam.
#[path = "task_effort/accounting.rs"]
mod accounting;
use accounting::*;

#[cfg(test)]
#[path = "task_effort_tests.rs"]
mod tests;
