//! Literal fixtures for the nine authoritative head rows.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::{
    ClaimHead, Command, CursorHead, FindingHead, LeaseHead, ResidencyHead, TaskHead, TrustHead,
    WitnessClass, WitnessHead, WitnessKind, WorkflowHead,
};

const TASK: &str = r#"{
  "born": "1kKRRsDP",
  "queue_origin": 40,
  "title": "Bind authority heads",
  "description": "Keep complete task context on the current statement",
  "acceptance": ["Every authored field survives the head round trip"],
  "project": "wire",
  "dependencies": ["1kKRRsDQ"],
  "provenance": "task:1kKRRsDQ",
  "creation": { "kind": "unavailable", "reason": "operator-actor" },
  "revision": 41
}"#;

const WORKFLOW: &str = r#"{
  "flow": ["todo", "review"],
  "phase": "todo",
  "status": "pending",
  "review": "assigned",
  "assignment": "lane:018f82d4-6f59-7d06-92e3-bce913116302",
  "deferral": "2030-04-12T18:44:09.271Z"
}"#;

const CLAIM: &str = r#"{
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce913116302",
  "fact": 42,
  "claimed": 42,
  "cell": {
    "project": "wire",
    "phase": "todo"
  },
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "occupant": "018f82d4-6f59-7d06-92e3-bce913116303",
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116304",
  "disposition": "active"
}"#;

const LEASE: &str = r#"{
  "start": "2030-04-12T18:44:09.271Z",
  "renewal": "2030-04-12T18:49:09.271Z",
  "maximum": "2030-04-12T19:44:09.271Z",
  "expiry": "2030-04-12T18:59:09.271Z",
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116304"
}"#;

const WITNESS: &str = r#"{
  "name": "review:WIRE-1kKRRsDP",
  "kind": "review",
  "predicate": "workflow head 41 is review",
  "head": 43
}"#;

const COMMAND: &str = r#"{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce913116304",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "kind": "task::note",
  "expected": [{ "kind": "existence", "task": "1kKRRsDP" }],
  "body": {}
}"#;

const TRUST: &str = r#"{
  "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "state": "pended"
}"#;

const FINDING: &str = r#"{
  "identity": "gate:file-shape:wire/src/head.rs",
  "severity": "error",
  "locus": "wire/src/head.rs:1",
  "owner": "wire",
  "receipt": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
  "state": "open"
}"#;

const RESIDENCY: &str = r#"{
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116304",
  "state": "seated",
  "topology_revision": 9,
  "subscriptions": [{
    "team": "018f82d4-6f59-7d06-92e3-bce913116302",
    "policy": "drive"
  }],
  "lanes": [{
    "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
    "team": "018f82d4-6f59-7d06-92e3-bce913116302",
    "worktree": "/worktrees/lane-a",
    "life": "drive",
    "vendor": "omp",
    "occupant": "018f82d4-6f59-7d06-92e3-bce913116303",
    "lineage": ["018f82d4-6f59-7d06-92e3-bce913116305"],
    "capabilities": ["018f82d4-6f59-7d06-92e3-bce913116306"],
    "session": {
      "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
      "occupant": "018f82d4-6f59-7d06-92e3-bce913116303",
      "worktree": "/worktrees/lane-a",
      "vendor": "omp",
      "model": "sonnet",
      "effort": "high",
      "fast_mode": true,
      "git": "current",
      "rtk": {
        "state": "active",
        "version": "0.45.0",
        "detail": "rewrite protocol and answer fidelity passed"
      },
      "epoch": "018f82d4-6f59-7d06-92e3-bce913116304",
      "stream": "stdout",
      "identity": "018f7777-7777-7444-8888-999999999999",
      "locus": { "member": 7, "offset": 31, "length": 93 }
    },
    "disposition": "active"
  }]
}"#;

const CURSOR: &str = r#"{
  "seq": 53,
  "wake": 51
}"#;

#[test]
fn every_authority_head_round_trips_the_wire_table_fields() {
    assert_fields::<TaskHead>(
        TASK,
        &[
            "acceptance",
            "born",
            "creation",
            "dependencies",
            "description",
            "project",
            "provenance",
            "queue_origin",
            "revision",
            "title",
        ],
    );
    assert_fields::<WorkflowHead>(
        WORKFLOW,
        &[
            "assignment",
            "deferral",
            "flow",
            "phase",
            "review",
            "status",
        ],
    );
    assert_fields::<ClaimHead>(
        CLAIM,
        &[
            "actor",
            "cell",
            "claimed",
            "disposition",
            "epoch",
            "fact",
            "lane",
            "occupant",
        ],
    );
    assert_fields::<LeaseHead>(LEASE, &["epoch", "expiry", "maximum", "renewal", "start"]);
    assert_fields::<WitnessHead>(WITNESS, &["head", "kind", "name", "predicate"]);
    assert_fields::<TrustHead>(TRUST, &["digest", "state"]);
    assert_fields::<FindingHead>(
        FINDING,
        &["identity", "locus", "owner", "receipt", "severity", "state"],
    );
    assert_fields::<ResidencyHead>(
        RESIDENCY,
        &[
            "epoch",
            "lanes",
            "state",
            "subscriptions",
            "topology_revision",
        ],
    );
    assert_fields::<CursorHead>(CURSOR, &["seq", "wake"]);

    let residency: serde_json::Value =
        serde_json::from_str(RESIDENCY).expect("residency fixture is JSON");
    assert_value_fields(
        &residency["lanes"][0],
        &[
            "capabilities",
            "disposition",
            "lane",
            "life",
            "lineage",
            "occupant",
            "session",
            "team",
            "vendor",
            "worktree",
        ],
    );
    assert_value_fields(&residency["subscriptions"][0], &["policy", "team"]);
}

