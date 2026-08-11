use wire::{CanonicalUuid, CanvasBootstrap, CanvasVersion};

const CANVAS_BOOTSTRAP: &str = r#"{
  "endpoint": "http://127.0.0.1:48123",
  "canvas": 1,
  "capability": "018f82d4-6f59-7d06-92e3-bce9131163c6",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "speech": {
    "backend": "system",
    "voice": "Zoe (Premium)",
    "words_per_minute": 175
  }
}"#;

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture UUID is canonical")
}

fn round_trip(document: &str) -> CanvasBootstrap {
    let decoded: CanvasBootstrap = wire::decode(document.as_bytes()).expect("bootstrap decodes");
    let reencoded: serde_json::Value =
        serde_json::from_slice(&wire::encode(&decoded).expect("bootstrap encodes"))
            .expect("re-encoded bootstrap is JSON");
    let original: serde_json::Value =
        serde_json::from_str(document).expect("bootstrap fixture is JSON");
    assert_eq!(reencoded, original, "round trip changed the bootstrap");
    decoded
}

#[test]
fn canvas_bootstrap_has_one_strict_current_generation_shape() {
    let bootstrap = round_trip(CANVAS_BOOTSTRAP);

    assert_eq!(bootstrap.endpoint, "http://127.0.0.1:48123");
    assert_eq!(bootstrap.canvas, CanvasVersion::CURRENT);
    assert_eq!(
        bootstrap.capability,
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c6")
    );
    assert_eq!(
        bootstrap.epoch,
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c3")
    );
    assert_eq!(
        bootstrap.speech,
        wire::SpeechPlayback::System {
            voice: "Zoe (Premium)".to_owned(),
            words_per_minute: 175,
        }
    );

    for field in ["endpoint", "canvas", "capability", "epoch", "speech"] {
        let mut value: serde_json::Value =
            serde_json::from_str(CANVAS_BOOTSTRAP).expect("bootstrap fixture is JSON");
        value
            .as_object_mut()
            .expect("bootstrap is an object")
            .remove(field);
        assert!(
            wire::decode::<CanvasBootstrap>(&serde_json::to_vec(&value).expect("shape encodes"))
                .is_err(),
            "bootstrap without {field} was admitted"
        );
    }

    for malformed in [
        r#"{"backend":"unknown"}"#,
        r#"{"backend":"disabled","voice":"Zoe"}"#,
        r#"{"backend":"system","words_per_minute":175}"#,
        r#"{"backend":"external","content_type":"audio/wav","words_per_minute":175}"#,
    ] {
        assert!(
            wire::decode::<wire::SpeechPlayback>(malformed.as_bytes()).is_err(),
            "malformed speech playback was admitted: {malformed}"
        );
    }
}
