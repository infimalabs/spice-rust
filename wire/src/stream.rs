//! Current authenticated canvas stream requests and resident frames.
//!
//! This is one closed DTO family: a client subscribes, continues an explicitly
//! named packed cursor, or asks for a causal thread; the resident returns
//! packed facts/trust/thread answers, capture-backed activity snapshots, later
//! fact or activity updates, or one typed terminal boundary. Every variant
//! carries the current wire and resident epoch. A capability is deliberately
//! absent: it crosses exactly once in [`crate::Hello`].

use serde::{Deserialize, Serialize};

use crate::{
    Actor, AnyFact, CanonicalUuid, Envelope, FactKind, Fault, FaultClass, Instant, Name, Origin,
    Packed, Refusal, RefusalDiagnostic, Seq, TrustHead, WireVersion,
};

/// Stable source position of one decoded event in a lane transcript tape.
///
/// A tape locus identifies the durable vendor record. `event` distinguishes
/// multiple visible activities decoded from that one record without
/// pretending transcript events are journal facts or journal positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageLocus {
    /// Tape member holding the vendor record.
    pub member: u64,
    /// Byte offset of the vendor record within the member.
    pub offset: u64,
    /// Number of bytes in the vendor record.
    pub length: u64,
    /// Zero-based decoded event index within the vendor record.
    pub event: u32,
}

/// Visible kind of one agent-authored transcript activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentActivityKind {
    /// Assistant prose rendered as a conversation card.
    #[default]
    Prose,
    /// A tool call, update, or result rendered in the lane status strip.
    Tool,
    /// Context compaction boundary rendered as a full-width stream divider.
    Compaction,
}

impl AgentActivityKind {
    /// Whether this activity belongs in the prose card plane.
    #[must_use]
    pub const fn is_prose(&self) -> bool {
        matches!(self, Self::Prose)
    }
}

/// Agent-authored activity projected from one durable transcript event.
///
/// This is a Canvas view over tape evidence, not an authoritative fact. Prose
/// remains the default wire shape for compatibility; tool activity carries its
/// explicit kind and uses `text` for the concise operator-facing preview.
/// The lane says where the transcript belongs, the author is the exact
/// occupant that emitted it, and the locus lets consumers deduplicate
/// reconnects.
///
/// Prose outlives the seat that produced it, so [`Self::author`] names an
/// occupant that need not still be seated anywhere, and a consumer routing a
/// reply may not assume otherwise. When exactly one current member is occupied
/// by the author, a reply follows that member in whichever lane it now occupies.
/// With zero matches the reply belongs to [`Self::lane`], the lane this activity
/// was carried in; multiple matches are ambiguous and resolve there as well.
/// Neither case may select another member by row order, so no unrelated occupant
/// is handed direction it did not earn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessage {
    /// Lane whose selected transcript carried the activity.
    pub lane: CanonicalUuid,
    /// Occupant identity that authored the activity.
    pub author: CanonicalUuid,
    /// Durable transcript event supplying this activity.
    pub locus: MessageLocus,
    /// Vendor record instant that authored this visible activity.
    pub authored: Instant,
    /// Whether `authored` was synthesized because the vendor record omitted time.
    #[serde(default, skip_serializing_if = "is_false")]
    pub authored_synthetic: bool,
    /// Durable Spice observation instant when the vendor record omitted time.
    ///
    /// This is distinct from `authored`: the synthetic authored value remains
    /// a stable ordering identity, while this instant is the wall-clock time at
    /// which the transcript batch containing the message crossed the journal.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub observed: Option<Instant>,
    /// Visible activity kind. Its absent wire spelling means prose.
    #[serde(default, skip_serializing_if = "AgentActivityKind::is_prose")]
    pub activity: AgentActivityKind,
    /// Visible assistant prose or concise tool preview.
    pub text: String,
    /// Admitted media carried beside this exact transcript event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<crate::Media>,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

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
    /// Agent-authored prose and tool moments projected from transcript capture.
    Messages,
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
    /// Ask the resident to prove it is still reading this socket.
    ///
    /// A half-open socket stays open and simply stops delivering, so silence on
    /// its own is not evidence of a fault: a quiet board is silent too. This is
    /// what separates the two. It carries no cursor and asks for no state, so
    /// the answer means one thing only -- the resident read this frame and
    /// wrote back on the same socket.
    ///
    /// The transport's own ping cannot serve here. Browsers expose no way to
    /// send a WebSocket ping frame, so the probe a browser can send has to live
    /// above the transport, which is why this is a stream request rather than a
    /// control frame.
    Ping {
        /// Current wire version.
        v: WireVersion,
        /// Correlation identity for this probe.
        id: CanonicalUuid,
        /// Resident generation being addressed.
        epoch: CanonicalUuid,
    },
}

