//! The result triad: one reply names the command and carries one outcome.

use serde::{Deserialize, Serialize};

use crate::envelope::{Seq, WireVersion};
use crate::ident::{CanonicalUuid, Digest};
use crate::manifest::ReleaseTarget;

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

/// Where a successful result sits relative to the journal.
///
/// A mutating command commits a range; a read-only command answers from a
/// snapshot instead; and a daemonless command states directly that no journal
/// position exists. The three are distinct keys rather than one overloaded
/// field, so a reader never guesses which it received and no caller needs to
/// fabricate a sequence for a journal-free answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReplyPositionShape")]
pub enum ReplyPosition {
    /// Inclusive range committed by a mutating transaction.
    #[serde(rename = "seq")]
    Committed(SeqRange),
    /// Snapshot cursor answered by a read-only command.
    #[serde(rename = "cursor")]
    Snapshot(Seq),
    /// Direct daemonless answer carrying no journal position.
    ///
    /// The struct variant deliberately encodes as `"direct": {}`. An empty
    /// object is the one explicit absence marker; `null` and objects carrying
    /// sequence-shaped or unknown fields are not alternate spellings.
    #[serde(rename = "direct")]
    Direct {},
}

/// The three optional keys are decoded before their exclusivity is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyPositionShape {
    #[serde(default, deserialize_with = "crate::optional::present")]
    seq: Option<SeqRange>,
    #[serde(default, deserialize_with = "crate::optional::present")]
    cursor: Option<Seq>,
    #[serde(default, deserialize_with = "crate::optional::present")]
    direct: Option<DirectShape>,
}

/// The only payload admitted beside the `direct` key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectShape {}

/// A reply position that names none or more than one journal relationship.
#[derive(Debug, thiserror::Error)]
#[error("reply position must carry exactly one of seq, cursor, or direct")]
struct ReplyPositionShapeError;

impl TryFrom<ReplyPositionShape> for ReplyPosition {
    type Error = ReplyPositionShapeError;

    fn try_from(shape: ReplyPositionShape) -> Result<Self, Self::Error> {
        match (shape.seq, shape.cursor, shape.direct) {
            (Some(range), None, None) => Ok(Self::Committed(range)),
            (None, Some(cursor), None) => Ok(Self::Snapshot(cursor)),
            (None, None, Some(DirectShape {})) => Ok(Self::Direct {}),
            _ => Err(ReplyPositionShapeError),
        }
    }
}

/// A typed result and its relationship to the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply<B> {
    /// Committed range, read snapshot, or explicit journal-free position.
    #[serde(flatten)]
    pub position: ReplyPosition,
    /// Kind-specific result.
    pub body: B,
}

/// Compiled identities reported by the daemonless `version` leaf.
///
/// These four values are one machine document rather than independent prose:
/// the native artifact and embedded app identify what was built, while the
/// wire and journal versions identify the only crossing and authority schemas
/// that build admits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// Digest of the native executable for its release target.
    pub binary: Digest,
    /// The one wire schema this build speaks.
    pub wire: WireVersion,
    /// Finalized journal schema this build observed without migrating it.
    pub journal: u32,
    /// Digest of the app bytes embedded in the native executable.
    pub app: Digest,
}

/// Authenticated identities and replay counts for one offline restore bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreVerification {
    /// Domain-separated identity of the canonical backup manifest.
    pub bundle: Digest,
    /// Crossing schema admitted by the canonical manifest.
    pub wire: WireVersion,
    /// Finalized schema admitted from the authority member.
    pub journal_schema: u32,
    /// Domain-separated identity of the SQLite authority bytes.
    pub authority: Digest,
    /// Number of authenticated regular-file members.
    pub members: u64,
    /// Facts whose complete replay closure verified.
    pub facts: u64,
    /// Command receipts whose request/answer bindings verified.
    pub receipts: u64,
    /// Current authority heads whose canonical bodies verified.
    pub heads: u64,
    /// Origin/cause thread-index rows whose closure verified.
    pub threads: u64,
}

/// One fully admitted target named in deterministic release notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotesTarget {
    /// Supported Rust target tuple.
    pub target: ReleaseTarget,
    /// Canonical receipt-free target-payload identity.
    pub payload: Digest,
    /// Detached proof-receipt identity that admitted the payload.
    pub proof_receipt: Digest,
    /// Complete gate-receipt identity that admitted the source for this target.
    pub gate_receipt: Digest,
}

/// Deterministic release facts derived from one canonical evidence document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotes {
    /// Sole release-note document schema emitted by this build.
    pub version: u16,
    /// Domain-separated identity of the exact canonical input document.
    pub identity: Digest,
    /// Exact source identity shared by both admitted targets.
    pub source: Digest,
    /// Both proof- and gate-admitted supported targets in declaration order.
    pub targets: Vec<ReleaseNotesTarget>,
}

/// Verified identities returned by both daemonless proof leaves.
///
/// This body has no standing or success flag: its existence is the clean
/// outcome. Proof verification admits only clean evidence before producing
/// this document, while a refused replay travels as [`Outcome::Refusal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofEvidence {
    /// Release tuple covered by the payload, replay, and receipt.
    pub target: ReleaseTarget,
    /// Canonical identity of the receipt-free target payload.
    pub payload: Digest,
    /// Canonical identity of the detached proof receipt.
    pub receipt: Digest,
    /// Canonical identity of the ordered proof battery.
    pub battery: Digest,
    /// Canonical identity of the complete replay verdict.
    pub replay: Digest,
}

/// One operator-previewed publication of a complete sealed release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlan {
    /// Digest the apply command must re-present.
    pub key: Digest,
    /// Canonical identity of the sealed two-target manifest.
    pub manifest: Digest,
    /// Source identity shared by both target payloads.
    pub source: Digest,
    /// Exact target payload and detached-receipt identities to publish.
    pub targets: Vec<crate::body::PublishedTarget>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub witness: Option<String>,
    /// Value the predicate required.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub expected: Option<String>,
    /// Value the predicate observed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
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
