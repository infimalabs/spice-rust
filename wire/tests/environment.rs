//! Environmental requirement shape, canonicality, and compiled semantics.

use serde_json::{Value, json};
use wire::body::TaskAdded;
use wire::{
    AnyFact, Environment, EnvironmentObservation, EnvironmentRequirement, EnvironmentRequirements,
    TaskHead,
};

const LEGACY_ADDED: &str = r#"{
  "kind": "task::added",
  "task": "1kKRRsDP",
  "title": "Build the gate",
  "project": "wire",
  "tags": [],
  "depends": [],
  "flow": ["todo"],
  "phase": "todo",
  "revision": 1
}"#;

const LEGACY_HEAD: &str = r#"{
  "born": "1kKRRsDP",
  "queue_origin": 9,
  "title": "Build the gate",
  "project": "wire",
  "tags": [],
  "dependencies": [],
  "provenance": "repo:genesis",
  "revision": 10
}"#;

#[test]
fn journal_schema_is_one_typed_fact_and_one_typed_value() {
    let requirement = EnvironmentRequirement::journal_schema(15).expect("positive schema");
    let encoded: Value = serde_json::from_slice(&wire::encode(&requirement).expect("encodes"))
        .expect("requirement is JSON");

    assert_eq!(encoded, json!({"fact": "journal-schema", "value": 15}));
    assert_eq!(
        wire::decode::<EnvironmentRequirement>(br#"{"fact":"journal-schema","value":15}"#)
            .expect("canonical requirement"),
        requirement
    );
}

#[test]
fn journal_schema_comparison_is_compiled_as_observed_at_least_required() {
    let requirements = EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(15).expect("positive schema"),
    );
    let below = Environment::journal_schema(14).expect("positive schema");
    let equal = Environment::journal_schema(15).expect("positive schema");
    let above = Environment::journal_schema(16).expect("positive schema");

    assert!(!requirements.are_satisfied_by(&Environment::EMPTY));
    assert!(!requirements.are_satisfied_by(&below));
    assert!(requirements.are_satisfied_by(&equal));
    assert!(requirements.are_satisfied_by(&above));
    assert!(EnvironmentRequirements::default().are_satisfied_by(&Environment::EMPTY));
}

#[test]
fn authored_evaluators_and_non_values_have_no_wire_shape() {
    for malformed in [
        br#"{"fact":"journal-schema","value":0}"#.as_slice(),
        br#"{"fact":"journal-schema","value":-1}"#,
        br#"{"fact":"journal-schema","value":1.0}"#,
        br#"{"fact":"journal-schema","value":4294967296}"#,
        br#"{"fact":"journal-schema","value":"15"}"#,
        br#"{"fact":"journal-schema"}"#,
        br#"{"fact":"journal-schema","value":15,"operator":">="}"#,
        br#"{"fact":"journal-schema","value":15,"evaluator":"shell"}"#,
        br#"{"fact":"journal-schema","value":15,"not":true}"#,
        br#"{"fact":"journal-schema","value":15,"or":[]}"#,
        br#"{"fact":"expression","value":"journal_schema >= required"}"#,
    ] {
        assert!(
            wire::decode::<EnvironmentRequirement>(malformed).is_err(),
            "malformed requirement was admitted: {}",
            String::from_utf8_lossy(malformed)
        );
    }
    assert!(EnvironmentRequirement::journal_schema(0).is_err());
    assert!(EnvironmentObservation::journal_schema(0).is_err());
    assert!(Environment::journal_schema(0).is_err());
}

#[test]
fn requirement_collections_refuse_duplicates_without_normalizing() {
    let duplicate = br#"[
      {"fact":"journal-schema","value":14},
      {"fact":"journal-schema","value":15}
    ]"#;

    assert!(wire::decode::<EnvironmentRequirements>(duplicate).is_err());
    assert!(
        EnvironmentRequirements::new(vec![
            EnvironmentRequirement::journal_schema(14).expect("positive schema"),
            EnvironmentRequirement::journal_schema(15).expect("positive schema"),
        ])
        .is_err()
    );
}

#[test]
fn task_carriers_omit_empty_requirements_and_refuse_an_explicit_empty_array() {
    let fact: AnyFact = wire::decode(LEGACY_ADDED.as_bytes()).expect("retained task fact decodes");
    let added: TaskAdded = fact.read().expect("task-added body");
    assert!(added.requirements.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&wire::encode(&fact).expect("retained fact re-encodes"))
            .expect("fact is JSON"),
        serde_json::from_str::<Value>(LEGACY_ADDED).expect("fixture is JSON")
    );

    let head: TaskHead = wire::decode(LEGACY_HEAD.as_bytes()).expect("retained task head decodes");
    assert!(head.requirements.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&wire::encode(&head).expect("retained head re-encodes"))
            .expect("head is JSON"),
        serde_json::from_str::<Value>(LEGACY_HEAD).expect("fixture is JSON")
    );

    let mut fact = serde_json::from_str::<Value>(LEGACY_ADDED).expect("fixture is JSON");
    fact["requirements"] = json!([]);
    assert!(wire::decode::<AnyFact>(&serde_json::to_vec(&fact).expect("fact encodes")).is_err());

    let mut head = serde_json::from_str::<Value>(LEGACY_HEAD).expect("fixture is JSON");
    head["requirements"] = json!([]);
    assert!(wire::decode::<TaskHead>(&serde_json::to_vec(&head).expect("head encodes")).is_err());
}

#[test]
fn task_fact_and_head_carry_the_same_canonical_requirements() {
    let requirement = json!([{"fact": "journal-schema", "value": 15}]);
    let mut fact = serde_json::from_str::<Value>(LEGACY_ADDED).expect("fixture is JSON");
    fact["requirements"] = requirement.clone();
    let fact_bytes = serde_json::to_vec(&fact).expect("fact encodes");
    let added: TaskAdded = wire::decode::<AnyFact>(&fact_bytes)
        .expect("task fact admits requirements")
        .read()
        .expect("task-added body");

    let mut head = serde_json::from_str::<Value>(LEGACY_HEAD).expect("fixture is JSON");
    head["requirements"] = requirement;
    let task_head: TaskHead =
        wire::decode(&serde_json::to_vec(&head).expect("head encodes")).expect("task head decodes");

    assert_eq!(added.requirements, task_head.requirements);
    assert_eq!(
        added.requirements.as_slice(),
        &[EnvironmentRequirement::journal_schema(15).expect("positive schema")]
    );
}
