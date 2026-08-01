//! Round-trip fixtures for the crossing types.
//!
//! The envelope, hello, command, and reply fixtures are the literal documents
//! printed in WIRE.md. Decoding those bytes and re-encoding them to the same
//! value proves the field names and shapes are the specified ones rather than
//! a paraphrase of them.

use serde::{Deserialize, Serialize};
use wire::manifest::ReleaseTarget;
use wire::{
    Actor, CanonicalUuid, Command, Envelope, Fact, FactKind, Fault, FaultClass, Hello, Instant,
    Origin, Outcome, Overflow, Packed, ProofEvidence, Refusal, Rejected, ReleaseNotes, Reply,
    ReplyPosition, Response, Seq, SeqRange, TaskHandle, Version, WireVersion, Witness, body,
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
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
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

const VERSION: &str = r#"{"binary":"1111111111111111111111111111111111111111111111111111111111111111","wire":1,"journal":1,"app":"2222222222222222222222222222222222222222222222222222222222222222"}"#;

const PROOF_EVIDENCE: &str = concat!(
    r#"{"target":"x86_64-unknown-linux-musl","#,
    r#""payload":"1111111111111111111111111111111111111111111111111111111111111111","#,
    r#""receipt":"2222222222222222222222222222222222222222222222222222222222222222","#,
    r#""battery":"3333333333333333333333333333333333333333333333333333333333333333","#,
    r#""replay":"4444444444444444444444444444444444444444444444444444444444444444"}"#,
);

const DIRECT_PROOF_RESULT: &str = concat!(
    r#"{"kind":"reply","direct":{},"body":{"target":"x86_64-unknown-linux-musl","#,
    r#""payload":"1111111111111111111111111111111111111111111111111111111111111111","#,
    r#""receipt":"2222222222222222222222222222222222222222222222222222222222222222","#,
    r#""battery":"3333333333333333333333333333333333333333333333333333333333333333","#,
    r#""replay":"4444444444444444444444444444444444444444444444444444444444444444"}}"#,
);

const RELEASE_NOTES: &str = concat!(
    r#"{"manifest":"1111111111111111111111111111111111111111111111111111111111111111","#,
    r#""build":"2222222222222222222222222222222222222222222222222222222222222222","#,
    r#""range":{"from":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","to":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","commits":1},"#,
    r#""schema":{"from":1,"to":2},"targets":[{"target":"aarch64-apple-darwin","#,
    r#""payload":"3333333333333333333333333333333333333333333333333333333333333333","#,
    r#""receipt":"4444444444444444444444444444444444444444444444444444444444444444"}],"#,
    r#""operator_actions":["Rotate a token"],"fixed_refusals":["Reject stale seals"],"#,
    r#""changes":[{"commit":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","subject":"Ship notes"}]}"#,
);

#[test]
fn envelope_matches_the_specified_document() {
    let envelope: Envelope<Fact<body::TaskPhased>> = round_trip(ENVELOPE);

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
    assert_eq!(envelope.fact.kind(), FactKind::TaskPhased);
    assert_eq!(envelope.fact.body.phase.as_str(), "verify");
    assert!(!envelope.is_root());
}

#[test]
fn version_is_the_exact_stable_four_identity_document() {
    let version: Version = wire::decode(VERSION.as_bytes()).expect("version document decodes");

    assert_eq!(version.binary.as_str(), "1".repeat(64));
    assert_eq!(version.wire, WireVersion::CURRENT);
    assert_eq!(version.journal, 1);
    assert_eq!(version.app.as_str(), "2".repeat(64));
    assert_eq!(
        wire::encode(&version).expect("version document re-encodes"),
        VERSION.as_bytes(),
        "the stable machine form changed"
    );
}

#[test]
fn version_refuses_missing_null_extra_and_wrong_shaped_identities() {
    let document: serde_json::Value = serde_json::from_str(VERSION).expect("fixture is JSON");

    for field in ["binary", "wire", "journal", "app"] {
        let mut missing = document.clone();
        missing
            .as_object_mut()
            .expect("version is an object")
            .remove(field);
        assert!(
            wire::decode::<Version>(&serde_json::to_vec(&missing).expect("missing shape encodes"))
                .is_err(),
            "missing {field} was admitted"
        );

        let mut null = document.clone();
        null[field] = serde_json::Value::Null;
        assert!(
            wire::decode::<Version>(&serde_json::to_vec(&null).expect("null shape encodes"))
                .is_err(),
            "null {field} was admitted"
        );
    }

    let mut extra = document.clone();
    extra
        .as_object_mut()
        .expect("version is an object")
        .insert("epoch".to_owned(), serde_json::json!(1));
    assert!(
        wire::decode::<Version>(&serde_json::to_vec(&extra).expect("extra shape encodes")).is_err(),
        "an unknown version identity was admitted"
    );

    for (field, wrong) in [
        ("binary", serde_json::json!(7)),
        ("wire", serde_json::json!("1")),
        ("journal", serde_json::json!("1")),
        ("app", serde_json::json!("not-a-digest")),
    ] {
        let mut malformed = document.clone();
        malformed[field] = wrong;
        assert!(
            wire::decode::<Version>(
                &serde_json::to_vec(&malformed).expect("malformed shape encodes")
            )
            .is_err(),
            "wrong-shaped {field} was admitted"
        );
    }
}

#[test]
fn proof_evidence_is_the_exact_stable_five_identity_document() {
    let evidence: ProofEvidence =
        wire::decode(PROOF_EVIDENCE.as_bytes()).expect("proof evidence decodes");

    assert_eq!(evidence.target, ReleaseTarget::X86_64UnknownLinuxMusl);
    assert_eq!(evidence.payload.as_str(), "1".repeat(64));
    assert_eq!(evidence.receipt.as_str(), "2".repeat(64));
    assert_eq!(evidence.battery.as_str(), "3".repeat(64));
    assert_eq!(evidence.replay.as_str(), "4".repeat(64));
    assert_eq!(
        wire::encode(&evidence).expect("proof evidence re-encodes"),
        PROOF_EVIDENCE.as_bytes(),
        "the stable proof evidence form changed"
    );
}

#[test]
fn release_notes_round_trip_and_refuse_unknown_fields_at_every_level() {
    let notes: ReleaseNotes = round_trip(RELEASE_NOTES);
    assert_eq!(notes.range.commits, 1);
    assert_eq!(notes.schema.from, 1);
    assert_eq!(notes.schema.to, 2);
    assert_eq!(notes.targets[0].target, ReleaseTarget::Aarch64AppleDarwin);
    assert_eq!(notes.operator_actions, ["Rotate a token"]);
    assert_eq!(notes.fixed_refusals, ["Reject stale seals"]);
    assert_eq!(notes.changes[0].subject, "Ship notes");

    let document: serde_json::Value = serde_json::from_str(RELEASE_NOTES).expect("fixture is JSON");
    for path in ["root", "range", "schema", "target", "change"] {
        let mut extra = document.clone();
        let object = match path {
            "root" => extra.as_object_mut().expect("notes object"),
            "range" => extra["range"].as_object_mut().expect("range object"),
            "schema" => extra["schema"].as_object_mut().expect("schema object"),
            "target" => extra["targets"][0].as_object_mut().expect("target object"),
            "change" => extra["changes"][0].as_object_mut().expect("change object"),
            _ => unreachable!(),
        };
        object.insert("unknown".to_owned(), serde_json::json!(true));
        assert!(
            wire::decode::<ReleaseNotes>(&wire::encode(&extra).expect("extra shape encodes"))
                .is_err(),
            "unknown field at {path} was admitted"
        );
    }
}

#[test]
fn proof_evidence_refuses_missing_null_extra_and_wrong_shaped_identities() {
    let document: serde_json::Value =
        serde_json::from_str(PROOF_EVIDENCE).expect("fixture is JSON");

    for field in ["target", "payload", "receipt", "battery", "replay"] {
        let mut missing = document.clone();
        missing
            .as_object_mut()
            .expect("proof evidence is an object")
            .remove(field);
        assert!(
            wire::decode::<ProofEvidence>(
                &serde_json::to_vec(&missing).expect("missing shape encodes")
            )
            .is_err(),
            "missing {field} was admitted"
        );

        let mut null = document.clone();
        null[field] = serde_json::Value::Null;
        assert!(
            wire::decode::<ProofEvidence>(&serde_json::to_vec(&null).expect("null shape encodes"))
                .is_err(),
            "null {field} was admitted"
        );
    }

    let mut extra = document.clone();
    extra
        .as_object_mut()
        .expect("proof evidence is an object")
        .insert("standing".to_owned(), serde_json::json!("clean"));
    assert!(
        wire::decode::<ProofEvidence>(&serde_json::to_vec(&extra).expect("extra shape encodes"))
            .is_err(),
        "a second standing signal was admitted"
    );

    for (field, wrong) in [
        ("target", serde_json::json!("unknown-target")),
        ("payload", serde_json::json!(7)),
        ("receipt", serde_json::json!("not-a-digest")),
        ("battery", serde_json::json!("3".repeat(63))),
        ("replay", serde_json::json!(null)),
    ] {
        let mut malformed = document.clone();
        malformed[field] = wrong;
        assert!(
            wire::decode::<ProofEvidence>(
                &serde_json::to_vec(&malformed).expect("malformed shape encodes")
            )
            .is_err(),
            "wrong-shaped {field} was admitted"
        );
    }
}

#[test]
fn proof_evidence_is_a_direct_result_without_a_fabricated_sequence() {
    let result: Outcome<ProofEvidence> =
        wire::decode(DIRECT_PROOF_RESULT.as_bytes()).expect("direct proof result decodes");

    let Outcome::Reply(reply) = &result else {
        panic!("the fixture is a successful reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body.target, ReleaseTarget::X86_64UnknownLinuxMusl);
    assert_eq!(
        wire::encode(&result).expect("direct proof result re-encodes"),
        DIRECT_PROOF_RESULT.as_bytes(),
        "the shared direct proof result changed"
    );
    assert!(!DIRECT_PROOF_RESULT.contains("\"seq\""));
    assert!(!DIRECT_PROOF_RESULT.contains("\"cursor\""));
    assert!(!DIRECT_PROOF_RESULT.contains("\"standing\""));
    assert!(!DIRECT_PROOF_RESULT.contains("\"success\""));
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

    let decoded: Envelope<Fact<body::TaskPhased>> =
        wire::decode(encoded.as_bytes()).expect("decodes");
    assert_eq!(decoded, root);
    assert!(decoded.is_root());
}

#[test]
fn a_zero_or_null_cause_sentinel_is_refused() {
    let zero = ENVELOPE.replace("\"cause\": 48190", "\"cause\": 0");
    let null = ENVELOPE.replace("\"cause\": 48190", "\"cause\": null");

    assert!(
        wire::decode::<Envelope<Fact<body::TaskPhased>>>(zero.as_bytes()).is_err(),
        "zero is not a position"
    );
    assert!(
        wire::decode::<Envelope<Fact<body::TaskPhased>>>(null.as_bytes()).is_err(),
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
fn a_daemonless_reply_states_that_its_journal_position_is_directly_absent() {
    let outcome = Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: DoneResult {
            task: "SPICE-aBcD".parse().expect("handle"),
            status: "checked".to_owned(),
        },
    });

    let encoded = wire::encode(&outcome).expect("direct reply encodes");
    let value: serde_json::Value = serde_json::from_slice(&encoded).expect("wire bytes are json");
    assert_eq!(value["direct"], serde_json::json!({}));
    assert!(value.get("seq").is_none() && value.get("cursor").is_none());

    let decoded: Outcome<DoneResult> = wire::decode(&encoded).expect("direct reply decodes");
    assert_eq!(decoded, outcome);
}

#[test]
fn direct_absence_has_no_null_unknown_or_sequence_shaped_alias() {
    for (name, direct) in [
        ("null", serde_json::Value::Null),
        ("unknown", serde_json::json!({"unknown": true})),
        (
            "committed range",
            serde_json::json!({"first": 1, "last": 1}),
        ),
        ("snapshot cursor", serde_json::json!(1)),
    ] {
        let alternate = serde_json::json!({
            "kind": "reply",
            "direct": direct,
            "body": {"task": "SPICE-aBcD", "status": "checked"}
        });
        let bytes = serde_json::to_vec(&alternate).expect("alternate is json");

        assert!(
            wire::decode::<Outcome<DoneResult>>(&bytes).is_err(),
            "the {name} spelling must not alias direct absence",
        );
    }
}

#[test]
fn one_reply_cannot_carry_two_position_keys() {
    for mixed in [
        serde_json::json!({"direct": {}, "cursor": 1}),
        serde_json::json!({"direct": {}, "seq": {"first": 1, "last": 1}}),
        serde_json::json!({"cursor": 1, "seq": {"first": 1, "last": 1}}),
    ] {
        let mut outcome = mixed;
        let object = outcome
            .as_object_mut()
            .expect("position fixture is an object");
        object.insert("kind".to_owned(), serde_json::json!("reply"));
        object.insert(
            "body".to_owned(),
            serde_json::json!({"task": "SPICE-aBcD", "status": "checked"}),
        );
        let bytes = serde_json::to_vec(&outcome).expect("mixed fixture is json");

        assert!(
            wire::decode::<Outcome<DoneResult>>(&bytes).is_err(),
            "two position keys must not decode by silently choosing one: {outcome}",
        );
    }
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
