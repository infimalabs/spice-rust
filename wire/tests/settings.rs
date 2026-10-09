//! Global settings carry fast mode only; operator identity is Git's.
use wire::intent::GlobalSettingsSet;
use wire::{AnyFact, body};

/// A body recorded while settings still accepted identity overrides.
const OVERRIDDEN: &str = r#"{
  "kind": "settings::changed",
  "fast_mode": false,
  "operator_name": "C Adams",
  "operator_email": "c@example.invalid",
  "operator": { "name": "C Adams", "email": "c@example.invalid", "initials": "CA" }
}"#;

/// A body recorded with only the effective identity snapshot.
const SNAPSHOT: &str = r#"{
  "kind": "settings::changed",
  "fast_mode": true,
  "operator": { "name": "C Adams", "initials": "CA" }
}"#;

#[test]
fn recorded_identity_bodies_still_decode_and_round_trip() {
    for (recorded, fast_mode) in [(OVERRIDDEN, false), (SNAPSHOT, true)] {
        let fact = wire::decode::<AnyFact>(recorded.as_bytes()).expect("recorded body decodes");
        let settings: body::GlobalSettingsChanged = fact.read().expect("settings body");
        assert_eq!(settings.fast_mode, fast_mode);
        let original: serde_json::Value = serde_json::from_str(recorded).expect("fixture is JSON");
        let reencoded: serde_json::Value =
            serde_json::from_slice(&wire::encode(&fact).expect("fact encodes")).expect("JSON");
        assert_eq!(
            reencoded, original,
            "a verifier re-encodes the exact fields"
        );
    }
}

#[test]
fn a_settings_change_records_and_admits_fast_mode_only() {
    assert_eq!(
        serde_json::to_value(body::GlobalSettingsChanged::new(true)).expect("body encodes"),
        serde_json::json!({ "fast_mode": true })
    );
    for field in ["operator_name", "operator_email"] {
        let request = format!(r#"{{ "fast_mode": true, "{field}": "C Adams" }}"#);
        assert!(
            wire::decode::<GlobalSettingsSet>(request.as_bytes()).is_err(),
            "{field}: a settings write must not admit operator identity"
        );
    }
}
