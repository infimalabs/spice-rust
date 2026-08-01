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

pub mod command;
pub mod envelope;
pub mod fact;
pub mod head;
pub mod hello;
pub mod ident;
pub mod packed;
pub mod reply;
pub mod witness;

pub use command::{Command, CommandKind, CommandKindError, Law, LawError};
pub use envelope::{Actor, ActorError, Envelope, Origin, OriginError, Seq, WireVersion};
pub use fact::{Fact, FactKind};
pub use head::{
    ClaimDisposition, ClaimHead, CursorHead, FindingHead, FindingState, LaneResidency, LeaseHead,
    ResidencyHead, ResidentState, TaskHead, TrustHead, TrustState, WitnessHead, WitnessKind,
    WorkflowHead,
};
pub use hello::{Hello, Rejected};
pub use ident::{CanonicalUuid, Digest, IdentError, Instant};
pub use packed::{Derived, FoldName, FoldNameError, Overflow, Packed};
pub use reply::{
    Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition, Response, SeqRange, SeqRangeError,
};
// `WitnessKind` is deliberately absent here and reached as
// `wire::witness::WitnessKind`. Two vocabularies currently claim that name:
// `head::WitnessKind` names the predicate classes a witness head carries,
// while `witness::WitnessKind` names the command registry's discriminants.
// They overlap at different granularity, and WIRE-1kJJZ3Y7 owns deciding
// whether they are one concept. This keeps the crate compiling without
// prejudging that decision by renaming or deleting either one.
pub use witness::{
    Answer, Disposition, Name, NameError, Phase, PhaseError, Standing, TaskHandle, TaskHandleError,
    Up, Witness,
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
