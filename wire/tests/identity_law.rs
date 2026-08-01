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

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wire::{
    Actor, Answer, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind,
    CursorHead, Derived, Digest, Disposition, Envelope, Fact, Fault, FaultClass, FindingHead,
    FindingState, FoldName, Hello, Instant, LaneResidency, LeaseHead, Name, Origin, Outcome,
    Overflow, Packed, Phase, Producer, Refusal, Reply, ReplyPosition, ResidencyHead, ResidentState,
    Response, Seq, SeqRange, Standing, TaskHandle, TaskHead, TrustHead, TrustState, Up,
    WireVersion, Witness, WitnessClass, WitnessHead, WitnessKind, WorkflowHead, body,
};

/// Names `wire` re-exports that never cross as values, each with its reason.
///
/// Every re-export is either exercised by the law below or listed here. A new
/// DTO that is neither fails `every_crossing_type_obeys_the_identity_law`,
/// which prints the unaccounted name rather than passing quietly.
const NOT_CROSSING: &[(&str, &str)] = &[
    (
        "ActorError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "Body",
        "a trait naming which bodies the vocabulary registers; a bound is not \
         a value and has no bytes",
    ),
    (
        "Law",
        "a registry row held in the binary as static slices; the command it \
         governs crosses, the row itself never leaves the process",
    ),
    (
        "LawError",
        "witness-law refusal; returned to a caller, never encoded",
    ),
    (
        "NameError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "CommandKindError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "FoldNameError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "IdentError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "OriginError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "PhaseError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "SeqRangeError",
        "construction refusal; returned to a caller, never encoded",
    ),
    (
        "TaskHandleError",
        "parse refusal; returned to a caller, never encoded",
    ),
    (
        "Rejected",
        "carries an already-covered Refusal or Fault to a caller; the admitted \
         outcome crosses, the rejection does not",
    ),
    (
        "CommandKind",
        "exercised through Command, whose kind field is this type",
    ),
    (
        "FactKind",
        "exercised through Fact, whose kind field is this type",
    ),
    (
        "FaultClass",
        "exercised through Fault, whose class field is this type",
    ),
    (
        "ReplyPosition",
        "exercised through Reply, whose position field is this type",
    ),
    (
        "Overflow",
        "exercised through Packed, whose overflow field is this type",
    ),
    (
        "WireVersion",
        "exercised through Hello and Envelope, whose version fields are this type",
    ),
];

/// Row used by the packed frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row {
    task: TaskHandle,
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical uuid")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero position")
}

fn handle(text: &str) -> TaskHandle {
    text.parse().expect("task handle")
}

fn digest(nibble: &str) -> Digest {
    nibble.repeat(64).parse().expect("digest spelling")
}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Assert the law for one type's samples and record that the type was covered.
///
/// Checks, per sample: encoding then decoding returns an equal value, and
/// re-encoding the decoded value returns identical bytes. Checks, per ordered
/// pair: byte equality holds exactly when value equality holds.
fn law<T>(covered: &mut BTreeSet<String>, name: &str, samples: &[T])
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    covered.insert(name.to_owned());
    assert!(
        samples.len() >= 2,
        "{name} needs at least two samples for the pairwise check to say anything",
    );

    let encoded: Vec<Vec<u8>> = samples
        .iter()
        .map(|sample| wire::encode(sample).unwrap_or_else(|_| panic!("{name} encodes")))
        .collect();

    for (sample, bytes) in samples.iter().zip(&encoded) {
        let decoded: T = wire::decode(bytes).unwrap_or_else(|_| panic!("{name} decodes"));
        assert_eq!(
            &decoded, sample,
            "{name} lost a distinction its bytes do not carry",
        );

        let again = wire::encode(&decoded).unwrap_or_else(|_| panic!("{name} re-encodes"));
        assert_eq!(&again, bytes, "{name} does not encode stably");
    }

    for (left, left_bytes) in samples.iter().zip(&encoded) {
        for (right, right_bytes) in samples.iter().zip(&encoded) {
            assert_eq!(
                left_bytes == right_bytes,
                left == right,
                "{name}: byte equality and value equality disagree for {left:?} and {right:?}",
            );
        }
    }
}

