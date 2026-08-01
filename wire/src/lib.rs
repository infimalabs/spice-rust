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
//! # The vocabularies are closed, so they stay enumerable
//!
//! [`CommandKind`], [`FactKind`], [`Witness`], and [`witness::WitnessKind`]
//! each name a closed set, and none of them is `#[non_exhaustive]`. That
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
pub mod packed;
pub mod reply;
pub mod witness;

pub use command::{Command, CommandKind, CommandKindError, Law, LawError};
pub use envelope::{Actor, ActorError, Envelope, Origin, OriginError, Seq, WireVersion};
pub use fact::{Body, Fact, FactKind, Producer};
pub use head::WitnessClass;
pub use head::{
    ClaimDisposition, ClaimHead, CursorHead, FindingHead, FindingState, LaneResidency, LeaseHead,
    ResidencyHead, ResidentState, TaskHead, TrustHead, TrustState, WitnessHead, WorkflowHead,
};
pub use hello::{Hello, Rejected};
pub use ident::{CanonicalUuid, Digest, IdentError, Instant};
pub use packed::{Derived, FoldName, FoldNameError, Overflow, Packed};
pub use reply::{
    Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition, Response, SeqRange, SeqRangeError,
};
// `WitnessKind` is deliberately absent here and reached as
// `wire::witness::WitnessKind`. The head vocabulary that once claimed the same
// name is now [`head::WitnessClass`], so the two no longer collide; what keeps
// the command discriminant off the crate root is that it and `WitnessClass`
// class the same predicates at different granularity, and a reader who reaches
// for the root spelling should have to say which one they mean.
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
