//! Room conversation authority that native steering does not already own.
//!
//! A lane destination is delivered by exactly one native steering directive
//! opened in the publication's own write: that directive's human key, inbox,
//! ACK/NAK, wake, lapse, recovery, dead-letter, and renewal inheritance are the
//! route's only receipt. Room authority adds what steering lacks: write-once
//! publication records, one correlated result per ASK, and operator items for
//! the operator destination. An ASK's public result handle is its delivery key
//! on either side: the key of the native directive that delivers it to a lane
//! respondent, or the key of the operator item that delivers it to the
//! operator.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CanonicalUuid, Instant, SequenceCode, TaskStamp};

use super::{RoomAnchor, RoomAuthorityStamp, RoomSource, TranscriptHunk};

/// Most independently qualified destinations one room publication may carry.
pub const ROOM_ROUTE_FANOUT_LIMIT: usize = 32;
/// Most source bytes one substantive room publication may account.
pub const ROOM_PUBLICATION_BYTES_LIMIT: usize = 64 * 1024;
/// Most simultaneously pending question results one admission scope may hold.
pub const ROOM_PENDING_RESULT_LIMIT: usize = 128;
/// Aggregate content and reserved control-return bytes admitted together.
pub const ROOM_ADMISSION_BYTES_LIMIT: usize = 80 * 1024;
/// Capacity reserved at content admission for one later ACK or NAK.
pub const ROOM_CONTROL_RETURN_RESERVE_BYTES: usize = 1024;

/// A frozen room route destination.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomDestination {
    /// The authenticated operator inbox.
    Operator {},
    /// One logical lane; later occupants of that lane inherit live duties.
    Lane { lane: CanonicalUuid },
    /// One task room, with no directed recipient duty.
    Task { task: TaskStamp },
}

impl RoomDestination {
    /// Whether this destination can owe a directed receipt or question result.
    #[must_use]
    pub const fn is_audience(&self) -> bool {
        matches!(self, Self::Operator {} | Self::Lane { .. })
    }
}

/// One selector after admission resolved its audience and its own optional context.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRoute {
    /// Frozen operator, lane, or task destination.
    pub destination: RoomDestination,
    /// This selector's task context, never borrowed from another route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RoomAnchor>,
}

/// Public handle of one substantive ASK result.
///
/// It is also the ASK's delivery key: the native steering key of the
/// directive that delivers the ASK to a lane respondent, or the key of the
/// operator item that delivers it to the operator.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionResultRef(RoomAuthorityStamp);

impl QuestionResultRef {
    /// Use one native room-authority identity as the public result handle.
    #[must_use]
    pub const fn new(value: RoomAuthorityStamp) -> Self {
        Self(value)
    }

    /// The native identity carried by this typed handle.
    #[must_use]
    pub const fn authority(&self) -> &RoomAuthorityStamp {
        &self.0
    }

    /// The handle as written, which is also its delivery key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for QuestionResultRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Key of one operator item, which the operator's ACK or NAK names.
///
/// A MSG's operator item is keyed by its publication, an ASK's by its result
/// handle, and an answer returned to the operator by the answer publication.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoomReceiptRef(RoomAuthorityStamp);

impl RoomReceiptRef {
    /// Use one native room-authority identity as an operator item key.
    #[must_use]
    pub const fn new(value: RoomAuthorityStamp) -> Self {
        Self(value)
    }

    /// The native identity carried by this key.
    #[must_use]
    pub const fn authority(&self) -> &RoomAuthorityStamp {
        &self.0
    }
}

impl fmt::Display for RoomReceiptRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// How one directed delivery is carried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomDelivery {
    /// A lane destination: one native steering directive opened in the same
    /// write is the delivery's only receipt.
    Directive { directive: CanonicalUuid },
    /// The operator destination: one room operator item.
    Item { item: RoomReceiptRef },
}

/// Semantic publication verb. Operator item controls use [`RoomReceiptControl`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "lowercase", deny_unknown_fields)]
pub enum RoomPublicationKind {
    /// Ordinary MSG publication.
    Msg {},
    /// ASK publication with a distinct public result and immutable deadline.
    Ask {
        result: QuestionResultRef,
        deadline: Instant,
    },
    /// ANS publication. Its route is derived from the stored question.
    Ans {
        result: QuestionResultRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<RoomAnchor>,
    },
}

