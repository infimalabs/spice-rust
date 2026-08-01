//! The result triad: one reply names the command and carries one outcome.

use serde::{Deserialize, Serialize};

use crate::envelope::{Seq, WireVersion};
use crate::ident::CanonicalUuid;

/// Process exit status carried by each outcome.
pub mod exit {
    /// A typed result committed or a snapshot answered.
    pub const REPLY: i32 = 0;
    /// A named defect in protocol, store, or environment.
    pub const FAULT: i32 = 1;
    /// An answer about an event that did not occur.
    pub const REFUSAL: i32 = 2;
}

/// An inclusive range of committed journal positions.
///
/// Decoding refuses a reversed range: WIRE names this an inclusive range, and
/// a `last` below `first` does not describe one. In-process construction is
/// the resident's own business, so the fields stay public and
/// [`is_ordered`](Self::is_ordered) remains available to check one before it
/// is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SeqRangeShape")]
pub struct SeqRange {
    /// First position committed by the transaction.
    pub first: Seq,
    /// Last position committed by the transaction.
    pub last: Seq,
}

impl SeqRange {
    /// True when the range is ordered.
    #[must_use]
    pub const fn is_ordered(self) -> bool {
        self.first.get() <= self.last.get()
    }
}

/// The decoded shape of a [`SeqRange`], before its ordering is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeqRangeShape {
    first: Seq,
    last: Seq,
}

/// A decoded range whose `last` sits below its `first`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("committed seq range is inclusive, so last must not precede first: {first}..{last}")]
pub struct SeqRangeError {
    /// First position as decoded.
    pub first: Seq,
    /// Last position as decoded.
    pub last: Seq,
}

impl TryFrom<SeqRangeShape> for SeqRange {
    type Error = SeqRangeError;

    fn try_from(shape: SeqRangeShape) -> Result<Self, Self::Error> {
        let range = Self {
            first: shape.first,
            last: shape.last,
        };
        if range.is_ordered() {
            Ok(range)
        } else {
            Err(SeqRangeError {
                first: shape.first,
                last: shape.last,
            })
        }
    }
}

/// Where a successful result sits in the journal.
///
/// A mutating command commits a range; a read-only command answers from a
/// snapshot instead. The two are distinct keys rather than one overloaded
/// field, so a reader never guesses which it received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplyPosition {
    /// Inclusive range committed by a mutating transaction.
    #[serde(rename = "seq")]
    Committed(SeqRange),
    /// Snapshot cursor answered by a read-only command.
    #[serde(rename = "cursor")]
    Snapshot(Seq),
}

/// A typed result and its journal position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply<B> {
    /// Committed range, or the snapshot cursor for a read-only result.
    #[serde(flatten)]
    pub position: ReplyPosition,
    /// Kind-specific result.
    pub body: B,
}

/// An answer about an event that did not occur.
///
/// A refusal is never a fact. It causes no journal or head mutation; the bus
/// writes a bounded telemetry row instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    /// Stable reason, suitable for matching rather than display alone.
    pub reason: String,
    /// Failed predicate name, when a witness is what failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<String>,
    /// Value the predicate required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Value the predicate observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    /// Actionable next step for the caller.
    pub remedy: String,
}

/// The plane in which a defect was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaultClass {
    /// A wire, epoch, or identity defect.
    Protocol,
    /// A journal or folds-store defect.
    Store,
    /// A defect outside the process boundary.
    Environment,
}

/// A named protocol, store, or environment defect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    /// Plane in which the defect was detected.
    pub class: FaultClass,
    /// Stable reason, suitable for matching rather than display alone.
    pub reason: String,
    /// What was observed, in enough detail to act on.
    pub detail: String,
}

/// One tagged outcome carried by a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Outcome<B> {
    /// The command succeeded.
    Reply(Reply<B>),
    /// The command was refused; nothing was committed.
    Refusal(Refusal),
    /// A defect prevented an answer.
    Fault(Fault),
}

impl<B> Outcome<B> {
    /// Process exit status for this outcome.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Reply(_) => exit::REPLY,
            Self::Refusal(_) => exit::REFUSAL,
            Self::Fault(_) => exit::FAULT,
        }
    }
}

/// The reply frame: the command it answers, the epoch that answered, and one
/// tagged outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response<B> {
    /// Current wire version.
    pub v: WireVersion,
    /// Identity of the command being answered.
    pub id: CanonicalUuid,
    /// Resident epoch that produced the answer.
    pub epoch: CanonicalUuid,
    /// The one outcome.
    pub outcome: Outcome<B>,
}

impl<B> Response<B> {
    /// Process exit status for this reply.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        self.outcome.exit_code()
    }
}