/// Assert the law plus the hash and order agreements a spelling type owes.
///
/// The spelling newtypes are the narrowing-hazard class: each wraps a richer
/// primitive behind one admitted text. They are also exactly the types that
/// derive `Ord` and `Hash`, so ranked lists and keyed maps depend on those
/// agreeing with what the bytes say.
fn spelling_law<T>(covered: &mut BTreeSet<String>, name: &str, samples: &[T])
where
    T: Serialize + serde::de::DeserializeOwned + Ord + Hash + std::fmt::Debug,
{
    law(covered, name, samples);

    let encoded: Vec<Vec<u8>> = samples
        .iter()
        .map(|sample| wire::encode(sample).unwrap_or_else(|_| panic!("{name} encodes")))
        .collect();

    for (left, left_bytes) in samples.iter().zip(&encoded) {
        for (right, right_bytes) in samples.iter().zip(&encoded) {
            if left_bytes == right_bytes {
                assert_eq!(
                    hash_of(left),
                    hash_of(right),
                    "{name}: identical bytes must hash identically",
                );
            }
            assert_eq!(
                left.cmp(right) == std::cmp::Ordering::Equal,
                left_bytes == right_bytes,
                "{name}: rank and spelling disagree for {left:?} and {right:?}",
            );
        }
    }
}

/// The body every crossing type generic over one carries in these samples.
///
/// This is a registered fact body rather than a shape declared here, because
/// [`Fact`] admits only bodies the vocabulary binds. It serves the command
/// samples too, whose bodies are unconstrained.
fn body(validation: &str) -> body::TaskDone {
    body::TaskDone {
        task: handle("SPICE-aBcD"),
        validation: validation.to_owned(),
    }
}

/// A digest fixture, spelled the one way a digest is spelled.
const DIGEST: &str = "9f2b4c7d1e6a08b35c4f9d2e7a1b8c05d6e3f4a291b7c8d0e5f6a3b2c1d4e7f8";

/// Parse a phase fixture, which is one lower-case word by construction.
fn phase(text: &str) -> Phase {
    text.parse().expect("phase")
}

/// Parse a name fixture, which is one lower-case word by construction.
fn name(text: &str) -> Name {
    text.parse().expect("name")
}

fn at(text: &str) -> Instant {
    text.parse().expect("instant")
}

/// A committed position covering exactly one sequence.
fn committed(position: u64) -> ReplyPosition {
    ReplyPosition::Committed(SeqRange {
        first: seq(position),
        last: seq(position),
    })
}

/// The refusal the reply frames reuse, carrying every optional field.
fn stale_witness() -> Refusal {
    Refusal {
        reason: "witness::stale".to_owned(),
        witness: Some("task".to_owned()),
        expected: Some("48190".to_owned()),
        observed: Some("48213".to_owned()),
        remedy: "re-read the task head and retry".to_owned(),
    }
}

/// The fault the reply frames reuse.
fn moved_epoch() -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: "epoch::moved".to_owned(),
        detail: "resident epoch moved".to_owned(),
    }
}

#[test]
fn every_crossing_type_obeys_the_identity_law() {
    // Coverage accumulates across the families so one assertion can speak for
    // the whole crate; splitting it per family would let a missing family pass.
    let mut covered = BTreeSet::new();

    cover_spellings(&mut covered);
    cover_journal_frames(&mut covered);
    cover_task_heads(&mut covered);
    cover_claim_heads(&mut covered);
    cover_policy_heads(&mut covered);
    cover_residency_heads(&mut covered);
    cover_head_enums(&mut covered);
    cover_command_frames(&mut covered);
    cover_packed_frames(&mut covered);
    cover_refusal_frames(&mut covered);
    cover_reply_frames(&mut covered);
    cover_witness_values(&mut covered);
    cover_vocabularies(&mut covered);

    assert_coverage_is_complete(&covered);
}

