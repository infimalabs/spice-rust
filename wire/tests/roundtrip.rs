//! Round-trip fixtures for the crossing types.
//!
//! The envelope, hello, command, and reply fixtures are the literal documents
//! printed in WIRE.md. Decoding those bytes and re-encoding them to the same
//! value proves the field names and shapes are the specified ones rather than
//! a paraphrase of them.

use serde::{Deserialize, Serialize};
use wire::{
    Actor, CanonicalUuid, Command, Envelope, Fact, FactKind, Fault, FaultClass, Hello, Instant,
    Origin, Outcome, Overflow, Packed, Refusal, Rejected, Reply, ReplyPosition, Response, Seq,
    SeqRange, TaskHandle, WireVersion, Witness, body,
};

/// Body of the `task::done` command printed in WIRE.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DoneBody {
    task: TaskHandle,
    validation: String,
}

/// Body of the reply printed in WIRE.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DoneResult {
    task: TaskHandle,
    status: String,
}

/// A row used by the packed-frame fixtures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row {
    task: TaskHandle,
}

/// Decode, re-encode, and assert the JSON value is unchanged.
fn round_trip<T>(document: &str) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let decoded: T = wire::decode(document.as_bytes()).expect("document decodes");
    let reencoded: serde_json::Value =
        serde_json::from_slice(&wire::encode(&decoded).expect("value encodes"))
            .expect("re-encoded bytes are json");
    let original: serde_json::Value = serde_json::from_str(document).expect("document is json");
    assert_eq!(reencoded, original, "round trip changed the document");
    decoded
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical uuid")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero position")
}

const ENVELOPE: &str = r#"{
  "v": 1,
  "seq": 48213,
  "at": "2030-04-12T18:44:09.271Z",
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2",
  "origin": "task:SPICE-aBcD",
  "cause": 48190,
  "fact": {
    "kind": "task::phased",
    "task": "SPICE-aBcD",
    "phase": "verify"
  }
}"#;

const HELLO: &str = r#"{
  "wire": 1,
  "capability": "018f82d4-6f59-7d06-92e3-bce9131163c6",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
}"#;

const COMMAND: &str = r#"{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "kind": "task::done",
  "expected": [
    { "kind": "task", "task": "SPICE-aBcD", "head": 48168 },
    { "kind": "workflow", "task": "SPICE-aBcD", "head": 48169, "phase": "verify" },
    { "kind": "claim", "task": "SPICE-aBcD", "head": 48172 },
    { "kind": "lease", "task": "SPICE-aBcD", "head": 48173 },
    {
      "kind": "residency",
      "lane": "018f82d4-6f59-7d06-92e3-bce9131163c2",
      "occupant": "018f82d4-6f59-7d06-92e3-bce9131163c5",
      "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3"
    }
  ],
  "body": {
    "task": "SPICE-aBcD",
    "validation": "cargo test --workspace"
  }
}"#;

const REPLY: &str = r#"{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "outcome": {
    "kind": "reply",
    "seq": { "first": 48213, "last": 48214 },
    "body": { "task": "SPICE-aBcD", "status": "done" }
  }
}"#;

#[test]
fn envelope_matches_the_specified_document() {
    let envelope: Envelope<body::TaskPhased> = round_trip(ENVELOPE);

    assert_eq!(envelope.v, WireVersion::CURRENT);
    assert_eq!(envelope.seq, seq(48213));
    assert_eq!(
        envelope.actor,
        Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"))
    );
    assert_eq!(
        envelope.origin,
        "task:SPICE-aBcD".parse::<Origin>().expect("origin")
    );
    assert_eq!(envelope.cause, Some(seq(48190)));
    assert_eq!(envelope.fact.kind, FactKind::TaskPhased);
    assert_eq!(envelope.fact.body.phase.as_str(), "verify");
    assert!(!envelope.is_root());
}

#[test]
fn envelope_omits_cause_at_a_chain_root() {
    let root = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(1),
        at: "2030-04-12T18:44:09.271Z".parse::<Instant>().expect("at"),
        actor: Actor::Spice,
        origin: "task:SPICE-aBcD".parse::<Origin>().expect("origin"),
        cause: None,
        fact: Fact::new(body::TaskPhased {
            task: "SPICE-aBcD".parse().expect("handle"),
            phase: "todo".parse().expect("phase"),
        }),
    };

    let encoded = String::from_utf8(wire::encode(&root).expect("encodes")).expect("utf8");
    assert!(
        !encoded.contains("cause"),
        "a chain root omits cause entirely rather than sending a sentinel: {encoded}"
    );

    let decoded: Envelope<body::TaskPhased> = wire::decode(encoded.as_bytes()).expect("decodes");
    assert_eq!(decoded, root);
    assert!(decoded.is_root());
}