/// One source-backed substantive publication shared by operator and lane admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RoomPublicationWire", into = "RoomPublicationWire")]
pub struct RoomPublication {
    publication: RoomAuthorityStamp,
    author: RoomDestination,
    source: RoomSource,
    content_bytes: u32,
    kind: RoomPublicationKind,
    routes: Vec<RoomRoute>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomPublicationWire {
    publication: RoomAuthorityStamp,
    author: RoomDestination,
    source: RoomSource,
    content_bytes: u32,
    kind: RoomPublicationKind,
    routes: Vec<RoomRoute>,
}

impl TryFrom<RoomPublicationWire> for RoomPublication {
    type Error = &'static str;

    fn try_from(value: RoomPublicationWire) -> Result<Self, Self::Error> {
        if !value.author.is_audience() {
            return Err("room publication author must be an operator or lane");
        }
        if value.content_bytes == 0 || value.content_bytes as usize > ROOM_PUBLICATION_BYTES_LIMIT {
            return Err("room publication content exceeds its positive byte bound");
        }
        if value.routes.len() > ROOM_ROUTE_FANOUT_LIMIT || !all_unique(&value.routes) {
            return Err("room publication routes must be unique and within the fanout bound");
        }
        for route in &value.routes {
            if matches!(route.destination, RoomDestination::Task { .. }) && route.context.is_some()
            {
                return Err("task destinations already name their context");
            }
        }
        if value
            .routes
            .iter()
            .filter(|route| route.destination == RoomDestination::Operator {})
            .count()
            > 1
        {
            return Err("room publication may route to the operator once");
        }
        match &value.kind {
            RoomPublicationKind::Msg {} if value.routes.is_empty() => {
                return Err("MSG must name at least one independently qualified route");
            }
            RoomPublicationKind::Ask { .. }
                if value.routes.len() != 1 || !value.routes[0].destination.is_audience() =>
            {
                return Err("ASK must name exactly one operator or lane audience");
            }
            RoomPublicationKind::Ask { result, .. } if result.authority() == &value.publication => {
                return Err("ASK result handle must differ from its publication");
            }
            RoomPublicationKind::Ans { .. } if !value.routes.is_empty() => {
                return Err("ANS inherits its stored question route and cannot retarget");
            }
            _ => {}
        }
        Ok(Self {
            publication: value.publication,
            author: value.author,
            source: value.source,
            content_bytes: value.content_bytes,
            kind: value.kind,
            routes: value.routes,
        })
    }
}

impl From<RoomPublication> for RoomPublicationWire {
    fn from(value: RoomPublication) -> Self {
        Self {
            publication: value.publication,
            author: value.author,
            source: value.source,
            content_bytes: value.content_bytes,
            kind: value.kind,
            routes: value.routes,
        }
    }
}

impl RoomPublication {
    /// Admit one validated source-backed publication.
    pub fn try_new(
        publication: RoomAuthorityStamp,
        author: RoomDestination,
        source: RoomSource,
        content_bytes: u32,
        kind: RoomPublicationKind,
        routes: Vec<RoomRoute>,
    ) -> Result<Self, &'static str> {
        RoomPublicationWire {
            publication,
            author,
            source,
            content_bytes,
            kind,
            routes,
        }
        .try_into()
    }

    /// Stable identity of this admitted publication.
    #[must_use]
    pub const fn publication(&self) -> &RoomAuthorityStamp {
        &self.publication
    }

    /// Authenticated author frozen at admission.
    #[must_use]
    pub const fn author(&self) -> &RoomDestination {
        &self.author
    }