/// The spelling newtypes.
///
/// Instant's samples include sub-millisecond, pre-epoch, and boundary readings,
/// which is the narrowing path that broke this law before.
fn cover_spellings(covered: &mut BTreeSet<String>) {
    spelling_law(
        covered,
        "Instant",
        &[
            Instant::new("2030-04-12T18:44:09.271828Z".parse().expect("timestamp")),
            Instant::new("2030-04-12T18:44:09.271Z".parse().expect("timestamp")),
            Instant::new("1969-12-31T23:59:59.999999Z".parse().expect("timestamp")),
            Instant::new(jiff::Timestamp::MAX),
        ],
    );
    spelling_law(
        covered,
        "CanonicalUuid",
        &[
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        ],
    );
    spelling_law(
        covered,
        "TaskHandle",
        &[handle("SPICE-aBcD"), handle("WIRE-1kJJP2Qc")],
    );
    spelling_law(covered, "Phase", &[phase("todo"), phase("review")]);
    spelling_law(
        covered,
        "Origin",
        &[
            "task:SPICE-aBcD".parse::<Origin>().expect("origin"),
            "repo:genesis".parse::<Origin>().expect("origin"),
        ],
    );
    spelling_law(
        covered,
        "FoldName",
        &[
            "board::ready".parse::<FoldName>().expect("fold name"),
            "bus::refusals".parse::<FoldName>().expect("fold name"),
        ],
    );
    spelling_law(
        covered,
        "Actor",
        &[
            Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c2")),
            Actor::Operator,
        ],
    );
    spelling_law(covered, "Seq", &[seq(1), seq(48213)]);
    spelling_law(
        covered,
        "Name",
        &[
            "next".parse::<Name>().expect("name"),
            "reads".parse::<Name>().expect("name"),
        ],
    );
    spelling_law(covered, "Digest", &[digest("a"), digest("b")]);
}

/// The frames that rest in the journal.
///
/// These derive only Eq and inherit the law structurally, so their samples vary
/// one field at a time against a shared base.
fn cover_journal_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "Envelope",
        &[
            Envelope {
                v: WireVersion(1),
                seq: seq(48213),
                at: at("2030-04-12T18:44:09.271Z"),
                actor: Actor::Operator,
                origin: "task:SPICE-aBcD".parse().expect("origin"),
                cause: Some(seq(48190)),
                fact: Fact::new(body("checked")),
            },
            Envelope {
                v: WireVersion(1),
                seq: seq(48214),
                at: at("2030-04-12T18:44:09.272Z"),
                actor: Actor::Operator,
                origin: "task:SPICE-aBcD".parse().expect("origin"),
                cause: None,
                fact: Fact::new(body("cargo test --workspace")),
            },
        ],
    );
    law(
        covered,
        "Fact",
        &[
            Fact::new(body("checked")),
            Fact::new(body("cargo test --workspace")),
        ],
    );

    // The opaque read of the same fact. It obeys the identity law for the same
    // reason the typed one does — a verifier that re-encodes a row it read must
    // get the row back — and the samples are the typed bodies above so a drift
    // between the two readings would show up here as a byte difference.
    law(
        covered,
        "AnyFact",
        &[
            any_fact(Fact::new(body("checked"))),
            any_fact(Fact::new(body("cargo test --workspace"))),
        ],
    );
}

/// Read a typed fact back the way a journal verifier reads a stored row.
fn any_fact<F: wire::Body>(fact: Fact<F>) -> AnyFact {
    let bytes = wire::encode(&fact).expect("a typed fact encodes");
    wire::decode(&bytes).expect("a stored fact reads back without its body type")
}