#[test]
fn a_zero_or_null_cause_sentinel_is_refused() {
    let zero = ENVELOPE.replace("\"cause\": 48190", "\"cause\": 0");
    let null = ENVELOPE.replace("\"cause\": 48190", "\"cause\": null");

    assert!(
        wire::decode::<Envelope<body::TaskPhased>>(zero.as_bytes()).is_err(),
        "zero is not a position"
    );
    assert!(
        wire::decode::<Envelope<body::TaskPhased>>(null.as_bytes()).is_err(),
        "null is not a position"
    );
}

#[test]
fn hello_matches_the_specified_document() {
    let hello: Hello = round_trip(HELLO);

    assert_eq!(hello.wire, WireVersion::CURRENT);
    assert_eq!(
        hello.capability,
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c6")
    );
    assert!(
        hello
            .admit(uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"))
            .is_ok()
    );
}

#[test]
fn a_wire_mismatch_refuses_and_names_both_versions() {
    let hello = Hello {
        wire: WireVersion(2),
        capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
    };

    let rejected = hello
        .admit(uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"))
        .expect_err("a foreign version does not open a session");

    let Rejected::Wire(refusal) = rejected else {
        panic!("a version this resident does not speak is a refusal, not a fault");
    };
    assert_eq!(refusal.expected.as_deref(), Some("1"));
    assert_eq!(refusal.observed.as_deref(), Some("2"));
}

#[test]
fn an_epoch_mismatch_faults_and_names_both_epochs() {
    let client = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let resident = uuid("018f82d4-6f59-7d06-92e3-bce9131163c9");
    let hello = Hello {
        wire: WireVersion::CURRENT,
        capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c6"),
        epoch: client,
    };

    let rejected = hello
        .admit(resident)
        .expect_err("a replaced generation does not open a session");

    let Rejected::Epoch(fault) = rejected else {
        panic!("a stale epoch is a fault, not a refusal");
    };
    assert_eq!(fault.class, FaultClass::Protocol);
    assert!(
        fault.detail.contains(&client.to_string()) && fault.detail.contains(&resident.to_string()),
        "the fault names both epochs: {}",
        fault.detail
    );
}

#[test]
fn command_witnesses_match_the_specified_document() {
    let command: Command<DoneBody> = round_trip(COMMAND);

    assert_eq!(command.kind.as_str(), "task::done");
    assert_eq!(command.expected.len(), 5);
    assert_eq!(command.body.validation, "cargo test --workspace");

    let residency = command
        .expected
        .iter()
        .find(|witness| matches!(witness, Witness::Residency { .. }))
        .expect("the residency witness survives the round trip");
    let Witness::Residency { lane, epoch, .. } = residency else {
        unreachable!("matched above");
    };
    assert_eq!(*lane, uuid("018f82d4-6f59-7d06-92e3-bce9131163c2"));
    assert_eq!(*epoch, uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"));
}

#[test]
fn witness_order_does_not_change_command_identity() {
    let forward: Command<DoneBody> = wire::decode(COMMAND.as_bytes()).expect("decodes");
    let mut reversed = forward.clone();
    reversed.expected.reverse();

    assert_ne!(
        forward.expected, reversed.expected,
        "the fixture must actually be reordered for this to prove anything"
    );
    assert_eq!(
        forward.canonical_json().expect("canonical"),
        reversed.canonical_json().expect("canonical"),
        "witness order carries no meaning, so canonical encoding sorts it away"
    );
}

#[test]
fn reply_matches_the_specified_document() {
    let response: Response<DoneResult> = round_trip(REPLY);

    let Outcome::Reply(reply) = &response.outcome else {
        panic!("the fixture carries a reply outcome");
    };
    let ReplyPosition::Committed(range) = reply.position else {
        panic!("a mutating command commits a range");
    };
    assert_eq!(range.first, seq(48213));
    assert_eq!(range.last, seq(48214));
    assert!(range.is_ordered());
    assert_eq!(reply.body.status, "done");
}

#[test]
fn a_read_only_reply_carries_a_snapshot_cursor_instead_of_a_range() {
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(48213)),
            body: DoneResult {
                task: "SPICE-aBcD".parse().expect("handle"),
                status: "done".to_owned(),
            },
        }),
    };

    let encoded = String::from_utf8(wire::encode(&response).expect("encodes")).expect("utf8");
    assert!(
        encoded.contains("\"cursor\":48213") && !encoded.contains("\"seq\""),
        "a snapshot answer names cursor, never a committed range: {encoded}"
    );

    let decoded: Response<DoneResult> = wire::decode(encoded.as_bytes()).expect("decodes");
    assert_eq!(decoded, response);
}