    /// Owning source link; content bytes remain with that authority.
    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
    }

    /// Accounted source-byte extent.
    #[must_use]
    pub const fn content_bytes(&self) -> u32 {
        self.content_bytes
    }

    /// Semantic publication operation.
    #[must_use]
    pub const fn kind(&self) -> &RoomPublicationKind {
        &self.kind
    }

    /// Independently qualified routes frozen at admission.
    #[must_use]
    pub fn routes(&self) -> &[RoomRoute] {
        &self.routes
    }

    /// Lane routes in route order, each delivered by one native directive.
    pub fn lane_routes(&self) -> impl Iterator<Item = (CanonicalUuid, Option<&RoomAnchor>)> {
        self.routes
            .iter()
            .filter_map(|route| match route.destination {
                RoomDestination::Lane { lane } => Some((lane, route.context.as_ref())),
                _ => None,
            })
    }

    /// The operator item this publication opens, with that route's context.
    ///
    /// MSG keys it by its publication and ASK by its result handle; ANS opens
    /// its return from the stored question instead.
    #[must_use]
    pub fn operator_item(&self) -> Option<(RoomReceiptRef, Option<&RoomAnchor>)> {
        let route = self
            .routes
            .iter()
            .find(|route| route.destination == RoomDestination::Operator {})?;
        let item = match &self.kind {
            RoomPublicationKind::Msg {} => self.publication.clone(),
            RoomPublicationKind::Ask { result, .. } => result.authority().clone(),
            RoomPublicationKind::Ans { .. } => return None,
        };
        Some((RoomReceiptRef::new(item), route.context.as_ref()))
    }

    /// Capacity atomically charged with this content, including later controls.
    pub fn admission_charge(
        &self,
        active_results: usize,
    ) -> Result<RoomAdmissionCharge, RoomAdmissionError> {
        let opens_result = usize::from(matches!(self.kind, RoomPublicationKind::Ask { .. }));
        if active_results
            .checked_add(opens_result)
            .is_none_or(|pending| pending > ROOM_PENDING_RESULT_LIMIT)
        {
            return Err(RoomAdmissionError::PendingResults);
        }
        let control_returns = match self.kind {
            RoomPublicationKind::Msg {} => self
                .routes
                .iter()
                .filter(|route| route.destination.is_audience())
                .count(),
            RoomPublicationKind::Ask { .. } => 2,
            RoomPublicationKind::Ans { .. } => 1,
        };
        let reserved_control_bytes = control_returns
            .checked_mul(ROOM_CONTROL_RETURN_RESERVE_BYTES)
            .ok_or(RoomAdmissionError::AggregateBytes)?;
        let aggregate_bytes = (self.content_bytes as usize)
            .checked_add(reserved_control_bytes)
            .ok_or(RoomAdmissionError::AggregateBytes)?;
        if aggregate_bytes > ROOM_ADMISSION_BYTES_LIMIT {
            return Err(RoomAdmissionError::AggregateBytes);
        }
        Ok(RoomAdmissionCharge {
            content_bytes: self.content_bytes,
            reserved_control_bytes: reserved_control_bytes as u32,
            aggregate_bytes: aggregate_bytes as u32,
            route_fanout: self.routes.len() as u16,
            pending_results: opens_result as u16,
        })
    }
}

/// Atomic content and control-return capacity consumed by one publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomAdmissionCharge {
    pub content_bytes: u32,
    pub reserved_control_bytes: u32,
    pub aggregate_bytes: u32,
    pub route_fanout: u16,
    pub pending_results: u16,
}

/// Why a substantive publication cannot reserve all authority it may create.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomAdmissionError {
    #[error("room pending-result capacity is exhausted")]
    PendingResults,
    #[error("room aggregate content and control-return capacity is exhausted")]
    AggregateBytes,
}

/// Immutable identity, publication, context, and source behind one operator item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptRecord {
    receipt: RoomReceiptRef,
    publication: RoomAuthorityStamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<RoomAnchor>,
    source: RoomSource,
}

impl RoomReceiptRecord {
    /// Create the immutable authority behind one operator item.
    #[must_use]
    pub const fn new(
        receipt: RoomReceiptRef,
        publication: RoomAuthorityStamp,
        context: Option<RoomAnchor>,
        source: RoomSource,
    ) -> Self {
        Self {
            receipt,
            publication,
            context,
            source,
        }
    }

    #[must_use]
    pub const fn receipt(&self) -> &RoomReceiptRef {
        &self.receipt
    }

    /// Publication whose operator route or answer return opened this item.
    #[must_use]
    pub const fn publication(&self) -> &RoomAuthorityStamp {
        &self.publication
    }

    #[must_use]
    pub const fn context(&self) -> Option<&RoomAnchor> {
        self.context.as_ref()
    }

    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
    }
}

/// Current operator disposition of one operator item.
///
/// Expiry is a lapse, never a decision: recovery returns the same item to
/// pending with a fresh bounded expiry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomReceiptState {
    Pending { expires: Instant },
    Acked { source: RoomSource },
    Declined { source: RoomSource },
    Expired { at: Instant },
    Cancelled { source: RoomSource },
}

