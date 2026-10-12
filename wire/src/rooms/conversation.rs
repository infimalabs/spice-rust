use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CanonicalUuid, Instant, SequenceCode, TaskStamp};

use super::{RoomAnchor, RoomAuthorityStamp, RoomSource};

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
}

impl fmt::Display for QuestionResultRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Semantic publication verb. Receipt controls use [`RoomReceiptControl`].
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
        match &value.kind {
            RoomPublicationKind::Msg {} if value.routes.is_empty() => {
                return Err("MSG must name at least one independently qualified route");
            }
            RoomPublicationKind::Ask { .. }
                if value.routes.len() != 1 || !value.routes[0].destination.is_audience() =>
            {
                return Err("ASK must name exactly one operator or lane audience");
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

/// Per-route receipt identity. It is never a question-result handle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptRef {
    pub publication: RoomAuthorityStamp,
    pub route: SequenceCode,
}

/// Immutable identity, destination, and lapse horizon behind one receipt head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptRecord {
    receipt: RoomReceiptRef,
    route: RoomRoute,
    source: RoomSource,
    deadline: Instant,
}

impl RoomReceiptRecord {
    /// Create the immutable authority behind one route receipt.
    #[must_use]
    pub const fn new(
        receipt: RoomReceiptRef,
        route: RoomRoute,
        source: RoomSource,
        deadline: Instant,
    ) -> Self {
        Self {
            receipt,
            route,
            source,
            deadline,
        }
    }

    #[must_use]
    pub const fn receipt(&self) -> &RoomReceiptRef {
        &self.receipt
    }

    #[must_use]
    pub const fn route(&self) -> &RoomRoute {
        &self.route
    }

    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
    }

    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// Current semantic recipient disposition of one directed route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomReceiptState {
    Pending {},
    Acked { source: RoomSource },
    Declined { source: RoomSource },
    Expired {},
    Unavailable {},
    Cancelled { source: RoomSource },
}

impl RoomReceiptState {
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending {})
    }
}

/// Guarded head of one route receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomReceiptHead {
    receipt: RoomReceiptRef,
    state: RoomReceiptState,
}

