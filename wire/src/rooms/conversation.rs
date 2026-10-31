//! Source-backed room messages and operator-item authority.
//!
//! A lane destination is delivered by exactly one native steering directive
//! opened in the message's own write. Room authority adds write-once message
//! records and operator items for the operator destination.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{CanonicalUuid, Instant, SequenceCode, TaskStamp};

use super::{RoomAnchor, RoomAuthorityStamp, RoomSource, TranscriptHunk};

/// Most independently qualified destinations one room publication may carry.
pub const ROOM_ROUTE_FANOUT_LIMIT: usize = 32;
/// Most source bytes one substantive room publication may account.
pub const ROOM_PUBLICATION_BYTES_LIMIT: usize = 64 * 1024;
/// Aggregate content and reserved control-return bytes admitted together.
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
    /// Whether this destination can owe a directed receipt.
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
    /// Key of the operator direction a lane's operator route concerns.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub direction: Option<String>,
}

/// Key of one operator item, which the operator's ACK or NAK names.
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

/// Semantic message verb. Operator item controls use [`RoomReceiptControl`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "lowercase", deny_unknown_fields)]
pub enum RoomPublicationKind {
    /// Ordinary MSG publication.
    Msg {},
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
            if route.direction.is_some()
                && (route.destination != RoomDestination::Operator {}
                    || !matches!(value.author, RoomDestination::Lane { .. }))
            {
                return Err("only a lane's operator route names the direction it concerns");
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
        if value.routes.is_empty() {
            return Err("MSG must name at least one independently qualified route");
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

    /// The operator item this message opens, with that route's context.
    #[must_use]
    pub fn operator_item(&self) -> Option<(RoomReceiptRef, Option<&RoomAnchor>)> {
        let route = self
            .routes
            .iter()
            .find(|route| route.destination == RoomDestination::Operator {})?;
        Some((
            RoomReceiptRef::new(self.publication.clone()),
            route.context.as_ref(),
        ))
    }

    /// Capacity atomically charged with this content, including later controls.
    pub fn admission_charge(&self) -> Result<RoomAdmissionCharge, RoomAdmissionError> {
        let control_returns = self
            .routes
            .iter()
            .filter(|route| route.destination.is_audience())
            .count();
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
}

/// Why a message cannot reserve all authority it may create.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomAdmissionError {
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

/// A lane's authored assistant MSG, admitted from the exact transcript hunk
/// that owns its bytes.
///
/// The hunk's lane is the author and the hunk is the source, so the admitted
/// [`RoomPublication`] is derived from this record rather than restated.
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
    /// Directives the same write opened for this message's lane deliveries.
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

/// An authenticated operator MSG whose text this fact owns.
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
    /// Directives the same write opened for this message's lane deliveries.
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