#[test]
fn a_refusal_round_trips() {
    let response: Response<DoneResult> = Response {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        outcome: Outcome::Refusal(Refusal {
            reason: "claim::not-held".to_owned(),
            witness: Some("claim".to_owned()),
            expected: Some("48172".to_owned()),
            observed: Some("48180".to_owned()),
            remedy: "reclaim the task before completing its phase".to_owned(),
        }),
    };

    let encoded = wire::encode(&response).expect("encodes");
    let decoded: Response<DoneResult> = wire::decode(&encoded).expect("decodes");

    assert_eq!(decoded, response);
    let Outcome::Refusal(refusal) = &decoded.outcome else {
        panic!("a refusal decodes as a refusal");
    };
    assert_eq!(refusal.witness.as_deref(), Some("claim"));
}

#[test]
fn a_fault_round_trips() {
    let response: Response<DoneResult> = Response {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        outcome: Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "command::id-collision".to_owned(),
            detail: "the id was bound to different canonical bytes".to_owned(),
        }),
    };

    let encoded = wire::encode(&response).expect("encodes");
    let decoded: Response<DoneResult> = wire::decode(&encoded).expect("decodes");

    assert_eq!(decoded, response);
}

#[test]
fn packed_names_its_overflow_and_horizon() {
    let packed = Packed {
        rows: vec![Row {
            task: "SPICE-aBcD".parse().expect("handle"),
        }],
        overflow: Some(Overflow {
            omitted: 41,
            continuation: seq(48200),
        }),
        horizon: seq(48213),
    };

    let encoded = wire::encode(&packed).expect("encodes");
    let decoded: Packed<Row> = wire::decode(&encoded).expect("decodes");

    assert_eq!(decoded, packed);
    assert!(decoded.is_overflowing());
    assert_eq!(decoded.overflow.expect("overflow").omitted, 41);
    assert_eq!(decoded.horizon, seq(48213));
}

#[test]
fn a_complete_packed_answer_omits_the_overflow_marker() {
    let packed: Packed<Row> = Packed {
        rows: Vec::new(),
        overflow: None,
        horizon: seq(48213),
    };

    let encoded = String::from_utf8(wire::encode(&packed).expect("encodes")).expect("utf8");
    assert!(
        !encoded.contains("overflow"),
        "no overflow marker when nothing was omitted: {encoded}"
    );

    let decoded: Packed<Row> = wire::decode(encoded.as_bytes()).expect("decodes");
    assert_eq!(decoded, packed);
    assert!(!decoded.is_overflowing());
}

#[test]
fn a_fold_backed_answer_wears_its_provenance() {
    let derived = wire::Derived {
        fold: "steer::inbox".parse().expect("fold name"),
        r#gen: 7,
        cursor: seq(48213),
        packed: Packed::<Row> {
            rows: Vec::new(),
            overflow: None,
            horizon: seq(48213),
        },
    };

    let encoded = String::from_utf8(wire::encode(&derived).expect("encodes")).expect("utf8");
    assert!(
        encoded.contains("\"gen\":7"),
        "the generation is spelled `gen` on the wire: {encoded}"
    );

    let decoded: wire::Derived<Row> = wire::decode(encoded.as_bytes()).expect("decodes");
    assert_eq!(decoded, derived);
}

#[test]
fn a_reversed_committed_range_does_not_decode() {
    let reversed = REPLY.replace("\"first\": 48213", "\"first\": 48215");

    assert!(
        wire::decode::<Response<DoneResult>>(reversed.as_bytes()).is_err(),
        "an inclusive range cannot end before it begins"
    );
    let built = SeqRange {
        first: seq(48214),
        last: seq(48213),
    };
    assert!(
        !built.is_ordered(),
        "the same check is available before a range is sent"
    );
}