impl RoomReceiptHead {
    /// Open one receipt at its only mutable semantic head.
    #[must_use]
    pub const fn pending(receipt: RoomReceiptRef) -> Self {
        Self {
            receipt,
            state: RoomReceiptState::Pending {},
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

    /// Admit one terminal winner or replay that exact winner.
    pub fn settle(&mut self, next: RoomReceiptState) -> Result<(), TerminalConflict> {
        settle_once(&mut self.state, next)
    }
}

/// ACK or NAK control over exact receipt keys; it has no retargeting fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RoomReceiptControlWire", into = "RoomReceiptControlWire")]
pub struct RoomReceiptControl {
    disposition: RoomReceiptDisposition,
    receipts: Vec<RoomReceiptRef>,
    source: RoomSource,
}

impl RoomReceiptControl {
    /// Admit one exact bounded ACK or NAK control.
    pub fn try_new(
        disposition: RoomReceiptDisposition,
        receipts: Vec<RoomReceiptRef>,
        source: RoomSource,
    ) -> Result<Self, &'static str> {
        RoomReceiptControlWire {
            disposition,
            receipts,
            source,
        }
        .try_into()
    }

    #[must_use]
    pub const fn disposition(&self) -> RoomReceiptDisposition {
        self.disposition
    }

    #[must_use]
    pub fn receipts(&self) -> &[RoomReceiptRef] {
        &self.receipts
    }

    #[must_use]
    pub const fn source(&self) -> &RoomSource {
        &self.source
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
    receipts: Vec<RoomReceiptRef>,
    source: RoomSource,
}

impl TryFrom<RoomReceiptControlWire> for RoomReceiptControl {
    type Error = &'static str;

    fn try_from(value: RoomReceiptControlWire) -> Result<Self, Self::Error> {
        if value.receipts.is_empty()
            || value.receipts.len() > ROOM_ROUTE_FANOUT_LIMIT
            || !all_unique(&value.receipts)
        {
            return Err("ACK/NAK receipt keys must be nonempty, unique, and bounded");
        }
        Ok(Self {
            disposition: value.disposition,
            receipts: value.receipts,
            source: value.source,
        })
    }
}

impl From<RoomReceiptControl> for RoomReceiptControlWire {
    fn from(value: RoomReceiptControl) -> Self {
        Self {
            disposition: value.disposition,
            receipts: value.receipts,
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

/// Immutable ASK authority. Lapse and recovery change neither this record nor its deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "QuestionResultRecordWire",
    into = "QuestionResultRecordWire"
)]
pub struct QuestionResultRecord {
    result: QuestionResultRef,
    question: RoomAuthorityStamp,
    receipt: RoomReceiptRef,
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
    receipt: RoomReceiptRef,
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
        if value.receipt.publication != value.question
            || value.receipt.route != SequenceCode::new(0)
        {
            return Err("ASK receipt must be route zero of the question publication");
        }
        if value.result.authority() == &value.question {
            return Err("ASK receipt and substantive result identities must be distinct");
        }
        Ok(Self {
            result: value.result,
            question: value.question,
            receipt: value.receipt,
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
            receipt: value.receipt,
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
    answer: RoomAuthorityStamp,
    destination: RoomDestination,
    receipt: RoomReceiptRef,
}

impl AnswerReturnDelivery {
    #[must_use]
    pub const fn answer(&self) -> &RoomAuthorityStamp {
        &self.answer
    }

    #[must_use]
    pub const fn destination(&self) -> &RoomDestination {
        &self.destination
    }

    #[must_use]
    pub const fn receipt(&self) -> &RoomReceiptRef {
        &self.receipt
    }
}

/// Substantive result state, separate from the ASK route receipt.
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
        settle_once(&mut self.state, next)
    }
}

impl QuestionResultRecord {
    /// Admit immutable authority for one ASK result.
    pub fn try_new(
        result: QuestionResultRef,
        question: RoomAuthorityStamp,
        receipt: RoomReceiptRef,
        parties: QuestionParties,
        source: RoomSource,
        deadline: Instant,
    ) -> Result<Self, &'static str> {
        QuestionResultRecordWire {
            result,
            question,
            receipt,
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
    pub const fn receipt(&self) -> &RoomReceiptRef {
        &self.receipt
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

    /// Validate ANS against the frozen question and derive its one return receipt.
    pub fn answer_state(
        &self,
        answer: &RoomPublication,
        admitted_at: Instant,
        return_receipt: RoomReceiptRef,
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
        if return_receipt.publication != answer.publication
            || return_receipt.route != SequenceCode::new(0)
        {
            return Err(QuestionAnswerError::ReturnReceipt);
        }
        let return_delivery = AnswerReturnDelivery {
            answer: answer.publication.clone(),
            destination: self.requester.clone(),
            receipt: return_receipt,
        };
        Ok(QuestionResultState::Answered {
            answer: answer.publication.clone(),
            return_delivery,
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
    #[error("answer return receipt does not belong to the answer publication")]
    ReturnReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("a terminal room authority already has another winner")]
pub struct TerminalConflict;

fn settle_once<T>(current: &mut T, next: T) -> Result<(), TerminalConflict>
where
    T: Clone + PartialEq + TerminalState,
{
    if current == &next {
        return Ok(());
    }
    if current.is_terminal() || !next.is_terminal() {
        return Err(TerminalConflict);
    }
    *current = next;
    Ok(())
}

trait TerminalState {
    fn is_terminal(&self) -> bool;
}

impl TerminalState for RoomReceiptState {
    fn is_terminal(&self) -> bool {
        RoomReceiptState::is_terminal(self)
    }
}

impl TerminalState for QuestionResultState {
    fn is_terminal(&self) -> bool {
        QuestionResultState::is_terminal(self)
    }
}

fn all_unique<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[index + 1..].contains(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(text: &str) -> RoomAuthorityStamp {
        text.parse().expect("authority")
    }

    fn task(text: &str) -> TaskStamp {
        text.parse().expect("task")
    }

    fn lane(text: &str) -> CanonicalUuid {
        text.parse().expect("lane")
    }

    fn instant(text: &str) -> Instant {
        text.parse().expect("instant")
    }

    fn source(sequence: u64) -> RoomSource {
        RoomSource::Journal {
            fact: SequenceCode::new(sequence),
        }
    }

    fn message(routes: Vec<RoomRoute>, bytes: u32) -> RoomPublication {
        serde_json::from_value(serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116301"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": bytes,
            "kind": {"verb": "msg"},
            "routes": routes,
        }))
        .expect("message")
    }

    #[test]
    fn selectors_keep_independent_qualifiers_and_reject_duplicate_or_unknown_routes() {
        let first = RoomRoute {
            destination: RoomDestination::Lane {
                lane: lane("018f82d4-6f59-7d06-92e3-bce913116302"),
            },
            context: Some(task("1kKRRsDR2")),
        };
        let second = RoomRoute {
            destination: RoomDestination::Operator {},
            context: None,
        };
        let publication = message(vec![first.clone(), second.clone()], 40);
        assert_eq!(publication.routes, [first.clone(), second]);
        assert_eq!(
            crate::decode::<RoomPublication>(&crate::encode(&publication).unwrap()).unwrap(),
            publication
        );

        let duplicate = serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "operator"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": 1,
            "kind": {"verb": "msg"},
            "routes": [first.clone(), first],
        });
        assert!(serde_json::from_value::<RoomPublication>(duplicate).is_err());
        let unknown = serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "operator"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": 1,
            "kind": {"verb": "msg"},
            "routes": [{"destination": {"kind": "group", "group": "1kKRRsDR2"}}],
        });
        assert!(serde_json::from_value::<RoomPublication>(unknown).is_err());
    }

    #[test]
    fn ask_receipt_result_and_answer_return_are_distinct_authorities() {
        let result = QuestionResultRef::new(authority("1kKRRsDR3"));
        let question = authority("1kKRRsDR1");
        let receipt = RoomReceiptRef {
            publication: question.clone(),
            route: SequenceCode::new(0),
        };
        let record: QuestionResultRecord = serde_json::from_value(serde_json::json!({
            "result": result,
            "question": question,
            "receipt": receipt,
            "requester": {"kind": "operator"},
            "respondent": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
            "context": "1kKRRsDR2",
            "source": {"kind": "journal", "fact": "1"},
            "deadline": "2030-01-02T00:00:00.000Z",
        }))
        .expect("question");
        let conflated: Result<QuestionResultRecord, _> =
            serde_json::from_value(serde_json::json!({
                "result": "1kKRRsDR1",
                "question": "1kKRRsDR1",
                "receipt": {"publication": "1kKRRsDR1", "route": "0"},
                "requester": {"kind": "operator"},
                "respondent": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
                "source": {"kind": "journal", "fact": "1"},
                "deadline": "2030-01-02T00:00:00.000Z",
            }));
        assert!(conflated.is_err());
        let wrong_receipt_route: Result<QuestionResultRecord, _> =
            serde_json::from_value(serde_json::json!({
                "result": "1kKRRsDR3",
                "question": "1kKRRsDR1",
                "receipt": {"publication": "1kKRRsDR1", "route": "1"},
                "requester": {"kind": "operator"},
                "respondent": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
                "source": {"kind": "journal", "fact": "1"},
                "deadline": "2030-01-02T00:00:00.000Z",
            }));
        assert!(wrong_receipt_route.is_err());
        let mut receipt_head = RoomReceiptHead::pending(record.receipt.clone());
        receipt_head
            .settle(RoomReceiptState::Acked { source: source(2) })
            .expect("ACK settles only delivery");
        let result_head = QuestionResultHead::pending(record.result.clone());
        assert_eq!(result_head.state, QuestionResultState::Pending {});

        let answer: RoomPublication = serde_json::from_value(serde_json::json!({
            "publication": "1kKRRsDR4",
            "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
            "source": {"kind": "journal", "fact": "3"},
            "content_bytes": 20,
            "kind": {"verb": "ans", "result": "1kKRRsDR3", "context": "1kKRRsDR2"},
            "routes": [],
        }))
        .expect("answer");
        let returned = record
            .answer_state(
                &answer,
                instant("2030-01-01T00:00:00.000Z"),
                RoomReceiptRef {
                    publication: authority("1kKRRsDR4"),
                    route: SequenceCode::new(0),
                },
            )
            .expect("answer derives return delivery");
        let QuestionResultState::Answered {
            return_delivery, ..
        } = returned
        else {
            panic!("answer is terminal");
        };
        assert_eq!(return_delivery.destination, RoomDestination::Operator {});
        assert_ne!(
            record.result.to_string(),
            record.receipt.publication.to_string()
        );
    }

    #[test]
    fn immutable_question_deadline_and_one_terminal_winner_survive_replay() {
        let result = QuestionResultRef::new(authority("1kKRRsDR3"));
        let winner = QuestionResultState::TimedOut {};
        let mut head = QuestionResultHead {
            result,
            state: QuestionResultState::Pending {},
        };
        head.settle(winner.clone()).expect("first terminal wins");
        head.settle(winner).expect("exact replay is idempotent");
        assert_eq!(
            head.settle(QuestionResultState::Cancelled { source: source(4) }),
            Err(TerminalConflict)
        );
    }

    #[test]
    fn content_admission_reserves_control_returns_before_saturation() {
        let routes = (0..ROOM_ROUTE_FANOUT_LIMIT)
            .map(|index| RoomRoute {
                destination: RoomDestination::Lane {
                    lane: CanonicalUuid::new(uuid::Uuid::from_u128(index as u128 + 1)),
                },
                context: None,
            })
            .collect::<Vec<_>>();
        let boundary_content = ROOM_ADMISSION_BYTES_LIMIT
            - ROOM_ROUTE_FANOUT_LIMIT * ROOM_CONTROL_RETURN_RESERVE_BYTES;
        let publication = message(routes.clone(), boundary_content as u32);
        let charge = publication.admission_charge(0).expect("boundary admits");
        assert_eq!(charge.aggregate_bytes as usize, ROOM_ADMISSION_BYTES_LIMIT);
        assert_eq!(
            message(routes, boundary_content as u32 + 1).admission_charge(0),
            Err(RoomAdmissionError::AggregateBytes)
        );

        let control: RoomReceiptControl = serde_json::from_value(serde_json::json!({
            "disposition": "ack",
            "receipts": [{"publication": "1kKRRsDR1", "route": "0"}],
            "source": {"kind": "journal", "fact": "5"},
        }))
        .expect("reserved ACK remains admissible independently of content saturation");
        assert_eq!(control.disposition, RoomReceiptDisposition::Ack);
    }

    #[test]
    fn pending_result_limit_and_answer_context_are_exact() {
        let ask: RoomPublication = serde_json::from_value(serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "operator"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": 10,
            "kind": {"verb": "ask", "result": "1kKRRsDR3", "deadline": "2030-01-02T00:00:00.000Z"},
            "routes": [{"destination": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"}, "context": "1kKRRsDR2"}],
        }))
        .expect("ask");
        assert_eq!(
            ask.admission_charge(ROOM_PENDING_RESULT_LIMIT),
            Err(RoomAdmissionError::PendingResults)
        );

        let answer = serde_json::json!({
            "publication": "1kKRRsDR4",
            "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
            "source": {"kind": "journal", "fact": "3"},
            "content_bytes": 20,
            "kind": {"verb": "ans", "result": "1kKRRsDR3", "context": "1kKRRsDR5"},
            "routes": [],
        });
        let answer: RoomPublication = serde_json::from_value(answer).expect("answer shape");
        let record: QuestionResultRecord = serde_json::from_value(serde_json::json!({
            "result": "1kKRRsDR3",
            "question": "1kKRRsDR1",
            "receipt": {"publication": "1kKRRsDR1", "route": "0"},
            "requester": {"kind": "operator"},
            "respondent": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"},
            "context": "1kKRRsDR2",
            "source": {"kind": "journal", "fact": "1"},
            "deadline": "2030-01-02T00:00:00.000Z",
        }))
        .expect("question");
        assert_eq!(
            record.answer_state(
                &answer,
                instant("2030-01-01T00:00:00.000Z"),
                RoomReceiptRef {
                    publication: authority("1kKRRsDR4"),
                    route: SequenceCode::new(0),
                },
            ),
            Err(QuestionAnswerError::Context)
        );
    }
}
