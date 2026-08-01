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
use wire::{
    Accepted, Actor, Answer, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command,
    CommandKind, CursorHead, Derived, Digest, Disposition, Envelope, Fact, FactKind, Fault,
    FaultClass, FindingHead, FindingState, FoldName, Hello, Instant, LaneResidency, LeaseHead,
    Name, Origin, Outcome, Overflow, Packed, Phase, Producer, Refusal, Reply, ReplyPosition,
    ResidencyHead, ResidentState, Response, Seq, SeqRange, TaskHandle, TaskHead, TrustHead,
    TrustState, WireVersion, Witness, WitnessClass, WitnessHead, WitnessKind, WorkflowHead, body,
};

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
    law_under(covered, name, samples, encoded_by_wire);
}

/// The encoder a type's identity binds, for types where that is `wire::encode`.
///
/// Named rather than written inline at each call so the registry states which
/// encoding it checked. A type whose identity binds a different encoding names
/// that one instead; see [`canonical_command_identity`].
fn encoded_by_wire<T: Serialize>(value: &T) -> Vec<u8> {
    wire::encode(value).expect("value encodes")
}

/// The encoder `Command` identity binds.
///
/// A command's identity is its canonical rendering, not `wire::encode`'s.
/// The two differ exactly where the difference matters: canonicalization sorts
/// `expected`, so a client that lists the same predicates in another order
/// spells the same request. Checking `Command` under the generic encoder would
/// report the type covered while testing bytes nothing takes an identity over.
fn canonical_command_identity<B: Serialize>(command: &Command<B>) -> Vec<u8> {
    command
        .canonical_json()
        .expect("command canonicalizes")
        .into_bytes()
}

