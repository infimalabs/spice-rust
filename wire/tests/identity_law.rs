//! The identity law every crossing type owes the wire.
//!
//! A type breaks the law when it stores a distinction its encoding erases.
//! `Instant` did: it derived `Eq`, `Ord`, and `Hash` over a full-resolution
//! timestamp while rendering three fractional digits, so two values encoding
//! to byte-identical text compared unequal.
//!
//! The law is stated here as `decode(encode(v)) == v`, which is stronger than
//! spot-checking pairs and implies what the wire actually needs. If encoding
//! then decoding returns the same value for every constructible `v`, then any
//! `a` and `b` with `encode(a) == encode(b)` satisfy
//! `a == decode(encode(a)) == decode(encode(b)) == b`. Byte equality therefore
//! forces value equality by substitution rather than by sampling luck.
//!
//! This runs the opposite direction from `roundtrip.rs`. There the documents
//! printed in WIRE.md are decoded and re-encoded, proving the field names and
//! shapes are the specified ones. Here values are encoded and decoded, proving
//! no value carries more than its bytes. A type needs both: the first pins the
//! spelling, the second pins that the spelling is the whole value.
//!
//! Samples must include values built through every narrowing construction
//! path, because a type that narrows only at render is exactly the defect this
//! fixture exists to catch.
//!
//! # An ordered type ranks one way, and the fixture says which
//!
//! A type that carries two orders has the same defect as one that carries two
//! spellings: whichever a reader picks, some other reader picked the other and
//! built a different list. So every covered type deriving `Ord` states which
//! order it has. [`spelling_law`] is for the types that rank by the text they
//! cross as, and checks `cmp` against the encoded bytes in both directions.
//! [`ranked_law`] is for the types that rank by a declared sequence instead —
//! [`Seq`] counts and [`ResidentState`] progresses through a shutdown, and
//! neither ranks the way its text spells — and checks `cmp` against the
//! sequence written at the call site.
//!
//! Asserting the first law universally would be false rather than strict:
//! position `9` ranks before `10` and spells after it. Neither law admits an
//! `Equal`-only comparison, which is what let [`Actor`] rank `Operator` before
//! `Lane(..)` while spelling `lane:` before `operator`.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use syn::{Attribute, Fields, Item, Meta, Token, UseTree, Visibility};
use wire::manifest::ReleaseTarget;
use wire::{
    Accepted, Actor, Answer, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command,
    CommandKind, CursorHead, DemoAnswer, Derived, Digest, Disposition, DoctorObservation,
    DoctorToolBounds, DoctorToolCancellation, DoctorToolMode, DoctorToolNetwork,
    DoctorToolRequirement, DoctorToolSource, DurationWord, Envelope, Fact, FactKind, Fault,
    FaultClass, FindingHead, FindingState, FoldName, FoldState, Hello, Instant, JournalReport,
    JournalRestore, LaneResidency, LaneSession, LeaseHead, Name, Origin, Outcome, Overflow, Packed,
    Phase, Producer, ProofEvidence, Provenance, Refusal, Reply, ReplyPosition, ResidencyHead,
    ResidentState, Response, RestoreVerification, Seq, SeqRange, SpiceVariable, StreamCursor,
    StreamFact, StreamFrame, StreamRequest, TapeLocus, TaskHandle, TaskHead, TrustHead, TrustState,
    TrustView, ViewSubject, WireVersion, Witness, WitnessClass, WitnessHead, WitnessKind,
    WorkflowHead, body,
};

include!("identity_law/laws.rs");
include!("identity_law/crossing.rs");
include!("identity_law/audit.rs");
include!("identity_law/vocabulary.rs");
