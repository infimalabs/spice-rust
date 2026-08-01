//! Literal documents proving the one spelling for absent optional fields.

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::{CursorHead, LaneResidency, Packed, Refusal, WorkflowHead};

const WORKFLOW: &str = r#"{
  "flow": ["todo", "review"],
  "phase": "todo",
  "status": "pending",
  "review": "unassigned"
}"#;

const LANE: &str = r#"{
  "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
  "lineage": [],
  "capabilities": [],
  "disposition": "vacant"
}"#;

const CURSOR: &str = r#"{}"#;

const PACKED: &str = r#"{
  "rows": [],
  "horizon": 48213
}"#;

const REFUSAL: &str = r#"{
  "reason": "claim::not-held",
  "remedy": "reclaim the task"
}"#;

#[test]
fn absent_head_fields_are_omitted_and_null_is_refused() {
    assert_canonical_absence::<WorkflowHead>(WORKFLOW, &["assignment", "deferral", "completion"]);
    assert_canonical_absence::<LaneResidency>(LANE, &["occupant"]);
    assert_canonical_absence::<CursorHead>(CURSOR, &["seq", "wake"]);
}

#[test]
fn absent_non_head_fields_are_omitted_and_null_is_refused() {
    assert_canonical_absence::<Packed<serde_json::Value>>(PACKED, &["overflow"]);
    assert_canonical_absence::<Refusal>(REFUSAL, &["witness", "expected", "observed"]);
}

fn assert_canonical_absence<T>(document: &str, absent_fields: &[&str])
where
    T: DeserializeOwned + Serialize,
{
    let decoded: T = wire::decode(document.as_bytes()).expect("omitted fields decode");
    let original: serde_json::Value = serde_json::from_str(document).expect("fixture is JSON");
    let encoded: serde_json::Value =
        serde_json::from_slice(&wire::encode(&decoded).expect("fixture encodes"))
            .expect("encoded fixture is JSON");
    assert_eq!(encoded, original, "absence did not remain omitted");

    for field in absent_fields {
        let mut alternate = original.clone();
        alternate
            .as_object_mut()
            .expect("fixture is an object")
            .insert((*field).to_owned(), serde_json::Value::Null);
        let bytes = serde_json::to_vec(&alternate).expect("alternate encodes as JSON");
        assert!(
            wire::decode::<T>(&bytes).is_err(),
            "{field}: null admitted a second spelling for absence"
        );
    }
}