impl RoomReceiptState {
    /// Whether a semantic decision retired this item for good.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Acked { .. } | Self::Declined { .. } | Self::Cancelled { .. }
        )
    }

    /// Instant of a recoverable lapse, if this item lapsed.
    #[must_use]
    pub const fn lapsed_at(&self) -> Option<Instant> {
        match self {
            Self::Expired { at } => Some(*at),
            _ => None,
        }
    }

    /// The state's wire name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Pending { .. } => "pending",
            Self::Acked { .. } => "acked",
            Self::Declined { .. } => "declined",
            Self::Expired { .. } => "expired",
            Self::Cancelled { .. } => "cancelled",
        }
    }
}

/// Why an operator item head refused a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomReceiptTransitionError {
    #[error("only a pending operator item can be settled or lapse")]
    NotPending,
    #[error("only a lapsed operator item can recover")]
    NotLapsed,
    #[error("a recovered operator item must expire after its lapse")]
    Expiry,
}

/// Guarded head of one operator item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptHead {
    receipt: RoomReceiptRef,
    state: RoomReceiptState,
}

impl RoomReceiptHead {
    /// Open one item pending until its bounded expiry.
    #[must_use]
    pub const fn pending(receipt: RoomReceiptRef, expires: Instant) -> Self {
        Self {
            receipt,
            state: RoomReceiptState::Pending { expires },
        }
    }

    #[must_use]
    pub const fn receipt(&self) -> &RoomReceiptRef {
        &self.receipt
    }

    #[must_use]
    pub const fn state(&self) -> &RoomReceiptState {
        &self.state
    }

    /// Move a pending item to a decision or a lapse, or replay that exact move.
    pub fn settle(&mut self, next: RoomReceiptState) -> Result<(), RoomReceiptTransitionError> {
        if self.state == next {
            return Ok(());
        }
        if !matches!(self.state, RoomReceiptState::Pending { .. })
            || matches!(next, RoomReceiptState::Pending { .. })
        {
            return Err(RoomReceiptTransitionError::NotPending);
        }
        self.state = next;
        Ok(())
    }

    /// Return a lapsed item to pending under the same key with a fresh expiry.
    pub fn recover(&mut self, expires: Instant) -> Result<(), RoomReceiptTransitionError> {
        let lapsed_at = self
            .state
            .lapsed_at()
            .ok_or(RoomReceiptTransitionError::NotLapsed)?;
        if expires <= lapsed_at {
            return Err(RoomReceiptTransitionError::Expiry);
        }
        self.state = RoomReceiptState::Pending { expires };
        Ok(())
    }
}

/// The operator's ACK or NAK over exact operator item keys; it has no
/// retargeting fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RoomReceiptControlWire", into = "RoomReceiptControlWire")]
pub struct RoomReceiptControl {
    disposition: RoomReceiptDisposition,
    keys: Vec<RoomReceiptRef>,
    source: RoomSource,
}

impl RoomReceiptControl {
    /// Admit one exact bounded ACK or NAK control.
    pub fn try_new(
        disposition: RoomReceiptDisposition,
        keys: Vec<RoomReceiptRef>,
        source: RoomSource,
    ) -> Result<Self, &'static str> {
        RoomReceiptControlWire {
            disposition,
            keys,
            source,
        }
        .try_into()
    }

    #[must_use]
    pub const fn disposition(&self) -> RoomReceiptDisposition {
        self.disposition
    }

    #[must_use]
    pub fn keys(&self) -> &[RoomReceiptRef] {
        &self.keys
    }

    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
    }

    /// Decision this control settles on each named operator item.
    #[must_use]
    pub fn receipt_state(&self) -> RoomReceiptState {
        let source = self.source.clone();
        match self.disposition {
            RoomReceiptDisposition::Ack => RoomReceiptState::Acked { source },
            RoomReceiptDisposition::Nak => RoomReceiptState::Declined { source },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomReceiptDisposition {
    Ack,
    Nak,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomReceiptControlWire {
    disposition: RoomReceiptDisposition,
    keys: Vec<RoomReceiptRef>,
    source: RoomSource,
}

impl TryFrom<RoomReceiptControlWire> for RoomReceiptControl {
    type Error = &'static str;

    fn try_from(value: RoomReceiptControlWire) -> Result<Self, Self::Error> {
        if value.keys.is_empty()
            || value.keys.len() > ROOM_ROUTE_FANOUT_LIMIT
            || !all_unique(&value.keys)
        {
            return Err("ACK/NAK operator item keys must be nonempty, unique, and bounded");
        }
        Ok(Self {
            disposition: value.disposition,
            keys: value.keys,
            source: value.source,
        })
    }
}

impl From<RoomReceiptControl> for RoomReceiptControlWire {
    fn from(value: RoomReceiptControl) -> Self {
        Self {
            disposition: value.disposition,
            keys: value.keys,
            source: value.source,
        }
    }
}

/// Frozen requester, respondent, and optional context of one ASK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionParties {
    pub requester: RoomDestination,
    pub respondent: RoomDestination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RoomAnchor>,
}

/// Immutable ASK authority. Delivery lapse and recovery change neither this
/// record nor its deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "QuestionResultRecordWire",
    into = "QuestionResultRecordWire"
)]
pub struct QuestionResultRecord {
    result: QuestionResultRef,
    question: RoomAuthorityStamp,
    requester: RoomDestination,
    respondent: RoomDestination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<RoomAnchor>,
    source: RoomSource,
    deadline: Instant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionResultRecordWire {
    result: QuestionResultRef,
    question: RoomAuthorityStamp,
    requester: RoomDestination,
    respondent: RoomDestination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<RoomAnchor>,
    source: RoomSource,
    deadline: Instant,
}

impl TryFrom<QuestionResultRecordWire> for QuestionResultRecord {
    type Error = &'static str;

