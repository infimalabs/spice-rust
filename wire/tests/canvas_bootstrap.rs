use wire::{CanonicalUuid, CanvasBootstrap, CanvasVersion};

const CANVAS_BOOTSTRAP: &str = r#"{
  "endpoint": "http://127.0.0.1:48123",
  "capability": "018f82d4-6f59-7d06-92e3-bce9131163c6",
  "epoch": "018f82d4-6f59-7d06-92e3-bce9131163c3",
  "version": {
    "product": "1.2.3",
    "binary": "1111111111111111111111111111111111111111111111111111111111111111",
    "wire": 1,
    "journal": 20,
    "canvas": 6,
    "app": "2222222222222222222222222222222222222222222222222222222222222222"
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
    assert_eq!(bootstrap.version.product.as_str(), "1.2.3");
    assert_eq!(bootstrap.version.app.as_str(), "2".repeat(64));

    for field in ["endpoint", "capability", "epoch", "version"] {
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
fn operator_gravatar_crosses_only_as_lowercase_sha256_hex() {
    let hash = "0123456789abcdef".repeat(4);
    let mut value: serde_json::Value =
        serde_json::from_str(CANVAS_BOOTSTRAP).expect("bootstrap fixture is JSON");
    value["operator_gravatar"] = serde_json::Value::String(hash.clone());
    let document = serde_json::to_string(&value).expect("bootstrap encodes");
    let bootstrap = round_trip(&document);
    assert_eq!(
        bootstrap
            .operator_gravatar
            .as_ref()
            .map(wire::GravatarHash::as_str),
        Some(hash.as_str())
    );

    // The hash is spliced into a CSS url(); only bare hex may cross.
    for malformed in [
        serde_json::Value::Null,
        serde_json::Value::String("0123456789ABCDEF".repeat(4)),
        serde_json::Value::String("0123456789abcdef".repeat(4)[1..].to_owned()),
        serde_json::Value::String(format!("{}\")", "0".repeat(62))),
        serde_json::Value::String("anthony@example.invalid".to_owned()),
    ] {
        value["operator_gravatar"] = malformed.clone();
        assert!(
            wire::decode::<CanvasBootstrap>(&serde_json::to_vec(&value).expect("shape encodes"))
                .is_err(),
            "malformed operator Gravatar was admitted: {malformed}"
        );
    }
}

#[test]
fn canvas_version_has_one_http_header_spelling() {
    assert_eq!(wire::hello::CANVAS_VERSION_HEADER, "x-spice-canvas-version");
    assert_eq!(
        wire::hello::CANVAS_GENERATION_HEADER,
        "x-spice-canvas-generation"
    );
    assert_eq!(
        CanvasVersion::CURRENT.to_string().parse(),
        Ok(CanvasVersion::CURRENT)
    );
    for malformed in ["", "current", "-1", "1.0", "+1", "01", " 1", "1 "] {
        assert!(
            malformed.parse::<CanvasVersion>().is_err(),
            "malformed Canvas version was admitted: {malformed:?}"
        );
    }
}
