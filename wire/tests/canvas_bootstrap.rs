use wire::{CanonicalUuid, CanvasBootstrap, CanvasVersion};

const CANVAS_BOOTSTRAP: &str = r#"{
  "endpoint": "http://127.0.0.1:48123",
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
fn canvas_bootstrap_has_one_strict_body_shape() {
    let bootstrap = round_trip(CANVAS_BOOTSTRAP);

    assert_eq!(bootstrap.endpoint, "http://127.0.0.1:48123");
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

    for field in ["endpoint", "capability", "epoch", "speech"] {
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

#[test]
fn canvas_version_has_one_http_header_spelling() {
    assert_eq!(wire::hello::CANVAS_VERSION_HEADER, "x-spice-canvas-version");
    assert_eq!(
        CanvasVersion::CURRENT.to_string().parse(),
        Ok(CanvasVersion::CURRENT)
    );
    for malformed in ["", "current", "-1", "1.0"] {
        assert!(
            malformed.parse::<CanvasVersion>().is_err(),
            "malformed Canvas version was admitted: {malformed:?}"
        );
    }
}

#[test]
fn strict_bootstrap_body_preserves_the_pre_version_shape() {
    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PriorCanvasBootstrap {
        endpoint: String,
        capability: CanonicalUuid,
        epoch: CanonicalUuid,
        speech: wire::SpeechPlayback,
    }

    let current: CanvasBootstrap =
        wire::decode(CANVAS_BOOTSTRAP.as_bytes()).expect("current bootstrap decodes");
    let bytes = wire::encode(&current).expect("current bootstrap encodes");
    let prior: PriorCanvasBootstrap =
        wire::decode(&bytes).expect("the prior strict bootstrap shape reads the current body");
    assert_eq!(prior.endpoint, current.endpoint);
    assert_eq!(prior.capability, current.capability);
    assert_eq!(prior.epoch, current.epoch);
    assert_eq!(prior.speech, current.speech);

    let mut five_field: serde_json::Value =
        serde_json::from_slice(&bytes).expect("current body is JSON");
    five_field
        .as_object_mut()
        .expect("bootstrap is an object")
        .insert("canvas".to_owned(), serde_json::json!(1));
    let five_field = serde_json::to_vec(&five_field).expect("five-field fixture encodes");
    let error = wire::decode::<PriorCanvasBootstrap>(&five_field)
        .expect_err("the pre-version strict decoder reproduces the screenshot");
    assert!(
        error.to_string().contains("unknown field `canvas`"),
        "{error}"
    );
    assert!(
        wire::decode::<CanvasBootstrap>(&five_field).is_err(),
        "the current strict body must not retain an alternate five-field path"
    );
}
