//! Literal fixtures for WIRE.md's nine authoritative head rows.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::{
    ClaimHead, Command, CursorHead, FindingHead, LeaseHead, ResidencyHead, TaskHead, TrustHead,
    WitnessClass, WitnessHead, WitnessKind, WorkflowHead,
};

const TASK: &str = r#"{
  "handle": "WIRE-aBcD",
  "uuid": "018f82d4-6f59-7d06-92e3-bce913116301",
  "title": "Bind authority heads",
  "project": "task.wire",
  "tags": ["genesis", "wire"],
  "dependencies": ["WIRE-ZyX9"],
  "provenance": "task:WIRE-ZyX9",
  "revision": 41
}"#;

const WORKFLOW: &str = r#"{
  "flow": ["todo", "review"],
  "phase": "todo",
  "status": "pending",
  "review": "unassigned",
  "assignment": "lane:018f82d4-6f59-7d06-92e3-bce913116302",
  "deferral": "2030-04-12T18:44:09.271Z",
  "completion": null
}"#;

const CLAIM: &str = r#"{
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce913116302",
  "fact": 42,
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
  "name": "review:WIRE-aBcD",
  "kind": "review",
  "predicate": "workflow head 41 is review",
  "head": 43
}"#;

const COMMAND: &str = r#"{
  "v": 1,
  "id": "018f82d4-6f59-7d06-92e3-bce913116304",
  "kind": "task::note",
  "expected": [{ "kind": "existence", "task": "WIRE-aBcD" }],
  "body": {}
}"#;

const TRUST: &str = r#"{
  "digest": "sha256:8fc01e",
  "state": "pended"
}"#;

const FINDING: &str = r#"{
  "identity": "gate:file-shape:wire/src/head.rs",
  "severity": "error",
  "locus": "wire/src/head.rs:1",
  "owner": "task.wire",
  "receipt": "sha256:19cba4",
  "state": "open"
}"#;

const RESIDENCY: &str = r#"{
  "epoch": "018f82d4-6f59-7d06-92e3-bce913116304",
  "state": "seated",
  "lanes": [{
    "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
    "occupant": "018f82d4-6f59-7d06-92e3-bce913116303",
    "lineage": ["018f82d4-6f59-7d06-92e3-bce913116305"],
    "capabilities": ["018f82d4-6f59-7d06-92e3-bce913116306"],
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
            "dependencies",
            "handle",
            "project",
            "provenance",
            "revision",
            "tags",
            "title",
            "uuid",
        ],
    );
    assert_fields::<WorkflowHead>(
        WORKFLOW,
        &[
            "assignment",
            "completion",
            "deferral",
            "flow",
            "phase",
            "review",
            "status",
        ],
    );
    assert_fields::<ClaimHead>(
        CLAIM,
        &["actor", "disposition", "epoch", "fact", "lane", "occupant"],
    );
    assert_fields::<LeaseHead>(LEASE, &["epoch", "expiry", "maximum", "renewal", "start"]);
    assert_fields::<WitnessHead>(WITNESS, &["head", "kind", "name", "predicate"]);
    assert_fields::<TrustHead>(TRUST, &["digest", "state"]);
    assert_fields::<FindingHead>(
        FINDING,
        &["identity", "locus", "owner", "receipt", "severity", "state"],
    );
    assert_fields::<ResidencyHead>(RESIDENCY, &["epoch", "lanes", "state"]);
    assert_fields::<CursorHead>(CURSOR, &["seq", "wake"]);

    let residency: serde_json::Value =
        serde_json::from_str(RESIDENCY).expect("residency fixture is JSON");
    assert_value_fields(
        &residency["lanes"][0],
        &["capabilities", "disposition", "lane", "lineage", "occupant"],
    );
}

#[test]
fn witness_heads_admit_each_settled_predicate_class() {
    for kind in [
        "assignment",
        "review",
        "directive",
        "consent",
        "budget",
        "document",
        "transition",
    ] {
        let fixture = WITNESS.replace("\"kind\": \"review\"", &format!("\"kind\": \"{kind}\""));
        let decoded = wire::decode::<WitnessHead>(fixture.as_bytes());
        assert!(decoded.is_ok(), "{kind} is an authoritative witness class");
    }
}

#[test]
fn both_witness_vocabularies_coexist_without_shadowing() {
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