/// Task statement and workflow heads.
fn cover_task_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let task = handle("SPICE-aBcD");

    law(
        covered,
        "TaskHead",
        &[
            TaskHead {
                handle: task.clone(),
                uuid: first_uuid,
                title: "Build the gate".to_owned(),
                project: "task.gate".to_owned(),
                tags: vec!["guard".to_owned()],
                dependencies: Vec::new(),
                provenance: "repo:genesis".parse().expect("origin"),
                revision: seq(10),
            },
            TaskHead {
                handle: task.clone(),
                uuid: second_uuid,
                title: "Review the gate".to_owned(),
                project: "task.gate".to_owned(),
                tags: vec!["guard".to_owned(), "review".to_owned()],
                dependencies: vec![handle("WIRE-eFgH")],
                provenance: "task:SPICE-aBcD".parse().expect("origin"),
                revision: seq(11),
            },
        ],
    );
    law(
        covered,
        "WorkflowHead",
        &[
            WorkflowHead {
                flow: vec![
                    "todo".parse().expect("phase"),
                    "review".parse().expect("phase"),
                ],
                phase: "todo".parse().expect("phase"),
                status: "pending".to_owned(),
                review: "unassigned".to_owned(),
                assignment: Some(Actor::Operator),
                deferral: Some(at("2030-04-12T18:44:09.271Z")),
                completion: None,
            },
            WorkflowHead {
                flow: vec![
                    "todo".parse().expect("phase"),
                    "review".parse().expect("phase"),
                ],
                phase: "review".parse().expect("phase"),
                status: "completed".to_owned(),
                review: "clean".to_owned(),
                assignment: Some(Actor::Lane(first_uuid)),
                deferral: None,
                completion: Some(at("2030-04-12T18:44:09.272Z")),
            },
        ],
    );
}

/// Claim possession and lease heads.
fn cover_claim_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");

    law(
        covered,
        "ClaimHead",
        &[
            ClaimHead {
                actor: Actor::Operator,
                fact: seq(12),
                lane: first_uuid,
                occupant: second_uuid,
                epoch: first_uuid,
                disposition: ClaimDisposition::Active,
            },
            ClaimHead {
                actor: Actor::Lane(first_uuid),
                fact: seq(13),
                lane: second_uuid,
                occupant: first_uuid,
                epoch: second_uuid,
                disposition: ClaimDisposition::Lapsed,
            },
        ],
    );
    law(
        covered,
        "LeaseHead",
        &[
            LeaseHead {
                start: at("2030-04-12T18:44:09.271Z"),
                renewal: at("2030-04-12T18:44:09.272Z"),
                maximum: at("2030-04-12T18:45:09.271Z"),
                expiry: at("2030-04-12T18:45:00.271Z"),
                epoch: first_uuid,
            },
            LeaseHead {
                start: at("2030-04-12T18:44:10.271Z"),
                renewal: at("2030-04-12T18:44:10.272Z"),
                maximum: at("2030-04-12T18:46:09.271Z"),
                expiry: at("2030-04-12T18:46:00.271Z"),
                epoch: second_uuid,
            },
        ],
    );
}

/// Witness, trust, and gate-finding heads.
fn cover_policy_heads(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "WitnessHead",
        &[
            WitnessHead {
                name: "review".to_owned(),
                kind: WitnessClass::Review,
                predicate: "reviewer differs from author".to_owned(),
                head: seq(14),
            },
            WitnessHead {
                name: "budget".to_owned(),
                kind: WitnessClass::Budget,
                predicate: "remaining equals 2".to_owned(),
                head: seq(15),
            },
        ],
    );
    law(
        covered,
        "TrustHead",
        &[
            TrustHead {
                digest: digest("a").to_string(),
                state: TrustState::Pended,
            },
            TrustHead {
                digest: digest("b").to_string(),
                state: TrustState::Granted,
            },
        ],
    );
    law(
        covered,
        "FindingHead",
        &[
            FindingHead {
                identity: "gate::one".to_owned(),
                severity: "high".to_owned(),
                locus: "wire/src/lib.rs".to_owned(),
                owner: "wire".to_owned(),
                receipt: digest("c").to_string(),
                state: FindingState::Open,
            },
            FindingHead {
                identity: "gate::two".to_owned(),
                severity: "low".to_owned(),
                locus: "wire/src/head.rs".to_owned(),
                owner: "gate".to_owned(),
                receipt: digest("d").to_string(),
                state: FindingState::Cleared,
            },
        ],
    );
}

