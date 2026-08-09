use wire::body::SourceBoundary;
use wire::intent::EffortUnavailable;
use wire::{CanonicalUuid, Instant, Phase, Seq, TaskHandle};

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
    pub(super) task: TaskHandle,
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
        task: TaskHandle,
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
