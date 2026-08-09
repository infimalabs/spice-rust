use std::collections::{BTreeMap, BTreeSet};

use wire::body::{self, SourceBoundary, TranscriptPosition};
use wire::intent::{
    EffortAggregateAnswer, EffortAvailability, EffortIdentity, EffortPhaseAnswer,
    EffortSegmentAnswer, EffortUnavailable, EffortUnavailableCode, EffortUsage,
};
use wire::{CanonicalUuid, FactKind, Instant, Name, Phase, Seq};

use super::ORIGIN;
use super::state::{CarryCut, Closing, OpenPhase};
use super::support::{
    add_usage, divide_usage, position, push_reason, read, sample_locus, usage, zero_usage,
};

pub(super) fn finish(
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

pub(super) fn aggregate(
    phases: &[EffortPhaseAnswer],
) -> Result<Vec<EffortAggregateAnswer>, String> {
    let mut groups = BTreeMap::<(EffortIdentity, Phase), Aggregate>::new();
    for phase in phases {
        let EffortAvailability::Available {
            identity,
            usage,
            wall_ms,
        } = &phase.availability
        else {
            continue;
        };
        groups
            .entry((identity.clone(), phase.phase.clone()))
            .or_insert_with(Aggregate::new)
            .add(usage, *wall_ms)?;
    }
    groups
        .into_iter()
        .map(|((identity, phase), total)| total.answer(identity, phase))
        .collect()
}

#[derive(Debug)]
struct Aggregate {
    count: u64,
    usage: EffortUsage,
    wall_ms: u64,
}

impl Aggregate {
    fn new() -> Self {
        Self {
            count: 0,
            usage: zero_usage(),
            wall_ms: 0,
        }
    }

    fn add(&mut self, usage: &EffortUsage, wall_ms: u64) -> Result<(), String> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| "task::effort aggregate count overflow".to_owned())?;
        self.usage = add_usage(&self.usage, usage)
            .ok_or_else(|| "task::effort aggregate usage overflow".to_owned())?;
        self.wall_ms = self
            .wall_ms
            .checked_add(wall_ms)
            .ok_or_else(|| "task::effort aggregate wall time overflow".to_owned())?;
        Ok(())
    }

    fn answer(
        self,
        identity: EffortIdentity,
        phase: Phase,
    ) -> Result<EffortAggregateAnswer, String> {
        if self.count == 0 {
            return Err("task::effort aggregate has zero members".to_owned());
        }
        Ok(EffortAggregateAnswer {
            identity,
            phase,
            count: self.count,
            usage_average: divide_usage(&self.usage, self.count),
            wall_ms_average: self.wall_ms / self.count,
            usage_sum: self.usage,
            wall_ms_sum: self.wall_ms,
        })
    }
}

#[derive(Debug)]
pub(super) struct Evidence {
    batches: Vec<Batch>,
    sessions: Vec<Session>,
    lineages: LineageTimeline,
}

impl Evidence {
    pub(super) fn collect(facts: &journal::FactSnapshot) -> Self {
        let mut batches = Vec::new();
        let mut sessions = Vec::new();
        for envelope in facts.facts() {
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

    pub(super) fn lineage_at(&self, lane: CanonicalUuid, seq: Seq) -> Option<CanonicalUuid> {
        self.lineages.at(lane, seq)
    }

    fn account(
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

    fn supersedes_between(
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

    fn superseded_after(&self, retired: &Batch) -> bool {
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

    fn latest_session(
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
struct Batch {
    seq: Seq,
    body: body::JournalIngested,
}

#[derive(Debug)]
struct Session {
    seq: Seq,
    body: body::LaneSessionDeclared,
}

impl Session {
    fn identity(&self) -> Option<EffortIdentity> {
        self.body.launch.as_ref().map(|launch| EffortIdentity {
            vendor: self.body.vendor.clone(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
        })
    }
}

#[derive(Debug, Default)]
struct LineageTimeline {
    points: Vec<LineagePoint>,
    moves: Vec<MembershipCut>,
}

impl LineageTimeline {
    fn collect(facts: &journal::FactSnapshot) -> Self {
        let mut current = BTreeMap::<CanonicalUuid, CanonicalUuid>::new();
        let mut points = Vec::new();
        let mut moves = Vec::new();
        for envelope in facts.facts() {
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

    fn at(&self, lane: CanonicalUuid, seq: Seq) -> Option<CanonicalUuid> {
        self.points
            .iter()
            .filter(|point| point.lane == lane && point.seq <= seq)
            .max_by_key(|point| point.seq)
            .map(|point| point.lineage)
    }

    fn between(&self, lane: CanonicalUuid, opening: Seq, closing: Seq) -> Vec<MembershipCut> {
        self.moves
            .iter()
            .filter(|member| member.lane == lane && member.seq > opening && member.seq < closing)
            .cloned()
            .collect()
    }
}

fn set_lineage(
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
fn move_lineage(
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
struct LineagePoint {
    seq: Seq,
    lane: CanonicalUuid,
    lineage: CanonicalUuid,
}

#[derive(Debug, Clone)]
struct MembershipCut {
    seq: Seq,
    lane: CanonicalUuid,
    lineage: CanonicalUuid,
    boundary: SourceBoundary,
}

#[derive(Debug)]
struct Cut {
    seq: Seq,
    boundary: SourceBoundary,
    occupant: CanonicalUuid,
    lineage: CanonicalUuid,
}

#[derive(Debug)]
enum CutEvent {
    Carry(CarryCut),
    Membership(MembershipCut),
}

impl CutEvent {
    fn seq(&self) -> Seq {
        match self {
            Self::Carry(carry) => carry.seq,
            Self::Membership(member) => member.seq,
        }
    }
}

struct AccountRequest<'a> {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    lineage: CanonicalUuid,
    vendor: &'a Name,
    stream: &'a str,
    source: CanonicalUuid,
    from: TranscriptPosition,
    to: TranscriptPosition,
    after: Seq,
    before: Seq,
}