/// Resident generation, lane, and cursor heads.
fn cover_residency_heads(covered: &mut BTreeSet<String>) {
    let first_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
    let second_uuid = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");

    let occupied = LaneResidency {
        lane: first_uuid,
        occupant: Some(second_uuid),
        lineage: vec![first_uuid],
        capabilities: vec![second_uuid],
        disposition: "seated".to_owned(),
    };
    let empty = LaneResidency {
        lane: second_uuid,
        occupant: None,
        lineage: Vec::new(),
        capabilities: Vec::new(),
        disposition: "empty".to_owned(),
    };
    law(covered, "LaneResidency", &[occupied.clone(), empty.clone()]);
    law(
        covered,
        "ResidencyHead",
        &[
            ResidencyHead {
                epoch: first_uuid,
                state: ResidentState::Seated,
                lanes: vec![occupied],
            },
            ResidencyHead {
                epoch: second_uuid,
                state: ResidentState::Dark,
                lanes: vec![empty],
            },
        ],
    );
    law(
        covered,
        "CursorHead",
        &[
            CursorHead {
                seq: None,
                wake: None,
            },
            CursorHead {
                seq: Some(seq(16)),
                wake: Some(seq(15)),
            },
        ],
    );
}

/// Closed enums carried inside authoritative heads.
fn cover_head_enums(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "ClaimDisposition",
        &[ClaimDisposition::Active, ClaimDisposition::Freed],
    );
    law(
        covered,
        "FindingState",
        &[FindingState::Open, FindingState::Cleared],
    );
    law(
        covered,
        "ResidentState",
        &[ResidentState::Seated, ResidentState::Dark],
    );
    law(
        covered,
        "TrustState",
        &[TrustState::Pended, TrustState::Revoked],
    );
    law(
        covered,
        "WitnessClass",
        &[WitnessClass::Assignment, WitnessClass::Transition],
    );
}

/// The frames a caller sends across the socket.
fn cover_command_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "Command",
        &[
            Command {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                kind: "task::done".parse::<CommandKind>().expect("command kind"),
                expected: vec![Witness::Task {
                    task: handle("SPICE-aBcD"),
                    head: seq(48190),
                }],
                body: body("checked"),
            },
            Command {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: "task::note".parse::<CommandKind>().expect("command kind"),
                expected: Vec::new(),
                body: body("checked"),
            },
        ],
    );
    cover_witness_values(covered);
    law(
        covered,
        "Hello",
        &[
            Hello {
                wire: WireVersion(1),
                capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            },
            Hello {
                wire: WireVersion(1),
                capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            },
        ],
    );
}

/// Concrete command predicates and their nested state vocabularies.
fn cover_witness_values(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "Witness",
        &[
            Witness::Task {
                task: handle("SPICE-aBcD"),
                head: seq(48190),
            },
            Witness::Workflow {
                task: handle("SPICE-aBcD"),
                head: seq(48190),
                phase: "todo".parse().expect("phase"),
            },
            Witness::Directive {
                steer: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                disposition: Disposition::Live,
            },
            Witness::Consent {
                request: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                answer: Answer::Asked,
            },
            Witness::Trust {
                digest: digest("c"),
                standing: Standing::Pended,
            },
            Witness::Transition {
                from: Up::Seated,
                to: Up::Quiescing,
            },
        ],
    );
}

/// The paged frames, sampled with and without an overflow tail.
fn cover_packed_frames(covered: &mut BTreeSet<String>) {
    let rows = vec![Row {
        task: handle("SPICE-aBcD"),
    }];
    let plain = Packed {
        rows: rows.clone(),
        overflow: None,
        horizon: seq(48213),
    };
    let overflowing = Packed {
        rows,
        overflow: Some(Overflow {
            omitted: 7,
            continuation: seq(48220),
        }),
        horizon: seq(48213),
    };

    law(covered, "Packed", &[plain.clone(), overflowing.clone()]);
    law(
        covered,
        "Derived",
        &[
            Derived {
                fold: "board::ready".parse().expect("fold name"),
                r#gen: 12,
                cursor: seq(48213),
                packed: plain,
            },
            Derived {
                fold: "board::ready".parse().expect("fold name"),
                r#gen: 13,
                cursor: seq(48214),
                packed: overflowing,
            },
        ],
    );
}