impl StreamRequest {
    /// Correlation identity carried by every request variant.
    #[must_use]
    pub const fn id(&self) -> CanonicalUuid {
        match self {
            Self::Subscribe { id, .. }
            | Self::Continue { id, .. }
            | Self::Thread { id, .. }
            | Self::Ping { id, .. } => *id,
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
            Self::Ping { v, epoch, .. } => (*v, *epoch, None),
        };
        if !wire.is_current() {
            return Err(StreamAdmissionError::Wire(Refusal::bare(
                RefusalDiagnostic::new(
                    "stream::wire-mismatch",
                    None,
                    Some(WireVersion::CURRENT.to_string()),
                    Some(wire.to_string()),
                ),
            )));
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

/// One credential-free fact rendered for an authenticated canvas view.
///
/// Journal facts retain their complete durable identity internally. This
/// fold carries the same version, position, instant, actor, origin,
/// cause, kind, and non-credential body fields, but `lane::seated` deliberately
/// omits the capability that is valid only in [`crate::Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamFact(Envelope<StreamFactBody>);

impl StreamFact {
    /// Wire version stored with the durable fact.
    #[must_use]
    pub const fn wire(&self) -> WireVersion {
        self.0.v
    }

    /// Durable journal position, unchanged by presentation.
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.0.seq
    }

    /// Commit instant, unchanged by presentation.
    #[must_use]
    pub const fn at(&self) -> &Instant {
        &self.0.at
    }

    /// Principal that caused the durable append.
    #[must_use]
    pub const fn actor(&self) -> &Actor {
        &self.0.actor
    }

    /// Root attribution carried through the durable append.
    #[must_use]
    pub const fn origin(&self) -> &Origin {
        &self.0.origin
    }

    /// Immediately provoking fact, if this is not a causal root.
    #[must_use]
    pub const fn cause(&self) -> Option<Seq> {
        self.0.cause
    }

    /// Closed fact kind retained by the credential-free fold.
    #[must_use]
    pub const fn kind(&self) -> FactKind {
        self.0.fact.kind
    }

    /// Strict kind-specific body fields retained for presentation.
    #[must_use]
    pub const fn body(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.0.fact.body
    }
}

impl From<Envelope<AnyFact>> for StreamFact {
    fn from(envelope: Envelope<AnyFact>) -> Self {
        let Envelope {
            v,
            seq,
            at,
            actor,
            origin,
            cause,
            fact,
        } = envelope;
        let kind = fact.kind();
        let body = stream_fact_body(&fact);
        Self(Envelope {
            v,
            seq,
            at,
            actor,
            origin,
            cause,
            fact: StreamFactBody { kind, body },
        })
    }
}

fn stream_fact_body(fact: &AnyFact) -> serde_json::Map<String, serde_json::Value> {
    let Some(durable) = fact.read::<crate::body::LaneSeated>() else {
        return fact.body().clone();
    };
    let crate::body::LaneSeated {
        lane,
        lineage_id,
        capability: _,
        name,
        worktree,
        life,
        vendor,
    } = durable;
    let visible = StreamLaneSeated {
        lane,
        lineage_id,
        name,
        worktree,
        life,
        vendor,
    };
    let serde_json::Value::Object(body) =
        serde_json::to_value(visible).expect("stream lane::seated body encodes")
    else {
        unreachable!("stream lane::seated is an object DTO");
    };
    body
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct StreamFactBody {
    kind: FactKind,
    #[serde(flatten)]
    body: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct UncheckedStreamFactBody {
    kind: FactKind,
    #[serde(flatten)]
    body: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for StreamFactBody {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let unchecked = UncheckedStreamFactBody::deserialize(deserializer)?;
        if unchecked.kind == FactKind::LaneSeated {
            validate_exact_body::<StreamLaneSeated>(&unchecked.body)
                .map_err(serde::de::Error::custom)?;
        } else {
            unchecked
                .kind
                .validate_body(&unchecked.body)
                .map_err(serde::de::Error::custom)?;
        }
        Ok(Self {
            kind: unchecked.kind,
            body: unchecked.body,
        })
    }
}

/// Public fields of `lane::seated`; the socket credential is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamLaneSeated {
    lane: CanonicalUuid,
    lineage_id: CanonicalUuid,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    name: Option<String>,
    worktree: String,
    life: String,
    vendor: Name,
}

fn validate_exact_body<T>(
    body: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), serde_json::Error>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    let supplied = serde_json::Value::Object(body.clone());
    let decoded: T = serde_json::from_value(supplied.clone())?;
    let registered = serde_json::to_value(decoded)?;
    if supplied == registered {
        Ok(())
    } else {
        Err(<serde_json::Error as serde::de::Error>::custom(
            "body fields do not exactly match the stream DTO",
        ))
    }
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
        packed: Packed<StreamFact>,
    },
    /// Initial, continued, or refreshed trust-head page.
    Trust {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        packed: Packed<TrustView>,
    },
    /// Current bounded transcript-activity snapshot.
    Messages {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        /// Maximum number of newest visible cards retained for each lane by this generation.
        limit: u32,
        messages: Vec<AgentMessage>,
    },
    /// One causal thread page.
    Thread {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        packed: Packed<StreamFact>,
    },
    /// One later durable fact for an active subscription.
    Fact {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        fact: StreamFact,
    },
    /// One later agent-authored transcript activity.
    Message {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
        message: AgentMessage,
    },
    /// The resident read a [`StreamRequest::Ping`] and answered on that socket.
    ///
    /// This frame deliberately carries no state. Its whole content is that it
    /// arrived, so a client that applies it must change nothing: a probe that
    /// redrew the surface every time it proved the socket healthy would make
    /// an idle board move for no reason anyone could point at.
    Pong {
        v: WireVersion,
        id: CanonicalUuid,
        epoch: CanonicalUuid,
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
