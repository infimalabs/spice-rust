//! Evidence traversal, phase state, and usage arithmetic for task-effort reduction.

use std::collections::{BTreeMap, BTreeSet};

use wire::body::{self, SourceBoundary, TranscriptPosition};
use wire::intent::{
    EffortIdentity, EffortPhaseAnswer, EffortSegmentAnswer, EffortUnavailable,
    EffortUnavailableCode, EffortUsage,
};
use wire::{CanonicalUuid, FactKind, Instant, Name, Phase, Seq, TaskStamp};

use super::ORIGIN;

#[derive(Debug)]
pub(super) struct Evidence {
    pub(super) batches: Vec<Batch>,
    pub(super) sessions: Vec<Session>,
    pub(super) lineages: LineageTimeline,
}

impl Evidence {
    pub(super) fn collect(facts: &[wire::Envelope<wire::AnyFact>]) -> Self {
        let mut batches = Vec::new();
        let mut sessions = Vec::new();
        for envelope in facts {
            match envelope.fact.kind() {
                FactKind::JournalIngested => batches.push(Batch {
                    seq: envelope.seq,
                    body: read(envelope),
                }),
                FactKind::LaneSessionDeclared => sessions.push(Session {
                    seq: envelope.seq,
                    body: read(envelope),
                }),
                _ => {}
            }
        }
        Self {
            batches,
            sessions,
            lineages: LineageTimeline::collect(facts),
        }
    }

    pub(super) fn account(
        &self,
        request: AccountRequest<'_>,
        reasons: &mut Vec<EffortUnavailable>,
    ) -> Vec<EffortSegmentAnswer> {
        if request.from == request.to {
            return Vec::new();
        }
        let mut cursor = request.from;
        let mut after = request.after;
        let mut matched = Vec::<&Batch>::new();
        while cursor != request.to {
            let candidates = self
                .batches
                .iter()
                .filter(|batch| {
                    batch.seq > after
                        && batch.seq <= request.before
                        && batch.body.lane == request.lane
                        && &batch.body.vendor == request.vendor
                        && batch.body.stream == request.stream
                        && batch.body.source == request.source
                        && batch.body.from == cursor
                        && position(batch.body.to) <= position(request.to)
                        && position(batch.body.to) > position(cursor)
                })
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                push_reason(
                    reasons,
                    EffortUnavailableCode::SourceGap,
                    format!(
                        "{} batch(es) continue {} from {}:{}",
                        candidates.len(),
                        request.stream,
                        cursor.member,
                        cursor.offset
                    ),
                );
                return Vec::new();
            }
            let batch = candidates[0];
            cursor = batch.body.to;
            after = batch.seq;
            matched.push(batch);
        }

        let mut segments = Vec::new();
        for batch in matched {
            if self.superseded_after(batch) {
                continue;
            }
            let mut batch_usage = zero_usage();
            for sample in &batch.body.usage {
                if sample_locus(sample, &batch.body, reasons).is_none() {
                    continue;
                }
                match add_usage(&batch_usage, &usage(sample)) {
                    Some(next) => batch_usage = next,
                    None => push_reason(
                        reasons,
                        EffortUnavailableCode::ArithmeticOverflow,
                        format!("usage counters overflowed in batch at {}", batch.seq),
                    ),
                }
            }
            let session = self.latest_session(
                request.lane,
                request.occupant,
                Some(request.vendor),
                batch.seq,
            );
            if session.is_none() {
                push_reason(
                    reasons,
                    EffortUnavailableCode::SessionDisagreement,
                    format!(
                        "batch at {} has no lane/occupant/vendor session declaration",
                        batch.seq
                    ),
                );
            }
            let identity = session.and_then(Session::identity);
            if identity.is_none() {
                push_reason(
                    reasons,
                    EffortUnavailableCode::UntaggedLaunch,
                    format!("batch at {} has no complete launch identity", batch.seq),
                );
            }
            segments.push(EffortSegmentAnswer {
                lane: request.lane,
                occupant: request.occupant,
                lineage: request.lineage,
                session: session.map(|session| session.body.identity.clone()),
                source: request.source,
                from: batch.body.from,
                to: batch.body.to,
                identity,
                usage: batch_usage,
            });
        }
        segments
    }

    pub(super) fn supersedes_between(
        &self,
        successor: &wire::TranscriptSourceHead,
        predecessor: &wire::TranscriptSourceHead,
        after: Seq,
        before: Seq,
    ) -> bool {
        let mut source = successor.source;
        let mut seen = BTreeSet::new();
        while source != predecessor.source {
            if !seen.insert(source) {
                return false;
            }
            let links = self
                .batches
                .iter()
                .filter(|batch| {
                    batch.seq > after
                        && batch.seq < before
                        && batch.body.lane == successor.lane
                        && batch.body.vendor == successor.vendor
                        && batch.body.stream == successor.stream
                        && batch.body.source == source
                        && batch.body.from == ORIGIN
                })
                .filter_map(|batch| batch.body.supersedes)
                .collect::<Vec<_>>();
            let [retired] = links.as_slice() else {
                return false;
            };
            source = *retired;
        }
        true
    }

    pub(super) fn superseded_after(&self, retired: &Batch) -> bool {
        self.batches.iter().any(|successor| {
            successor.seq > retired.seq
                && successor.body.lane == retired.body.lane
                && successor.body.vendor == retired.body.vendor
                && successor.body.stream == retired.body.stream
                && successor.body.source != retired.body.source
                && successor.body.from == ORIGIN
                && successor.body.supersedes == Some(retired.body.source)
        })
    }

    pub(super) fn latest_session(
        &self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        vendor: Option<&Name>,
        before: Seq,
    ) -> Option<&Session> {
        self.sessions
            .iter()
            .filter(|session| {
                session.seq <= before
                    && session.body.lane == lane
                    && session.body.occupant == occupant
                    && vendor.is_none_or(|vendor| vendor == &session.body.vendor)
            })
            .max_by_key(|session| session.seq)
    }
}