/// The frames that carry a refusal or a fault back to a caller.
fn cover_refusal_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "SeqRange",
        &[
            SeqRange {
                first: seq(1),
                last: seq(2),
            },
            SeqRange {
                first: seq(1),
                last: seq(3),
            },
        ],
    );
    law(
        covered,
        "Refusal",
        &[
            Refusal {
                reason: "wire::mismatch".to_owned(),
                witness: None,
                expected: None,
                observed: None,
                remedy: "reconnect with the resident's current wire".to_owned(),
            },
            stale_witness(),
        ],
    );
    law(
        covered,
        "Fault",
        &[
            moved_epoch(),
            Fault {
                class: FaultClass::Store,
                reason: "journal::unreadable".to_owned(),
                detail: "journal read failed".to_owned(),
            },
        ],
    );
}

/// The frames a caller reads back after a command lands.
fn cover_reply_frames(covered: &mut BTreeSet<String>) {
    law(
        covered,
        "Reply",
        &[
            Reply {
                position: committed(48213),
                body: body("checked"),
            },
            Reply {
                position: ReplyPosition::Snapshot(seq(48213)),
                body: body("checked"),
            },
        ],
    );
    law(
        covered,
        "Outcome",
        &[
            Outcome::Reply(Reply {
                position: committed(48213),
                body: body("checked"),
            }),
            Outcome::Refusal(stale_witness()),
        ],
    );
    law(
        covered,
        "Response",
        &[
            Response {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                outcome: Outcome::Reply(Reply {
                    position: committed(48213),
                    body: body("checked"),
                }),
            },
            Response {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                outcome: Outcome::Fault(moved_epoch()),
            },
        ],
    );
}

/// Fail by name when a re-exported type is neither exercised nor excused.
fn assert_coverage_is_complete(covered: &BTreeSet<String>) {
    let exported = re_exported_names();
    let excused: BTreeSet<String> = NOT_CROSSING
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();

    let accounted: BTreeSet<String> = covered.union(&excused).cloned().collect();

    let unaccounted: Vec<&String> = exported.difference(&accounted).collect();
    assert!(
        unaccounted.is_empty(),
        "these wire re-exports obey no stated identity law: {unaccounted:?}. \
         Exercise each in every_crossing_type_obeys_the_identity_law, or list \
         it in NOT_CROSSING with the reason it never crosses as a value.",
    );

    let stale: Vec<&String> = accounted.difference(&exported).collect();
    assert!(
        stale.is_empty(),
        "these names are covered or excused but no longer re-exported: {stale:?}",
    );
}

/// The type names `wire/src/lib.rs` re-exports, read from the source.
///
/// Reading the source rather than restating the list is what makes a newly
/// added DTO fail here instead of passing unnoticed.
fn re_exported_names() -> BTreeSet<String> {
    let source = fs::read_to_string(lib_source()).expect("wire/src/lib.rs is readable");

    let mut names = BTreeSet::new();
    let mut rest = source.as_str();
    while let Some(start) = rest.find("pub use ") {
        rest = &rest[start + "pub use ".len()..];
        let end = rest.find(';').expect("a re-export ends in a semicolon");
        let statement = &rest[..end];
        rest = &rest[end..];

        let braced = statement
            .find('{')
            .map(|open| {
                let close = statement.find('}').expect("a braced group closes");
                &statement[open + 1..close]
            })
            .unwrap_or_else(|| {
                statement
                    .rsplit("::")
                    .next()
                    .expect("a re-export names something")
            });

        for name in braced.split(',') {
            let name = name.trim();
            if !name.is_empty() {
                names.insert(name.to_owned());
            }
        }
    }

    assert!(
        !names.is_empty(),
        "no re-exports parsed from wire/src/lib.rs; the parser has drifted from the source",
    );
    names
}