/// Assert the law for one type's samples under the encoder that type's
/// identity actually binds.
///
/// The encoder is a parameter rather than always [`encoded_by_wire`] because a
/// type is free to bind a narrower rendering than the generic one, and only the
/// bound rendering is the one anything takes an identity over. Each call names
/// which it checked.
fn law_under<T, E>(covered: &mut BTreeSet<String>, name: &str, samples: &[T], encode: E)
where
    T: serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    E: Fn(&T) -> Vec<u8>,
{
    covered.insert(name.to_owned());
    assert!(
        samples.len() >= 2,
        "{name} needs at least two samples for the pairwise check to say anything",
    );

    let encoded: Vec<Vec<u8>> = samples.iter().map(&encode).collect();

    for (sample, bytes) in samples.iter().zip(&encoded) {
        let decoded: T = wire::decode(bytes).unwrap_or_else(|_| panic!("{name} decodes"));
        assert_eq!(
            &decoded, sample,
            "{name} lost a distinction its bytes do not carry",
        );

        let again = encode(&decoded);
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

/// Assert the hash agreement every covered `Hash` type owes.
///
/// Stated so it says something for every such type rather than only for the
/// types that happen to carry two samples encoding alike. A decoded round trip
/// is a second value equal to the first, so this exercises the `Eq`-`Hash`
/// contract across the wire once per sample; the pairwise arm then catches a
/// type whose distinct-looking samples spell the same bytes.
fn hash_law<T>(name: &str, samples: &[T], encoded: &[Vec<u8>])
where
    T: serde::de::DeserializeOwned + Hash,
{
    for (sample, bytes) in samples.iter().zip(encoded) {
        let decoded: T = wire::decode(bytes).unwrap_or_else(|_| panic!("{name} decodes"));
        assert_eq!(
            hash_of(sample),
            hash_of(&decoded),
            "{name}: a value and its round trip hash differently, so a keyed \
             lookup would miss what the bytes say is the same value",
        );
    }

    for (left, left_bytes) in samples.iter().zip(encoded) {
        for (right, right_bytes) in samples.iter().zip(encoded) {
            if left_bytes == right_bytes {
                assert_eq!(
                    hash_of(left),
                    hash_of(right),
                    "{name}: identical bytes must hash identically",
                );
            }
        }
    }
}

/// Assert the law plus the agreements a type ranked by its spelling owes.
///
/// These are the narrowing-hazard types: each crosses as one admitted text,
/// and each ranks by that text and nothing else. The order assertion is
/// directional. Asserting only that `cmp` is `Equal` exactly when the bytes
/// match leaves `Less` and `Greater` free to point the opposite way from the
/// spelling, which is what a ranked list actually reads, and `Actor` did point
/// the opposite way: `Operator` is declared before `Lane(..)` while `lane:`
/// spells before `operator`.
///
/// A type whose rank is a declared sequence rather than its text belongs in
/// [`ranked_law`] instead.
fn spelling_law<T>(covered: &mut BTreeSet<String>, name: &str, samples: &[T])
where
    T: Serialize + serde::de::DeserializeOwned + Ord + Hash + std::fmt::Debug,
{
    law(covered, name, samples);

    let encoded: Vec<Vec<u8>> = samples.iter().map(encoded_by_wire).collect();
    hash_law(name, samples, &encoded);

    for bytes in &encoded {
        assert_eq!(
            bytes.first(),
            Some(&b'"'),
            "{name}: claims to rank by its spelling but does not cross as text, \
             so comparing encoded bytes would not be comparing that spelling",
        );
    }

    for (left, left_bytes) in samples.iter().zip(&encoded) {
        for (right, right_bytes) in samples.iter().zip(&encoded) {
            assert_eq!(
                left.cmp(right),
                left_bytes.cmp(right_bytes),
                "{name}: rank and spelling disagree for {left:?} and {right:?}; \
                 the value ranks {:?} while its bytes rank {:?}",
                left.cmp(right),
                left_bytes.cmp(right_bytes),
            );
        }
    }
}

/// Assert the law plus the agreements a type ranked by a declared sequence
/// owes, given its samples in ascending order.
///
/// Not every ordered type ranks by its spelling, and asserting that it does
/// would be false rather than strict. [`Seq`] ranks numerically while spelling
/// decimal digits, so `9` ranks before `10` and spells after it; a closed
/// vocabulary like [`ResidentState`] ranks by how far a shutdown has
/// progressed, which is the order the arms are declared in. For these the
/// ascending order is the thing worth pinning, so it is written out at the call
/// site and compared against `cmp` directly.
///
/// The fixture then requires at least one adjacent pair the spelling ranks the
/// other way. A declared order that agrees with its spelling at every step is
/// a spelling order wearing a different name, and belongs in [`spelling_law`]
/// where the whole sequence is checked rather than the sequence someone
/// remembered to list.
fn ranked_law<T>(covered: &mut BTreeSet<String>, name: &str, ascending: &[T])
where
    T: Serialize + serde::de::DeserializeOwned + Ord + Hash + std::fmt::Debug,
{
    law(covered, name, ascending);

    let encoded: Vec<Vec<u8>> = ascending.iter().map(encoded_by_wire).collect();
    hash_law(name, ascending, &encoded);

    for (left, right) in ascending.iter().zip(ascending.iter().skip(1)) {
        assert!(
            left < right,
            "{name}: samples are given ascending but {left:?} does not rank \
             before {right:?}",
        );
    }

    let contradicted = encoded
        .iter()
        .zip(encoded.iter().skip(1))
        .any(|(left, right)| left > right);
    assert!(
        contradicted,
        "{name}: every step of the declared order also ascends by spelling, so \
         this proves nothing the stricter spelling_law would not prove; either \
         move it there or extend the samples to the pair that disagrees",
    );
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

fn task_witness() -> Witness {
    Witness::Task {
        task: handle("SPICE-aBcD"),
        head: seq(48190),
    }
}

fn workflow_witness() -> Witness {
    Witness::Workflow {
        task: handle("SPICE-aBcD"),
        head: seq(48190),
        phase: "todo".parse().expect("phase"),
    }
}

/// A command differing from its siblings only in the predicates it names.
fn command(kind: &str, expected: Vec<Witness>) -> Command<body::TaskDone> {
    Command {
        v: WireVersion(1),
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
        kind: kind.parse::<CommandKind>().expect("command kind"),
        expected,
        body: body("checked"),
    }
}

/// Witness order is not part of what a command asked for.
///
/// The two identities a command could have — its canonical bytes and its
/// value equality — have to agree, or a caller that de-duplicates by value and
/// a journal that de-duplicates by bytes would disagree about whether they had
/// already seen a request. They previously did disagree: canonicalization
/// sorted `expected` while derived equality compared the vector position-wise.
#[test]
fn a_command_is_the_predicates_it_names_not_the_order_it_named_them() {
    let asked = command("task::done", vec![task_witness(), workflow_witness()]);
    let reordered = command("task::done", vec![workflow_witness(), task_witness()]);

    assert_eq!(
        canonical_command_identity(&asked),
        canonical_command_identity(&reordered),
        "canonical identity must sort witness order away",
    );
    assert_eq!(
        asked, reordered,
        "two commands the wire cannot tell apart must not compare unequal",
    );
}

/// Naming a predicate twice is not the same as naming it once.
///
/// Canonicalization sorts and deliberately does not deduplicate, so
/// multiplicity survives into the bytes. Equality has to see what the bytes
/// see, or the collision the sort closes would reopen one field over.
#[test]
fn a_command_keeps_the_multiplicity_its_canonical_bytes_keep() {
    let once = command("task::done", vec![task_witness()]);
    let twice = command("task::done", vec![task_witness(), task_witness()]);

    assert_ne!(
        canonical_command_identity(&once),
        canonical_command_identity(&twice),
        "a repeated predicate must survive canonicalization",
    );
    assert_ne!(
        once, twice,
        "equality must keep the multiplicity the bytes keep",
    );
}

/// An actor ranks by the text it crosses as, not by where it was declared.
///
/// This is the pair that proves the fixture's order assertion reads direction
/// rather than only equality. `Operator` is declared before `Lane(..)` while
/// `lane:` spells before `operator`, so a derived order and the bytes point
/// opposite ways here and an `Equal`-only assertion passes over it.
#[test]
fn a_lane_ranks_before_the_operator_because_its_spelling_does() {
    let lane = Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"));
    let operator = Actor::Operator;

    assert!(
        lane.to_string() < operator.to_string(),
        "the spelling this regression rests on changed",
    );
    assert!(
        lane < operator,
        "a lane must rank before the operator, as their spellings do",
    );
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
            Instant::new("0000-01-01T00:00:00Z".parse().expect("minimum wire year"))
                .expect("minimum wire instant"),
            Instant::new("2030-04-12T18:44:09.271828Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new("2030-04-12T18:44:09.271Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new("1969-12-31T23:59:59.999999Z".parse().expect("timestamp"))
                .expect("reading is in the wire year range"),
            Instant::new(jiff::Timestamp::MAX).expect("maximum wire instant"),
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
    // A position spells decimal digits but ranks as a number, and the journal
    // reads it as a number: `9` before `10`, which is the reverse of how the
    // two spell. Samples span that step so the fixture pins the numeric order
    // rather than a lexical one that happens to agree on the samples chosen.
    ranked_law(covered, "Seq", &[seq(1), seq(9), seq(10), seq(48213)]);
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
                digest: digest("a"),
                state: TrustState::Pended,
            },
            TrustHead {
                digest: digest("b"),
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
                receipt: digest("c"),
                state: FindingState::Open,
            },
            FindingHead {
                identity: "gate::two".to_owned(),
                severity: "low".to_owned(),
                locus: "wire/src/head.rs".to_owned(),
                owner: "gate".to_owned(),
                receipt: digest("d"),
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
        "WitnessClass",
        &[WitnessClass::Assignment, WitnessClass::Transition],
    );
}

/// The frames a caller sends across the socket.
///
/// A command is checked under the encoder its identity binds rather than the
/// generic one, so the pairwise arm reads canonical bytes. Its samples carry
/// several witnesses in more than one order, because a single-witness sample
/// cannot tell a sorted encoding from an unsorted one.
fn cover_command_frames(covered: &mut BTreeSet<String>) {
    law_under(
        covered,
        "Command",
        &[
            command("task::done", vec![task_witness()]),
            command("task::done", vec![task_witness(), workflow_witness()]),
            command("task::done", vec![workflow_witness(), task_witness()]),
            command("task::done", vec![task_witness(), task_witness()]),
            Command {
                v: WireVersion(1),
                id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
                kind: "task::note".parse::<CommandKind>().expect("command kind"),
                expected: Vec::new(),
                body: body("checked"),
            },
        ],
        canonical_command_identity,
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
    law(
        covered,
        "Accepted",
        &[
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Operator,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Spice,
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
            Accepted {
                wire: WireVersion(1),
                actor: Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c4")),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"),
            },
        ],
    );
}

/// Concrete command predicates and their nested state vocabularies.
///
/// Every type here derives `Ord` and `Hash` over a declared sequence rather
/// than over its spelling, so each states that sequence in full and the
/// fixture pins it. Reordering a variant is then a failing test naming the
/// pair that moved, not a silent change to how a rendered column sorts.
fn cover_witness_values(covered: &mut BTreeSet<String>) {
    ranked_law(
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
                standing: TrustState::Pended,
            },
            Witness::Transition {
                from: ResidentState::Seated,
                to: ResidentState::Quiescing,
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

/// Fail by name when a re-exported type has no executable or structural proof.
fn assert_coverage_is_complete(covered: &BTreeSet<String>) {
    let source = fs::read_to_string(lib_source()).expect("wire/src/lib.rs is readable");
    let exports = parse_exports(&source).expect("wire/src/lib.rs re-exports parse");
    let modules = read_module_sources(&exports);
    let audit = audit_coverage(&source, &modules, covered).expect("wire source declarations parse");

    assert!(
        audit.unaccounted.is_empty(),
        "these wire re-exports obey no stated identity law: {:?}. Exercise a \
         custom crossing type in every_crossing_type_obeys_the_identity_law. \
         Mark a process-only type beside its declaration with `identity-law: \
         non-crossing - <reason>`.",
        audit.unaccounted,
    );
    assert!(
        audit.stale.is_empty(),
        "these names have law samples but are no longer re-exported: {:?}",
        audit.stale,
    );
}

#[derive(Debug)]
struct Export {
    module: String,
    source_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclarationClass {
    Structural,
    Error,
    Trait,
    NonCrossing,
    Custom,
}

#[derive(Debug, PartialEq, Eq)]
struct CoverageAudit {
    unaccounted: BTreeMap<String, String>,
    stale: BTreeSet<String>,
}

/// Parse root re-exports and classify their declarations where those declarations live.
fn audit_coverage(
    root_source: &str,
    module_sources: &BTreeMap<String, String>,
    covered: &BTreeSet<String>,
) -> Result<CoverageAudit, String> {
    let exports = parse_exports(root_source)?;
    let mut declarations = BTreeMap::new();

    for (module, source) in module_sources {
        let file = syn::parse_file(source)
            .map_err(|error| format!("could not parse module {module}: {error}"))?;
        for item in &file.items {
            if let Some((name, class)) = classify_declaration(item)? {
                declarations.insert((module.clone(), name), class);
            }
        }
    }

    let mut unaccounted = BTreeMap::new();
    for (exported_name, export) in &exports {
        if covered.contains(exported_name) {
            continue;
        }

        let key = (export.module.clone(), export.source_name.clone());
        match declarations.get(&key) {
            Some(
                DeclarationClass::Structural
                | DeclarationClass::Error
                | DeclarationClass::Trait
                | DeclarationClass::NonCrossing,
            ) => {}
            Some(DeclarationClass::Custom) => {
                unaccounted.insert(
                    exported_name.clone(),
                    format!(
                        "{}::{} has custom or potentially narrowing encoding",
                        export.module, export.source_name
                    ),
                );
            }
            None => {
                unaccounted.insert(
                    exported_name.clone(),
                    format!(
                        "{}::{} has no source declaration the audit can classify",
                        export.module, export.source_name
                    ),
                );
            }
        }
    }

    let exported: BTreeSet<String> = exports.into_keys().collect();
    Ok(CoverageAudit {
        unaccounted,
        stale: covered.difference(&exported).cloned().collect(),
    })
}

fn parse_exports(source: &str) -> Result<BTreeMap<String, Export>, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut exports = BTreeMap::new();
    for item in file.items {
        let Item::Use(item_use) = item else {
            continue;
        };
        if !matches!(item_use.vis, Visibility::Public(_)) {
            continue;
        }
        collect_exports(&item_use.tree, &mut Vec::new(), &mut exports)?;
    }
    if exports.is_empty() {
        return Err("no public re-exports found".to_owned());
    }
    Ok(exports)
}

fn collect_exports(
    tree: &UseTree,
    path: &mut Vec<String>,
    exports: &mut BTreeMap<String, Export>,
) -> Result<(), String> {
    match tree {
        UseTree::Path(item) => {
            path.push(item.ident.to_string());
            collect_exports(&item.tree, path, exports)?;
            path.pop();
        }
        UseTree::Name(item) => {
            insert_export(
                path,
                item.ident.to_string(),
                item.ident.to_string(),
                exports,
            )?;
        }
        UseTree::Rename(item) => {
            insert_export(
                path,
                item.ident.to_string(),
                item.rename.to_string(),
                exports,
            )?;
        }
        UseTree::Group(item) => {
            for child in &item.items {
                collect_exports(child, path, exports)?;
            }
        }
        UseTree::Glob(_) => {
            return Err("glob re-exports cannot prove which wire types crossed".to_owned());
        }
    }
    Ok(())
}

fn insert_export(
    path: &[String],
    source_name: String,
    exported_name: String,
    exports: &mut BTreeMap<String, Export>,
) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!(
            "root re-export {exported_name} has no owning module"
        ));
    }
    let export = Export {
        module: path.join("::"),
        source_name,
    };
    if exports.insert(exported_name.clone(), export).is_some() {
        return Err(format!("root name {exported_name} is re-exported twice"));
    }
    Ok(())
}

fn read_module_sources(exports: &BTreeMap<String, Export>) -> BTreeMap<String, String> {
    let source_dir = lib_source()
        .parent()
        .expect("wire/src/lib.rs has a parent")
        .to_path_buf();
    let modules: BTreeSet<&str> = exports
        .values()
        .map(|export| export.module.as_str())
        .collect();

    modules
        .into_iter()
        .map(|module| {
            let path = source_dir
                .join(module.replace("::", "/"))
                .with_extension("rs");
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
            (module.to_owned(), source)
        })
        .collect()
}

fn classify_declaration(item: &Item) -> Result<Option<(String, DeclarationClass)>, String> {
    match item {
        Item::Struct(item) if matches!(item.vis, Visibility::Public(_)) => Ok(Some((
            item.ident.to_string(),
            classify_data(&item.attrs, fields_attributes(&item.fields))?,
        ))),
        Item::Enum(item) if matches!(item.vis, Visibility::Public(_)) => {
            let nested = item.variants.iter().flat_map(|variant| {
                variant
                    .attrs
                    .iter()
                    .chain(fields_attributes(&variant.fields))
            });
            Ok(Some((
                item.ident.to_string(),
                classify_data(&item.attrs, nested)?,
            )))
        }
        Item::Trait(item) if matches!(item.vis, Visibility::Public(_)) => {
            Ok(Some((item.ident.to_string(), DeclarationClass::Trait)))
        }
        Item::Type(item) if matches!(item.vis, Visibility::Public(_)) => {
            Ok(Some((item.ident.to_string(), DeclarationClass::Custom)))
        }
        _ => Ok(None),
    }
}

fn fields_attributes(fields: &Fields) -> impl Iterator<Item = &Attribute> {
    fields.iter().flat_map(|field| field.attrs.iter())
}

fn classify_data<'a>(
    attrs: &[Attribute],
    nested_attrs: impl Iterator<Item = &'a Attribute>,
) -> Result<DeclarationClass, String> {
    if has_non_crossing_marker(attrs)? {
        return Ok(DeclarationClass::NonCrossing);
    }

    let derives = derive_names(attrs)?;
    if derives.contains("Error") {
        return Ok(DeclarationClass::Error);
    }

    let nested_attrs: Vec<&Attribute> = nested_attrs.collect();
    let structural = derives.contains("Serialize")
        && derives.contains("Deserialize")
        && derives.contains("PartialEq")
        && !has_narrowing_serde(attrs.iter().chain(nested_attrs))?;
    Ok(if structural {
        DeclarationClass::Structural
    } else {
        DeclarationClass::Custom
    })
}

fn derive_names(attrs: &[Attribute]) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("derive")) {
        let paths = attr
            .parse_args_with(syn::punctuated::Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .map_err(|error| format!("could not parse derive attribute: {error}"))?;
        for path in paths {
            let name = path
                .segments
                .last()
                .expect("a derive path has one segment")
                .ident
                .to_string();
            names.insert(name);
        }
    }
    Ok(names)
}

