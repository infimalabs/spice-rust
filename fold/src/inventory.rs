use crate::{
    Beyond, ConfigSource, FactPlane, FoldId, HeadSource, Recovery, RingId, Source, TapeSource,
};

/// One fold's static contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub fold: FoldId,
    pub sources: &'static [Source],
    pub beyond: Beyond,
    pub recovery: Recovery,
}

impl Definition {
    /// Exact command that heals this disposable fold.
    #[must_use]
    pub fn recovery_command(self) -> Option<String> {
        match self.recovery {
            Recovery::Replay => Some(format!("spice journal replay {}", self.fold)),
            Recovery::None => None,
        }
    }
}

/// One bounded ring that may be named as a non-replayable source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingDefinition {
    pub ring: RingId,
    pub recovery: Recovery,
}

/// The closed fold and ring registry.
#[derive(Clone, Copy, Debug)]
pub struct Inventory;

/// The one inventory value.
pub const INVENTORY: Inventory = Inventory;

impl Inventory {
    /// Every fold in dependency-safe rebuild order.
    #[must_use]
    pub const fn definitions(self) -> &'static [Definition] {
        DEFINITIONS
    }

    /// Every admitted bounded ring source.
    #[must_use]
    pub const fn rings(self) -> &'static [RingDefinition] {
        RINGS
    }

    /// Looks up the one contract for a fold.
    #[must_use]
    pub fn definition(self, fold: FoldId) -> &'static Definition {
        DEFINITIONS
            .iter()
            .find(|definition| definition.fold == fold)
            .expect("FoldId::ALL and the fold inventory are one closed set")
    }
}

const STEER_INBOX: &[Source] = &[
    Source::Facts(FactPlane::Steering),
    Source::Head(HeadSource::Witness),
    Source::Head(HeadSource::Residency),
];

const CONSENT_QUEUE: &[Source] = &[
    Source::Facts(FactPlane::Consent),
    Source::Head(HeadSource::Witness),
    Source::Head(HeadSource::Residency),
    Source::Config(ConfigSource::Consent),
];

const LANE_CANARY: &[Source] = &[
    Source::Facts(FactPlane::Canary),
    Source::Facts(FactPlane::Vendor),
    Source::Head(HeadSource::Residency),
    Source::Tape(TapeSource::DeclaredExtents),
];

const TASTE_ACTIVE: &[Source] = &[
    Source::Facts(FactPlane::Taste),
    Source::Config(ConfigSource::Taste),
];

const CANVAS_VERDICTS: &[Source] = &[
    Source::Facts(FactPlane::Commit),
    Source::Facts(FactPlane::Gate),
    Source::Head(HeadSource::Finding),
];

const CANVAS_MAGNITUDE: &[Source] = &[
    Source::Facts(FactPlane::Claim),
    Source::Facts(FactPlane::Consent),
    Source::Facts(FactPlane::Canary),
    Source::Facts(FactPlane::Vendor),
    Source::Fold(FoldId::LaneCanary),
    Source::Head(HeadSource::Task),
    Source::Head(HeadSource::Claim),
    Source::Head(HeadSource::Lease),
    Source::Head(HeadSource::Residency),
];

const CANVAS_FLOW: &[Source] = &[
    Source::Facts(FactPlane::Task),
    Source::Facts(FactPlane::Claim),
    Source::Facts(FactPlane::Document),
    Source::Head(HeadSource::Task),
    Source::Head(HeadSource::Workflow),
    Source::Head(HeadSource::Claim),
    Source::Head(HeadSource::Lease),
];

const CANVAS_TOPOLOGY: &[Source] = &[
    Source::Facts(FactPlane::Lane),
    Source::Fact(wire::FactKind::ClaimCarried),
    Source::Fact(wire::FactKind::ClaimRenewed),
    Source::Facts(FactPlane::Resident),
    Source::Head(HeadSource::Claim),
    Source::Head(HeadSource::Residency),
];

const CANVAS_CHRONOLOGY: &[Source] = &[
    Source::Facts(FactPlane::Lane),
    Source::Facts(FactPlane::Steering),
    Source::Facts(FactPlane::Consent),
    Source::Facts(FactPlane::Canary),
    Source::Facts(FactPlane::Document),
    Source::Facts(FactPlane::Trust),
    Source::Facts(FactPlane::Resident),
    Source::Facts(FactPlane::Journal),
    Source::Facts(FactPlane::Gate),
    Source::CommandResults,
    Source::Fold(FoldId::SteerInbox),
    Source::Fold(FoldId::ConsentQueue),
    Source::Fold(FoldId::LaneCanary),
    Source::Fold(FoldId::CanvasVerdicts),
    Source::Ring(RingId::BusRefusals),
    Source::Head(HeadSource::Witness),
    Source::Head(HeadSource::Residency),
];

const RELEASE_HISTORY: &[Source] = &[
    Source::Facts(FactPlane::Proof),
    Source::Facts(FactPlane::Release),
];

const DEFINITIONS: &[Definition] = &[
    definition(FoldId::SteerInbox, STEER_INBOX, Beyond::Wait),
    definition(FoldId::ConsentQueue, CONSENT_QUEUE, Beyond::Wait),
    definition(FoldId::LaneCanary, LANE_CANARY, Beyond::Wait),
    definition(FoldId::TasteActive, TASTE_ACTIVE, Beyond::Answer),
    definition(FoldId::CanvasVerdicts, CANVAS_VERDICTS, Beyond::Answer),
    definition(FoldId::CanvasMagnitude, CANVAS_MAGNITUDE, Beyond::Answer),
    definition(FoldId::CanvasFlow, CANVAS_FLOW, Beyond::Answer),
    definition(FoldId::CanvasTopology, CANVAS_TOPOLOGY, Beyond::Answer),
    definition(FoldId::CanvasChronology, CANVAS_CHRONOLOGY, Beyond::Answer),
    definition(FoldId::ReleaseHistory, RELEASE_HISTORY, Beyond::Answer),
];

const RINGS: &[RingDefinition] = &[RingDefinition {
    ring: RingId::BusRefusals,
    recovery: Recovery::None,
}];

const fn definition(fold: FoldId, sources: &'static [Source], beyond: Beyond) -> Definition {
    Definition {
        fold,
        sources,
        beyond,
        recovery: Recovery::Replay,
    }
}