fn lib_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs")
}

#[test]
fn the_excuse_list_stays_anchored_to_the_source() {
    // Coverage itself is asserted by the law test. This asserts the registry
    // that assertion reads against is sound: the parser finds real names, every
    // excuse points at a re-export that still exists, and every excuse says why.
    // Without this, the excuse list could drift into an unchecked wish list.
    let exported = re_exported_names();
    assert!(
        exported.contains("Instant"),
        "the re-export parser lost a name it should have found",
    );

    let excused: BTreeSet<String> = NOT_CROSSING
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    let unknown: Vec<&String> = excused.difference(&exported).collect();
    assert!(
        unknown.is_empty(),
        "NOT_CROSSING excuses names that are not re-exported: {unknown:?}",
    );

    for (name, reason) in NOT_CROSSING {
        assert!(
            !reason.trim().is_empty(),
            "{name} is excused without a reason",
        );
    }

    // Two rows for one name is how a merge leaves this list: both sides excuse
    // the same type and only the first reason is ever read, so the second is a
    // claim about the crate that nothing checks.
    let mut seen = BTreeSet::new();
    let repeated: Vec<&str> = NOT_CROSSING
        .iter()
        .filter(|(name, _)| !seen.insert(*name))
        .map(|(name, _)| *name)
        .collect();
    assert!(
        repeated.is_empty(),
        "NOT_CROSSING excuses these names more than once, so a reason is going \
         unread: {repeated:?}",
    );
}

#[test]
fn a_seq_cannot_encode_a_zero_position() {
    // Seq wraps NonZeroU64, so the law above can never sample a zero. The
    // refusal belongs to the decoder, not to a caller-side check.
    let refused = wire::decode::<Seq>(b"0".as_slice());
    assert!(refused.is_err(), "zero is not a position");

    let admitted: Seq = wire::decode(b"1".as_slice()).expect("one is a position");
    assert_eq!(admitted.get(), NonZeroU64::new(1).expect("non-zero").get());
}

/// The closed sets a frame names rather than spells out.
///
/// Each is a fixed vocabulary, so the law that matters is that one variant has
/// exactly one spelling and two variants never share one. Every set here goes
/// through `spelling_law` rather than the weaker helper: these types derive
/// `Ord` and `Hash`, so they owe rank-and-spelling agreement too. `Law` holds
/// `WitnessKind` in static slices and asks `contains`, and any of them can key
/// a map or sort a rendered column.
///
/// The sweep is over the whole variant set rather than a sample, because a
/// closed set is exactly the case where the whole set is nameable: `ALL` and
/// the enumerated arms cost the same to write as two values and catch the
/// variant a sample would skip.
fn cover_vocabularies(covered: &mut BTreeSet<String>) {
    spelling_law(covered, "Producer", Producer::ALL);
    spelling_law(covered, "WitnessKind", WitnessKind::ALL);
    spelling_law(
        covered,
        "Disposition",
        &[
            Disposition::Live,
            Disposition::Acked,
            Disposition::Declined,
            Disposition::Lapsed,
            Disposition::Canceled,
        ],
    );
    spelling_law(
        covered,
        "Answer",
        &[
            Answer::Asked,
            Answer::Granted,
            Answer::Denied,
            Answer::Queued,
        ],
    );
    spelling_law(
        covered,
        "Standing",
        &[Standing::Pended, Standing::Granted, Standing::Revoked],
    );
    spelling_law(
        covered,
        "Up",
        &[
            Up::Seated,
            Up::Quiescing,
            Up::Draining,
            Up::Terminating,
            Up::Killing,
            Up::Verifying,
            Up::Dark,
        ],
    );
    spelling_law(
        covered,
        "Name",
        &[name("polling"), name("aliases"), name("legacy")],
    );
    spelling_law(
        covered,
        "Digest",
        &[
            DIGEST.parse::<Digest>().expect("digest"),
            "0000000000000000000000000000000000000000000000000000000000000000"
                .parse::<Digest>()
                .expect("digest"),
        ],
    );
}