    fn try_from(value: QuestionResultRecordWire) -> Result<Self, Self::Error> {
        if !value.requester.is_audience() || !value.respondent.is_audience() {
            return Err("question parties must be an operator or lane");
        }
        if value.result.authority() == &value.question {
            return Err("ASK publication and result identities must be distinct");
        }
        Ok(Self {
            result: value.result,
            question: value.question,
            requester: value.requester,
            respondent: value.respondent,
            context: value.context,
            source: value.source,
            deadline: value.deadline,
        })
    }
}

impl From<QuestionResultRecord> for QuestionResultRecordWire {
    fn from(value: QuestionResultRecord) -> Self {
        Self {
            result: value.result,
            question: value.question,
            requester: value.requester,
            respondent: value.respondent,
            context: value.context,
            source: value.source,
            deadline: value.deadline,
        }
    }
}

/// Nonrecursive delivery of one admitted answer back to its stored requester.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerReturnDelivery {
    destination: RoomDestination,
    delivery: RoomDelivery,
}

impl AnswerReturnDelivery {
    #[must_use]
    pub const fn destination(&self) -> &RoomDestination {
        &self.destination
    }

    #[must_use]
    pub const fn delivery(&self) -> &RoomDelivery {
        &self.delivery
    }
}

/// Substantive result state, separate from the ASK's delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionResultState {
    Pending {},
    Answered {
        answer: RoomAuthorityStamp,
        return_delivery: AnswerReturnDelivery,
    },
    Declined {
        source: RoomSource,
    },
    Cancelled {
        source: RoomSource,
    },
    TimedOut {},
}

impl QuestionResultState {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending {})
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Self::Pending {} => "pending",
            Self::Answered { .. } => "answered",
            Self::Declined { .. } => "declined",
            Self::Cancelled { .. } => "cancelled",
            Self::TimedOut {} => "timed_out",
        }
    }
}

/// Guarded result head; the immutable [`QuestionResultRecord`] holds its deadline and context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionResultHead {
    result: QuestionResultRef,
    state: QuestionResultState,
}

impl QuestionResultHead {
    /// Open one result at its only mutable semantic head.
    #[must_use]
    pub const fn pending(result: QuestionResultRef) -> Self {
        Self {
            result,
            state: QuestionResultState::Pending {},
        }
    }

    #[must_use]
    pub const fn result(&self) -> &QuestionResultRef {
        &self.result
    }

    #[must_use]
    pub const fn state(&self) -> &QuestionResultState {
        &self.state
    }

    /// Admit one terminal winner or replay that exact winner.
    pub fn settle(&mut self, next: QuestionResultState) -> Result<(), TerminalConflict> {
        if self.state == next {
            return Ok(());
        }
        if self.state.is_terminal() || !next.is_terminal() {
            return Err(TerminalConflict);
        }
        self.state = next;
        Ok(())
    }
}

