//! The crossing types.
//!
//! One current DTO schema crosses the socket, rests in the journal, and
//! compiles into the browser app. This crate owns those types and the one
//! decoder every consumer uses.
//!
//! The socket, journal, and browser do not carry three compatibility systems:
//! the socket admits the current version only, the journal runs one forward
//! migration at open, and the browser has no independent clock because the app
//! is compiled from this crate and embedded in the binary that owns the socket.
//!
//! # Absent optional fields are omitted
//!
//! Every crossing `Option<T>` has one document spelling for absence: the key
//! is omitted. A present key must carry a `T`; explicit `null` is refused so
//! two different documents cannot decode to the same DTO. Authors enforce the
//! rule with `default`, `skip_serializing_if = "Option::is_none"`, and
//! `deserialize_with = "crate::optional::present"` on every optional field.
//!
//! # The vocabularies are closed, so they stay enumerable
//!
//! [`CommandKind`], [`FactKind`], [`Witness`], [`WitnessKind`], and
//! [`WitnessClass`] each name a closed set, and none of them is
//! `#[non_exhaustive]`. `WitnessKind` classifies command predicates;
//! `WitnessClass` classifies authoritative witness heads. That
//! attribute protects consumers compiled against an older release of a
//! published crate, and this crate has no such consumer: the socket admits the
//! current version only, the journal runs one forward migration at open, and
//! the browser app is compiled from this crate into the binary that owns the
//! socket. Every consumer recompiles from this source.
//!
//! What the attribute costs is what decides it. Marking these enums
//! non-exhaustive obliges every fold, producer, and view outside this crate to
//! carry a wildcard arm, which turns "a kind was added" from a build failure
//! naming the unhandled variant into a value that quietly takes the default
//! branch. Exhaustive matching is the strongest enforcement a closed vocabulary
//! can have, and these sets are declared closed precisely so they can have it.

pub mod body;
pub mod command;
pub mod envelope;
pub mod fact;
pub mod head;
pub mod hello;
pub mod ident;
/// Command bodies and answers, reached as `wire::intent::TaskDone` rather than
/// re-exported flat. There is a one-per-command body set here and a
/// one-per-fact set beside it, and short nouns like `TaskAdd` name different
/// things on the two sides. The module qualifier is what keeps the two
/// vocabularies apart, so flattening it away would forfeit that.
pub mod intent;
pub mod manifest;
mod optional;
pub mod packed;
pub mod reply;
pub mod witness;

pub use command::{Authority, Command, CommandKind, CommandKindError, Law, LawError};
pub use envelope::{Actor, ActorError, Envelope, Origin, OriginError, Seq, WireVersion};
pub use fact::{AnyFact, Body, Fact, FactKind, Producer};
pub use head::{
    ClaimDisposition, ClaimHead, CursorHead, FindingHead, FindingState, LaneResidency, LeaseHead,
    ResidencyHead, ResidentState, TaskHead, TrustHead, TrustState, WitnessClass, WitnessHead,
    WorkflowHead,
};
pub use hello::{Accepted, Hello, Rejected};
pub use ident::{CanonicalUuid, Digest, Domain, IdentError, Instant};
pub use packed::{Derived, FoldName, FoldNameError, Overflow, Packed};
pub use reply::{
    Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition, Response, SeqRange, SeqRangeError,
    Version,
};
pub use witness::{
    Answer, Disposition, Name, NameError, Phase, PhaseError, TaskHandle, TaskHandleError, Witness,
    WitnessKind,
};

/// Decode wire bytes into a DTO.
///
/// This is the sole bytes-to-DTO path for socket frames, journal rows, and
/// browser frames. Tape stays outside it: the listening dialect converts
/// preserved vendor bytes into its own typed events before resident
/// recognition decides whether a fact crossed.
///
/// # Errors
///
/// Returns the decoder error when the bytes are not a well-formed frame of the
/// requested type. A known kind with a malformed body names the field it
/// failed on.
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(bytes)
}

/// Encode a DTO into wire bytes.
///
/// # Errors
///
/// Returns the encoder error when the value cannot be represented.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value)
}
