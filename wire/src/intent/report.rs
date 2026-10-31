//! Development-scoped historical evidence reads; never a publication payload.

use serde::{Deserialize, Serialize};

use crate::{CanonicalUuid, Digest, FactKind, Instant, Seq, StreamFact};

/// Version of the bounded report read and answer contract.
pub const REPORT_READ_VERSION: u32 = 2;

/// Explicit vocabulary consumed by the report reducer; ingestion is excluded.
pub const REPORT_FACT_KINDS: &[FactKind] = &[
    FactKind::TaskAdded,
    FactKind::TaskPhased,
    FactKind::TaskDone,
    FactKind::TaskReviewed,
    FactKind::TaskAssigned,
    FactKind::TaskDeferred,
    FactKind::TaskWoken,
    FactKind::TaskFiled,
    FactKind::TaskEdited,
    FactKind::TaskDeleted,
    FactKind::TaskClosed,
    FactKind::TaskConcluded,
    FactKind::ClaimWon,
    FactKind::ClaimCarried,
    FactKind::ClaimRebound,
    FactKind::ClaimFreed,
    FactKind::ClaimLapsed,
    FactKind::ClaimAcknowledged,
    FactKind::ClaimRenewed,
    FactKind::ClaimLeaseWarned,
    FactKind::LaneSeated,
    FactKind::LaneArranged,
    FactKind::LaneRetuned,
    FactKind::LaneEffortRetuned,
    FactKind::LaneFastRetuned,
    FactKind::LaneRuntimeRetuned,
    FactKind::TeamConfigured,
    FactKind::TeamArranged,
    FactKind::LaneSessionDeclared,
    FactKind::LaneStarted,
    FactKind::LaneImported,
    FactKind::LaneRemoved,
    FactKind::LaneExited,
    FactKind::LaneTimedOut,
    FactKind::LaneCancelled,
    FactKind::LaneSignalled,
    FactKind::LaneCompacted,
    FactKind::LaneRenewed,
    FactKind::LaneRenewalChanged,
    FactKind::LaneDrained,
    FactKind::LaneKilled,
    FactKind::LaneDegraded,
    FactKind::LaneHeld,
    FactKind::LaneResumed,
    FactKind::JournalMigrated,
    FactKind::JournalRestored,
    FactKind::UpSeated,
    FactKind::UpDark,
    FactKind::ProofSealed,
    FactKind::ReleasePublished,
];
/// Authority and decoder identity that every page in an acquisition retains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportPin {
    /// Finalized journal schema admitted by this resident.
    pub schema: u32,
    /// Exact resident executable decoding the retained bytes.
    pub decoder: Digest,
    /// Native residency generation; replacement refuses an old pin.
    pub epoch: CanonicalUuid,
}

/// One bounded historical page requested by the development exporter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRead {
    /// Explicit report-read contract version.
    pub version: u32,
    /// Exclusive continuation; absent begins the retained history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Seq>,
    /// Inclusive fixed study barrier, never beyond the resident cursor.
    pub upper: Seq,
    /// Explicit consumed vocabulary, restricted by resident admission.
    pub kinds: Vec<FactKind>,
    /// First answer establishes the pin; every subsequent request supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<ReportPin>,
    /// Optional case-only review command provenance at one committed position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_at: Option<Seq>,
}

/// Exact retained non-journal evidence of authored correction flow operands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportFlowRecord {
    /// Durable request identity; retained only in private evidence.
    pub command: CanonicalUuid,
    /// Digest of the exact stored request, not its projected operands.
    pub request_digest: Digest,
    /// First committed journal position covered by the request.
    pub first: Seq,
    /// Last committed journal position covered by the request.
    pub last: Seq,
    /// Review target used for the existing public-task admission.
    pub task: crate::TaskHandle,
    /// Authored `then` flows in request order; absence is not an explicit flow.
    pub requested_flows: Vec<Option<String>>,
}

/// Bounded private evidence page after typed decoding and scope admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportAnswer {
    /// Explicit report-read contract version.
    pub version: u32,
    /// Decoder and native generation pinned for this page.
    pub pin: ReportPin,
    /// Inclusive historical barrier used by the query.
    pub upper: Seq,
    /// Exact instant of the retained barrier, not the acquisition wall clock.
    pub horizon_at: Instant,
    /// Authorized credential-free envelopes, in strictly increasing position
    /// order; `lane::seated` rows omit the lane's socket capability.
    pub rows: Vec<StreamFact>,
    /// Next exclusive cursor, even if a page contains only omitted hidden rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Seq>,
    /// Case-only non-journal evidence; never inferred from a task fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_record: Option<ReportFlowRecord>,
}
