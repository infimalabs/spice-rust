//! Current authenticated canvas stream requests and resident frames.
//!
//! This is one closed DTO family: a client subscribes, continues an explicitly
//! named packed cursor, or asks for a causal thread; the resident returns
//! packed facts/trust/thread answers, later fact updates, or one typed terminal
//! boundary. Every variant carries the current wire and resident epoch. A
//! capability is deliberately absent: it crosses exactly once in [`crate::Hello`].

use serde::{Deserialize, Serialize};

use crate::{
    AnyFact, CanonicalUuid, Envelope, Fault, FaultClass, Packed, Refusal, Seq, TrustHead,
    WireVersion,
};

/// A resumable journal position bound to one resident generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamCursor {
    /// Resident generation that assigned the position.
    pub epoch: CanonicalUuid,
    /// Last position the client admitted, or the exact packed continuation.
    pub seq: Seq,
}

/// Subjects the resident can publish on the authenticated canvas socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ViewSubject {
    /// Canonical fact envelopes in durable journal order.
    Facts,
    /// Current executable-surface trust heads.
    Trust,
}

/// One strict client-to-resident stream request.
///
/// `stream` is the closed discriminant. Unknown fields and variants fail the
/// sole wire decoder rather than being ignored or upcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stream", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StreamRequest {
    /// Begin one subscription, optionally after a same-generation cursor.
    Subscribe {
        /// Current wire version.
        v: WireVersion,
        /// Correlation identity for this subscription.
        id: CanonicalUuid,
        /// Resident generation being addressed.
        epoch: CanonicalUuid,
        /// Non-empty, duplicate-free subject set.
        subjects: Vec<ViewSubject>,
        /// Last position already admitted on a reconnect.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        after: Option<StreamCursor>,
    },
    /// Continue one packed subject at its exact explicit cursor.
    Continue {
        /// Current wire version.
        v: WireVersion,
        /// Correlation identity for this request.
        id: CanonicalUuid,
        /// Resident generation being addressed.
        epoch: CanonicalUuid,
        /// Subject whose preceding packed answer overflowed.
        subject: ViewSubject,
        /// Exact continuation reported by that answer.
        cursor: StreamCursor,
    },
    /// Walk one causal thread from an explicit generation-bound position.
    Thread {
        /// Current wire version.
        v: WireVersion,
        /// Correlation identity for this request.
        id: CanonicalUuid,
        /// Resident generation being addressed.
        epoch: CanonicalUuid,
        /// First causal position for this page.
        cursor: StreamCursor,
    },
}

impl StreamRequest {
    /// Correlation identity carried by every request variant.
    #[must_use]
    pub const fn id(&self) -> CanonicalUuid {
        match self {
            Self::Subscribe { id, .. } | Self::Continue { id, .. } | Self::Thread { id, .. } => *id,
        }
    }

    /// Admit the request's version, epoch, and any embedded cursor epoch.
    ///
    /// Shape failures, including zero cursors and unknown fields, have already
    /// failed decoding before this method can be called.
    pub fn admit(&self, resident_epoch: CanonicalUuid) -> Result<(), StreamAdmissionError> {
        let (wire, epoch, cursor) = match self {
            Self::Subscribe {
                v, epoch, after, ..
            } => (*v, *epoch, *after),
            Self::Continue {
                v, epoch, cursor, ..
            }
            | Self::Thread {
                v, epoch, cursor, ..
            } => (*v, *epoch, Some(*cursor)),
        };
        if !wire.is_current() {
            return Err(StreamAdmissionError::Wire(Refusal {
                reason: "stream::wire-mismatch".to_owned(),
                witness: None,
                expected: Some(WireVersion::CURRENT.to_string()),
                observed: Some(wire.to_string()),
                remedy: "reconnect with the canvas embedded by this resident".to_owned(),
            }));
        }
        if epoch != resident_epoch {
            return Err(StreamAdmissionError::Epoch(stale_epoch(
                epoch,
                resident_epoch,
            )));
        }
        if let Some(cursor) = cursor
            && cursor.epoch != resident_epoch
        {
            return Err(StreamAdmissionError::Cursor(stale_epoch(
                cursor.epoch,
                resident_epoch,
            )));
        }
        Ok(())
    }
}

fn stale_epoch(observed: CanonicalUuid, resident: CanonicalUuid) -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: "stream::epoch-mismatch".to_owned(),
        detail: format!("stream addressed epoch {observed}; resident epoch is {resident}"),
    }
}

/// A decoded stream request addressed the wrong schema or generation.
///
/// identity-law: non-crossing - local validation state converted to a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamAdmissionError {
    /// The peer did not speak the resident's one current wire.
    Wire(Refusal),
    /// The request addressed a replaced resident generation.
    Epoch(Fault),
    /// An embedded continuation belongs to another generation.
    Cursor(Fault),
}

/// One current trust head together with the fact position that established it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustView {
    /// Fact position stored beside this head.
    pub position: Seq,
    /// Current head body.
    pub head: TrustHead,
}

/// One strict resident-to-client stream frame.
///
/// Successful packed answers keep their overflow, continuation, and horizon.
/// Live facts carry their own durable sequence. Lag and shutdown frames carry
/// the last admitted cursor so reconnect has one explicit continuation point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stream", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StreamFrame {
    /// Initial or continued fact page.
    Facts {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        packed: Packed<Envelope<AnyFact>>,
    },
    /// Initial, continued, or refreshed trust-head page.
    Trust {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        packed: Packed<TrustView>,
    },
    /// One causal thread page.
    Thread {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        packed: Packed<Envelope<AnyFact>>,
    },
    /// One later durable fact for an active subscription.
    Fact {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        fact: Envelope<AnyFact>,
    },
    /// The authenticated principal cannot admit the request as written.
    Refusal {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        refusal: Refusal,
    },
    /// The request or resident stream faulted.
    Fault {
        v: WireVersion,
        epoch: CanonicalUuid,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        id: Option<CanonicalUuid>,
        fault: Fault,
    },
    /// Bounded broadcast retention was exceeded; reconnect after this cursor.
    Overflow {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        after: StreamCursor,
        dropped: u64,
    },
    /// The resident generation is shutting down at this admitted cursor.
    Closing {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        after: StreamCursor,
    },
}