#[derive(Debug)]
pub(super) struct Batch {
    pub(super) seq: Seq,
    pub(super) body: body::JournalIngested,
}

#[derive(Debug)]
pub(super) struct Session {
    pub(super) seq: Seq,
    pub(super) body: body::LaneSessionDeclared,
}

impl Session {
    pub(super) fn identity(&self) -> Option<EffortIdentity> {
        self.body.launch.as_ref().map(|launch| EffortIdentity {
            vendor: self.body.vendor.clone(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
        })
    }
}

#[derive(Debug, Default)]
pub(super) struct LineageTimeline {
    pub(super) points: Vec<LineagePoint>,
    pub(super) moves: Vec<MembershipCut>,
}

impl LineageTimeline {
    pub(super) fn collect(facts: &[wire::Envelope<wire::AnyFact>]) -> Self {
        let mut current = BTreeMap::<CanonicalUuid, CanonicalUuid>::new();
        let mut points = Vec::new();
        let mut moves = Vec::new();
        for envelope in facts {
            match envelope.fact.kind() {
                FactKind::LaneSeated => {
                    let body: body::LaneSeated = read(envelope);
                    set_lineage(
                        &mut current,
                        &mut points,
                        envelope.seq,
                        body.lane,
                        body.lineage_id,
                    );
                }
                FactKind::LaneArranged => {
                    let body: body::LaneArranged = read(envelope);
                    let boundary = body.sources.iter().find(|source| source.lane == body.lane);
                    move_lineage(
                        &mut current,
                        &mut points,
                        &mut moves,
                        envelope.seq,
                        body.lane,
                        body.lineage_id,
                        boundary.map(|source| source.source.clone()),
                    );
                }
                FactKind::LineageConfigured => {
                    let body: body::LineageConfigured = read(envelope);
                    if let Some(lane) = body.member {
                        let boundary = body.sources.iter().find(|source| source.lane == lane);
                        move_lineage(
                            &mut current,
                            &mut points,
                            &mut moves,
                            envelope.seq,
                            lane,
                            body.lineage_id,
                            boundary.map(|source| source.source.clone()),
                        );
                    }
                }
                FactKind::LineageArranged => {
                    let body: body::LineageArranged = read(envelope);
                    for standing in body.lineages {
                        for lane in standing.lanes {
                            if current.get(&lane) != Some(&standing.lineage_id) {
                                let boundary =
                                    body.sources.iter().find(|source| source.lane == lane);
                                move_lineage(
                                    &mut current,
                                    &mut points,
                                    &mut moves,
                                    envelope.seq,
                                    lane,
                                    standing.lineage_id,
                                    boundary.map(|source| source.source.clone()),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Self { points, moves }
    }

    pub(super) fn at(&self, lane: CanonicalUuid, seq: Seq) -> Option<CanonicalUuid> {
        self.points
            .iter()
            .filter(|point| point.lane == lane && point.seq <= seq)
            .max_by_key(|point| point.seq)
            .map(|point| point.lineage)
    }

    pub(super) fn between(
        &self,
        lane: CanonicalUuid,
        opening: Seq,
        closing: Seq,
    ) -> Vec<MembershipCut> {
        self.moves
            .iter()
            .filter(|member| member.lane == lane && member.seq > opening && member.seq < closing)
            .cloned()
            .collect()
    }
}

pub(super) fn set_lineage(
    current: &mut BTreeMap<CanonicalUuid, CanonicalUuid>,
    points: &mut Vec<LineagePoint>,
    seq: Seq,
    lane: CanonicalUuid,
    lineage: CanonicalUuid,
) {
    current.insert(lane, lineage);
    points.push(LineagePoint { seq, lane, lineage });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn move_lineage(
    current: &mut BTreeMap<CanonicalUuid, CanonicalUuid>,
    points: &mut Vec<LineagePoint>,
    moves: &mut Vec<MembershipCut>,
    seq: Seq,
    lane: CanonicalUuid,
    lineage: CanonicalUuid,
    boundary: Option<SourceBoundary>,
) {
    set_lineage(current, points, seq, lane, lineage);
    moves.push(MembershipCut {
        seq,
        lane,
        lineage,
        boundary: boundary.unwrap_or_else(SourceBoundary::unrecorded),
    });
}

#[derive(Debug, Clone)]
pub(super) struct LineagePoint {
    pub(super) seq: Seq,
    pub(super) lane: CanonicalUuid,
    pub(super) lineage: CanonicalUuid,
}

#[derive(Debug, Clone)]
pub(super) struct MembershipCut {
    pub(super) seq: Seq,
    pub(super) lane: CanonicalUuid,
    pub(super) lineage: CanonicalUuid,
    pub(super) boundary: SourceBoundary,
}

#[derive(Debug)]
pub(super) struct TaskState {
    pub(super) phase: Phase,
    pub(super) phase_index: u32,
    pub(super) phase_index_overflow: bool,
    pub(super) open: Option<OpenPhase>,
}

impl TaskState {
    pub(super) fn unknown() -> Self {
        Self {
            phase: "unknown".parse().expect("unknown is one lower-case word"),
            phase_index: 0,
            phase_index_overflow: false,
            open: None,
        }
    }
}

#[derive(Debug)]
pub(super) struct OpenPhase {
    /// Durable birth identity; project edits cannot split one effort history.
    pub(super) task: TaskStamp,
    pub(super) phase: Phase,
    pub(super) phase_index: u32,
    pub(super) opening: Seq,
    pub(super) opened_at: Instant,
    pub(super) opening_boundary: SourceBoundary,
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) lineage: CanonicalUuid,
    pub(super) renewals: u32,
    pub(super) carries: Vec<CarryCut>,
    pub(super) reasons: Vec<EffortUnavailable>,
}

impl OpenPhase {
    pub(super) fn missing(
        task: TaskStamp,
        phase: Phase,
        phase_index: u32,
        seq: Seq,
        at: Instant,
    ) -> Self {
        let nil: CanonicalUuid = "00000000-0000-0000-0000-000000000000"
            .parse()
            .expect("nil UUID is canonical");
        Self {
            task,
            phase,
            phase_index,
            opening: seq,
            opened_at: at,
            opening_boundary: SourceBoundary::unrecorded(),
            lane: nil,
            occupant: nil,
            lineage: nil,
            renewals: 0,
            carries: Vec::new(),
            reasons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CarryCut {
    pub(super) seq: Seq,
    pub(super) boundary: SourceBoundary,
    pub(super) ancestor: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
}

#[derive(Debug)]
pub(super) struct Closing {
    pub(super) seq: Seq,
    pub(super) at: Instant,
    pub(super) boundary: SourceBoundary,
}

#[derive(Debug)]
pub(super) struct Cut {
    pub(super) seq: Seq,
    pub(super) boundary: SourceBoundary,
    pub(super) occupant: CanonicalUuid,
    pub(super) lineage: CanonicalUuid,
}

#[derive(Debug)]
pub(super) enum CutEvent {
    Carry(CarryCut),
    Membership(MembershipCut),
}

impl CutEvent {
    pub(super) fn seq(&self) -> Seq {
        match self {
            Self::Carry(carry) => carry.seq,
            Self::Membership(member) => member.seq,
        }
    }
}

pub(super) struct AccountRequest<'a> {
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) lineage: CanonicalUuid,
    pub(super) vendor: &'a Name,
    pub(super) stream: &'a str,
    pub(super) source: CanonicalUuid,
    pub(super) from: TranscriptPosition,
    pub(super) to: TranscriptPosition,
    pub(super) after: Seq,
    pub(super) before: Seq,
}

pub(super) fn sample_locus(
    sample: &body::TranscriptUsage,
    batch: &body::JournalIngested,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<(TranscriptPosition, TranscriptPosition)> {
    let Some(first) = sample.spans.first() else {
        push_reason(
            reasons,
            EffortUnavailableCode::MalformedUsage,
            "usage sample has no source span",
        );
        return None;
    };
    let last = sample.spans.last()?;
    let Some(end) = last.offset.checked_add(last.length) else {
        push_reason(
            reasons,
            EffortUnavailableCode::ArithmeticOverflow,
            "usage span end overflowed u64",
        );
        return None;
    };
    let from = TranscriptPosition {
        member: first.member,
        offset: first.offset,
    };
    let to = TranscriptPosition {
        member: last.member,
        offset: end,
    };
    let ordered = sample.spans.windows(2).all(|pair| {
        let left_end = pair[0].offset.checked_add(pair[0].length);
        left_end
            .is_some_and(|left_end| (pair[0].member, left_end) <= (pair[1].member, pair[1].offset))
    });
    if !ordered || position(from) < position(batch.from) || position(to) > position(batch.to) {
        push_reason(
            reasons,
            EffortUnavailableCode::MalformedUsage,
            "usage spans are unordered or cross their accounted batch",
        );
        return None;
    }
    Some((from, to))
}

pub(super) fn usage(sample: &body::TranscriptUsage) -> EffortUsage {
    EffortUsage {
        turns: sample.turns,
        input_tokens: sample.input_tokens,
        cached_input_tokens: sample.cached_input_tokens,
        cache_write_input_tokens: sample.cache_write_input_tokens,
        output_tokens: sample.output_tokens,
        reasoning_output_tokens: sample.reasoning_output_tokens,
    }
}

pub(super) fn zero_usage() -> EffortUsage {
    EffortUsage {
        reasoning_output_tokens: Some(0),
        ..EffortUsage::default()
    }
}

pub(super) fn add_usage(left: &EffortUsage, right: &EffortUsage) -> Option<EffortUsage> {
    Some(EffortUsage {
        turns: left.turns.checked_add(right.turns)?,
        input_tokens: left.input_tokens.checked_add(right.input_tokens)?,
        cached_input_tokens: left
            .cached_input_tokens
            .checked_add(right.cached_input_tokens)?,
        cache_write_input_tokens: left
            .cache_write_input_tokens
            .checked_add(right.cache_write_input_tokens)?,
        output_tokens: left.output_tokens.checked_add(right.output_tokens)?,
        reasoning_output_tokens: match (left.reasoning_output_tokens, right.reasoning_output_tokens)
        {
            (Some(left), Some(right)) => Some(left.checked_add(right)?),
            _ => None,
        },
    })
}

pub(super) fn divide_usage(total: &EffortUsage, count: u64) -> EffortUsage {
    EffortUsage {
        turns: total.turns / count,
        input_tokens: total.input_tokens / count,
        cached_input_tokens: total.cached_input_tokens / count,
        cache_write_input_tokens: total.cache_write_input_tokens / count,
        output_tokens: total.output_tokens / count,
        reasoning_output_tokens: total.reasoning_output_tokens.map(|value| value / count),
    }
}

pub(super) fn first_locus(phase: &EffortPhaseAnswer) -> (u64, u64) {
    phase
        .segments
        .first()
        .map_or((0, 0), |segment| position(segment.from))
}

pub(super) const fn position(value: TranscriptPosition) -> (u64, u64) {
    (value.member, value.offset)
}

pub(super) fn read<F: wire::Body>(envelope: &wire::Envelope<wire::AnyFact>) -> F {
    envelope
        .fact
        .read()
        .expect("fact kind selected its registered body")
}

pub(super) fn push_reason(
    reasons: &mut Vec<EffortUnavailable>,
    code: EffortUnavailableCode,
    detail: impl Into<String>,
) {
    reasons.push(reason(code, detail));
}

pub(super) fn reason(code: EffortUnavailableCode, detail: impl Into<String>) -> EffortUnavailable {
    EffortUnavailable {
        code,
        detail: detail.into(),
    }
}

pub(super) fn epoch() -> Instant {
    "1970-01-01T00:00:00.000Z"
        .parse()
        .expect("epoch spelling is canonical")
}