impl QuestionResultRecord {
    /// Admit immutable authority for one ASK result.
    pub fn try_new(
        result: QuestionResultRef,
        question: RoomAuthorityStamp,
        parties: QuestionParties,
        source: RoomSource,
        deadline: Instant,
    ) -> Result<Self, &'static str> {
        QuestionResultRecordWire {
            result,
            question,
            requester: parties.requester,
            respondent: parties.respondent,
            context: parties.context,
            source,
            deadline,
        }
        .try_into()
    }

    #[must_use]
    pub const fn result(&self) -> &QuestionResultRef {
        &self.result
    }

    #[must_use]
    pub const fn question(&self) -> &RoomAuthorityStamp {
        &self.question
    }

    #[must_use]
    pub const fn requester(&self) -> &RoomDestination {
        &self.requester
    }

    #[must_use]
    pub const fn respondent(&self) -> &RoomDestination {
        &self.respondent
    }

    #[must_use]
    pub const fn context(&self) -> Option<&RoomAnchor> {
        self.context.as_ref()
    }

    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
    }

    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The operator item delivering this ASK, keyed by its result handle, when
    /// the operator is the respondent; a lane respondent's delivery is the
    /// native directive keyed by that same handle.
    #[must_use]
    pub fn item(&self) -> Option<RoomReceiptRef> {
        (self.respondent == RoomDestination::Operator {})
            .then(|| RoomReceiptRef::new(self.result.authority().clone()))
    }

    /// Result consequence of an operator control: NAK of this ASK's item
    /// declines the pending result, while ACK settles only the item.
    #[must_use]
    pub fn control_result(&self, control: &RoomReceiptControl) -> Option<QuestionResultState> {
        let item = self.item()?;
        (control.disposition == RoomReceiptDisposition::Nak && control.keys.contains(&item)).then(
            || QuestionResultState::Declined {
                source: control.source.clone(),
            },
        )
    }

    /// Validate ANS against the frozen question and bind its one return
    /// delivery: the single native directive `directed` names when the
    /// requester is a lane, or the operator item keyed by the answer.
    pub fn answer_state(
        &self,
        answer: &RoomPublication,
        admitted_at: Instant,
        directed: &[CanonicalUuid],
    ) -> Result<QuestionResultState, QuestionAnswerError> {
        let RoomPublicationKind::Ans { result, context } = &answer.kind else {
            return Err(QuestionAnswerError::NotAnswer);
        };
        if result != &self.result {
            return Err(QuestionAnswerError::Result);
        }
        if context
            .as_ref()
            .is_some_and(|context| Some(context) != self.context.as_ref())
        {
            return Err(QuestionAnswerError::Context);
        }
        if answer.author != self.respondent {
            return Err(QuestionAnswerError::Respondent);
        }
        if admitted_at >= self.deadline {
            return Err(QuestionAnswerError::Deadline);
        }
        let delivery = match (&self.requester, directed) {
            (RoomDestination::Lane { .. }, [directive]) => RoomDelivery::Directive {
                directive: *directive,
            },
            (RoomDestination::Operator {}, []) => RoomDelivery::Item {
                item: RoomReceiptRef::new(answer.publication.clone()),
            },
            _ => return Err(QuestionAnswerError::ReturnDelivery),
        };
        Ok(QuestionResultState::Answered {
            answer: answer.publication.clone(),
            return_delivery: AnswerReturnDelivery {
                destination: self.requester.clone(),
                delivery,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuestionAnswerError {
    #[error("publication is not ANS")]
    NotAnswer,
    #[error("ANS names another result")]
    Result,
    #[error("ANS context does not equal the stored question context")]
    Context,
    #[error("ANS author is not the stored respondent")]
    Respondent,
    #[error("ANS arrived at or after the immutable result deadline")]
    Deadline,
    #[error(
        "answer return is not one directive to a lane requester or one operator item to the operator"
    )]
    ReturnDelivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a terminal room authority already has another winner")]
pub struct TerminalConflict;

/// A lane's authored assistant MSG, ASK, or ANS, admitted from the exact
/// transcript hunk that owns its bytes.
///
/// The hunk's lane is the author and the hunk is the source, so the admitted
/// [`RoomPublication`] is derived from this record rather than restated. Every
/// lane delivery it makes is a peer message its own write opens from that
/// record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomConversed {
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// The publication's exact body bytes in its author's transcript.
    pub hunk: TranscriptHunk,
    /// Semantic verb; `kind` is the fact discriminant.
    pub operation: RoomPublicationKind,
    /// Independently qualified routes frozen at admission.
    pub routes: Vec<RoomRoute>,
    /// Directives the same write opened for this publication's lane
    /// deliveries: its lane routes in route order, or an answer's return to a
    /// lane requester.
    pub directed: Vec<CanonicalUuid>,
    /// Bounded expiry of the operator item this publication opens, if any.
    pub expires: Instant,
}

impl RoomConversed {
    /// The publication this lane record admits.
    pub fn publication(&self) -> Result<RoomPublication, &'static str> {
        let bytes = self
            .hunk
            .text
            .end
            .checked_sub(self.hunk.text.start)
            .and_then(|bytes| u32::try_from(bytes).ok())
            .ok_or("room publication hunk is not a byte range")?;
        RoomPublication::try_new(
            self.publication.clone(),
            RoomDestination::Lane {
                lane: self.hunk.head.lane,
            },
            RoomSource::Transcript(self.hunk.clone()),
            bytes,
            self.operation.clone(),
            self.routes.clone(),
        )
    }
}

/// An authenticated operator MSG, ASK, or ANS whose text this fact owns.
///
/// Every lane delivery it makes is an operator direction its own write opens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPosted {
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// The operator's exact composed text.
    pub text: String,
    /// Semantic verb; `kind` is the fact discriminant.
    pub operation: RoomPublicationKind,
    /// Independently qualified routes frozen at admission.
    pub routes: Vec<RoomRoute>,
    /// Directives the same write opened for this publication's lane
    /// deliveries: its lane routes in route order, or an answer's return to a
    /// lane requester.
    pub directed: Vec<CanonicalUuid>,
    /// Bounded expiry of the operator item this publication opens, if any.
    pub expires: Instant,
}