#[test]
fn a_residency_head_requires_its_explicit_subscription_table() {
    let mut omitted: serde_json::Value =
        serde_json::from_str(RESIDENCY).expect("residency fixture is JSON");
    let head = omitted
        .as_object_mut()
        .expect("residency fixture is an object");
    head.remove("subscriptions");
    let error = wire::decode::<ResidencyHead>(&serde_json::to_vec(&omitted).expect("encodes"))
        .expect_err("an omitted subscription table refuses");
    assert!(error.to_string().contains("subscriptions"), "{error}");

    let mut empty: ResidencyHead = wire::decode(RESIDENCY.as_bytes()).expect("residency decodes");
    empty.lanes.clear();
    empty.subscriptions.clear();
    let encoded = serde_json::to_value(&empty).expect("empty residency encodes");
    assert_eq!(encoded["subscriptions"], serde_json::json!([]));
}

#[test]
fn witness_heads_admit_each_settled_predicate_class() {
    for class in [
        "assignment",
        "review",
        "directive",
        "consent",
        "budget",
        "document",
        "transition",
    ] {
        let fixture = WITNESS.replace("\"kind\": \"review\"", &format!("\"kind\": \"{class}\""));
        let decoded = wire::decode::<WitnessHead>(fixture.as_bytes());
        assert!(decoded.is_ok(), "{class} is an authoritative witness class");
    }
}

#[test]
fn both_witness_vocabularies_decode_without_shadowing() {
    let head: WitnessHead = wire::decode(WITNESS.as_bytes()).expect("witness head decodes");
    let command: Command<serde_json::Value> =
        wire::decode(COMMAND.as_bytes()).expect("command decodes");

    assert_eq!(head.kind, WitnessClass::Review);
    assert_eq!(command.expected[0].kind(), WitnessKind::Existence);
}

#[test]
fn lease_instants_refuse_non_utc_spellings() {
    let offset = LEASE.replace("2030-04-12T18:44:09.271Z", "2030-04-12T18:44:09.271+00:00");

    assert!(wire::decode::<LeaseHead>(offset.as_bytes()).is_err());
}

#[test]
fn trust_head_refuses_a_noncanonical_digest() {
    let malformed = TRUST.replace(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sha256:8fc01e",
    );

    assert!(wire::decode::<TrustHead>(malformed.as_bytes()).is_err());
}

#[test]
fn finding_head_refuses_a_noncanonical_receipt_digest() {
    let malformed = FINDING.replace(
        "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "sha256:19cba4",
    );

    assert!(wire::decode::<FindingHead>(malformed.as_bytes()).is_err());
}

fn assert_fields<T>(fixture: &str, fields: &[&str])
where
    T: DeserializeOwned + Serialize,
{
    let expected: serde_json::Value = serde_json::from_str(fixture).expect("fixture is JSON");
    let decoded: T = wire::decode(fixture.as_bytes()).expect("head fixture decodes");
    let encoded: serde_json::Value =
        serde_json::from_slice(&wire::encode(&decoded).expect("head fixture encodes"))
            .expect("encoded head is JSON");
    assert_value_fields(&expected, fields);

    assert_eq!(encoded, expected);
}

fn assert_value_fields(value: &serde_json::Value, fields: &[&str]) {
    let mut actual_fields = value
        .as_object()
        .expect("head fixture is an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    actual_fields.sort_unstable();

    assert_eq!(actual_fields, fields);
}

/// Both witness vocabularies name themselves in one scope without shadowing.
///
/// This test exists because they once collided: two lanes each exported a
/// `WitnessKind` from the crate root and `wire` stopped compiling. Importing
/// both unqualified is the cheapest standing proof the names stayed distinct.
/// `WitnessClass` and `WitnessKind` now differ by one word, so the reason for
/// two types has to be asserted rather than assumed — the day either becomes a
/// subset of the other is the day to merge them, and this fails then.
#[test]
fn both_witness_vocabularies_stay_distinct_unqualified() {
    use wire::{WitnessClass, WitnessKind};

    let head: WitnessHead = wire::decode(WITNESS.as_bytes()).expect("witness head decodes");
    assert_eq!(head.kind, WitnessClass::Review);

    // Neither vocabulary contains the other, proved through the one decoder
    // both share: `review` is a stored head class the command side cannot
    // spell, and `existence` is a command precondition that is no head row.
    assert!(wire::decode::<WitnessClass>(b"\"review\"".as_slice()).is_ok());
    assert!(wire::decode::<WitnessKind>(b"\"review\"".as_slice()).is_err());
    assert!(wire::decode::<WitnessKind>(b"\"existence\"".as_slice()).is_ok());
    assert!(wire::decode::<WitnessClass>(b"\"existence\"".as_slice()).is_err());

    // They do overlap, which is why one type cannot serve both roles: the same
    // five spellings decode on both sides and mean the same predicate.
    for shared in ["assignment", "directive", "consent", "budget", "transition"] {
        let spelling = format!("\"{shared}\"");
        assert!(
            wire::decode::<WitnessClass>(spelling.as_bytes()).is_ok(),
            "{shared} is a head class"
        );
        assert!(
            wire::decode::<WitnessKind>(spelling.as_bytes()).is_ok(),
            "{shared} is a command predicate"
        );
    }
}