fn has_narrowing_serde<'a>(attrs: impl Iterator<Item = &'a Attribute>) -> Result<bool, String> {
    const NARROWING: &[&str] = &[
        "flatten",
        "from",
        "into",
        "skip",
        "skip_deserializing",
        "skip_serializing",
        "skip_serializing_if",
        "try_from",
        "with",
        "deserialize_with",
        "serialize_with",
    ];

    for attr in attrs.filter(|attr| attr.path().is_ident("serde")) {
        let metas = attr
            .parse_args_with(syn::punctuated::Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|error| format!("could not parse serde attribute: {error}"))?;
        for meta in metas {
            let Some(name) = meta.path().segments.last() else {
                continue;
            };
            if NARROWING.contains(&name.ident.to_string().as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn has_non_crossing_marker(attrs: &[Attribute]) -> Result<bool, String> {
    let mut found = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(meta) = &attr.meta else {
            continue;
        };
        let syn::Expr::Lit(expression) = &meta.value else {
            continue;
        };
        let syn::Lit::Str(line) = &expression.lit else {
            continue;
        };
        let line = line.value();
        let line = line.trim();
        if !line.contains("identity-law:") {
            continue;
        }
        let reason = line
            .strip_prefix("identity-law: non-crossing -")
            .ok_or_else(|| format!("malformed identity-law source marker: {line:?}"))?
            .trim();
        if reason.is_empty() {
            return Err("identity-law non-crossing marker has no reason".to_owned());
        }
        if found {
            return Err("declaration has more than one identity-law source marker".to_owned());
        }
        found = true;
    }
    Ok(found)
}

fn lib_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lib.rs")
}

#[test]
fn a_concurrent_structural_export_needs_no_shared_registry_edit() {
    let root = "pub use landed::{ConcurrentDto, ParseError, ProcessOnly};";
    let module = r#"
        #[derive(Serialize, Deserialize, PartialEq)]
        #[serde(deny_unknown_fields)]
        pub struct ConcurrentDto { pub value: String }

        #[derive(Debug, thiserror::Error)]
        #[error("bad input")]
        pub struct ParseError;

        /// identity-law: non-crossing - process-local registry metadata.
        pub struct ProcessOnly;
    "#;
    let modules = BTreeMap::from([("landed".to_owned(), module.to_owned())]);
    let audit = audit_coverage(root, &modules, &BTreeSet::new()).expect("fixture parses");

    assert_eq!(
        audit,
        CoverageAudit {
            unaccounted: BTreeMap::new(),
            stale: BTreeSet::new(),
        }
    );
}

#[test]
fn a_custom_encoder_still_requires_executable_law_samples() {
    let root = "pub use landed::CustomCodec;";
    let module = r#"
        #[derive(Serialize, Deserialize, PartialEq)]
        pub struct CustomCodec {
            #[serde(skip)]
            hidden: String,
        }
    "#;
    let modules = BTreeMap::from([("landed".to_owned(), module.to_owned())]);
    let uncovered = audit_coverage(root, &modules, &BTreeSet::new()).expect("fixture parses");
    assert_eq!(
        uncovered.unaccounted.keys().collect::<Vec<_>>(),
        vec!["CustomCodec"]
    );

    let covered = BTreeSet::from(["CustomCodec".to_owned()]);
    let sampled = audit_coverage(root, &modules, &covered).expect("fixture parses");
    assert!(sampled.unaccounted.is_empty());
    assert!(sampled.stale.is_empty());
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
/// exactly one spelling and two variants never share one. Every set here also
/// derives `Ord` and `Hash`, so each owes a stated order: `Law` holds
/// `WitnessKind` in static slices and asks `contains`, and any of them can key
/// a map or sort a rendered column.
///
/// The enums go through [`ranked_law`] because a derived `Ord` ranks by the
/// arm's position, and these arms are declared in the order the domain reads
/// in — a producer chain, a lifecycle, a retirement path — which no vocabulary
/// here spells alphabetically. [`Name`] and [`Digest`] go through
/// [`spelling_law`] instead: both wrap the text itself, so the text is the
/// rank.
///
/// The sweep is over the whole variant set rather than a sample, because a
/// closed set is exactly the case where the whole set is nameable: `ALL` and
/// the enumerated arms cost the same to write as two values and catch the
/// variant a sample would skip. Listing them in declared order is what
/// `ranked_law` wants anyway, so the sweep and the order assertion are the same
/// list.
fn cover_vocabularies(covered: &mut BTreeSet<String>) {
    ranked_law(covered, "CommandKind", CommandKind::ALL);
    ranked_law(covered, "FactKind", FactKind::ALL);
    ranked_law(covered, "Producer", Producer::ALL);
    ranked_law(covered, "WitnessKind", WitnessKind::ALL);
    ranked_law(
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
    ranked_law(
        covered,
        "Answer",
        &[
            Answer::Asked,
            Answer::Granted,
            Answer::Denied,
            Answer::Queued,
        ],
    );
    ranked_law(
        covered,
        "TrustState",
        &[TrustState::Pended, TrustState::Granted, TrustState::Revoked],
    );
    ranked_law(
        covered,
        "ResidentState",
        &[
            ResidentState::Seated,
            ResidentState::Quiescing,
            ResidentState::Draining,
            ResidentState::Terminating,
            ResidentState::Killing,
            ResidentState::Verifying,
            ResidentState::Dark,
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