impl RoomPosted {
    /// The publication this fact admits at its own journal position `at`,
    /// which owns the text.
    pub fn publication(&self, at: SequenceCode) -> Result<RoomPublication, &'static str> {
        let bytes = u32::try_from(self.text.len())
            .map_err(|_| "room publication content exceeds its positive byte bound")?;
        RoomPublication::try_new(
            self.publication.clone(),
            RoomDestination::Operator {},
            RoomSource::Journal { fact: at },
            bytes,
            self.operation.clone(),
            self.routes.clone(),
        )
    }
}

/// The operator's ACK or NAK of exact operator items; this fact owns its
/// clause.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDisposed {
    pub disposition: RoomReceiptDisposition,
    pub keys: Vec<RoomReceiptRef>,
    pub clause: String,
}

impl RoomDisposed {
    /// The control this fact admits at its own journal position `at`.
    pub fn control(&self, at: SequenceCode) -> Result<RoomReceiptControl, &'static str> {
        RoomReceiptControl::try_new(
            self.disposition,
            self.keys.clone(),
            RoomSource::Journal { fact: at },
        )
    }
}

/// How an explicit command ends one pending question result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionClosure {
    /// The respondent, or the operator overriding it, refused to answer.
    Declined,
    /// The requester, or the operator overriding it, withdrew the question.
    Cancelled,
}

/// An explicit decline or cancellation of one pending question result; this
/// fact owns its clause.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomQuestionClosed {
    pub result: QuestionResultRef,
    pub closure: QuestionClosure,
    pub clause: String,
}

impl RoomQuestionClosed {
    /// The terminal result this fact admits at its own journal position `at`.
    #[must_use]
    pub fn state(&self, at: SequenceCode) -> QuestionResultState {
        let source = RoomSource::Journal { fact: at };
        match self.closure {
            QuestionClosure::Declined => QuestionResultState::Declined { source },
            QuestionClosure::Cancelled => QuestionResultState::Cancelled { source },
        }
    }
}

/// A pending question result reached its immutable deadline unanswered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomQuestionTimedOut {
    pub result: QuestionResultRef,
}

/// A pending operator item reached its expiry without a disposition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptLapsed {
    pub receipt: RoomReceiptRef,
}

/// Operator recovery returned a lapsed operator item to pending under the same
/// key with a fresh bounded expiry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptRecovered {
    pub receipt: RoomReceiptRef,
    pub expires: Instant,
}

fn all_unique<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[index + 1..].contains(value))
}

#[cfg(test)]
mod tests;
