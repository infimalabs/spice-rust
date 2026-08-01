use std::str::FromStr;

use wire::Digest;
use wire::manifest::{BuildPins, InventoryEntry, ReleaseTarget, TargetPayload};

const SHA256_HEX_LEN: usize = 64;

fn digest(spelling: char) -> Digest {
    Digest::from_str(&spelling.to_string().repeat(SHA256_HEX_LEN))
        .expect("fixture digest is canonical")
}

fn payload(target: ReleaseTarget, marker: char) -> TargetPayload {
    let artifact = digest(marker);
    TargetPayload {
        version: 1,
        target,
        source: digest('1'),
        app: artifact.clone(),
        embedded_app: digest('2'),
        native: digest('3'),
        pins: BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!(
                "registry.example/proof@sha256:{}",
                "4".repeat(SHA256_HEX_LEN)
            ),
        },
        pins_digest: digest('5'),
        inventory: vec![InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: 12,
            digest: artifact,
        }],
        inventory_digest: digest('6'),
    }
}

#[test]
fn release_payloads_round_trip_without_aliasing_targets() {
    let darwin = payload(ReleaseTarget::Aarch64AppleDarwin, 'a');
    let linux = payload(ReleaseTarget::X86_64UnknownLinuxMusl, 'b');
    let darwin_bytes = wire::encode(&darwin).expect("darwin payload encodes");
    let linux_bytes = wire::encode(&linux).expect("linux payload encodes");

    assert_eq!(
        wire::decode::<TargetPayload>(&darwin_bytes).expect("darwin payload decodes"),
        darwin
    );
    assert_eq!(
        wire::decode::<TargetPayload>(&linux_bytes).expect("linux payload decodes"),
        linux
    );
    assert_ne!(darwin_bytes, linux_bytes);
}

#[test]
fn release_targets_encode_as_canonical_rust_triples() {
    assert_eq!(
        wire::encode(&ReleaseTarget::Aarch64AppleDarwin).expect("target encodes"),
        br#""aarch64-apple-darwin""#
    );
    assert_eq!(
        wire::encode(&ReleaseTarget::X86_64UnknownLinuxMusl).expect("target encodes"),
        br#""x86_64-unknown-linux-musl""#
    );
}

#[test]
fn payloads_refuse_unknown_fields_at_every_structured_level() {
    let encoded =
        wire::encode(&payload(ReleaseTarget::Aarch64AppleDarwin, 'a')).expect("payload encodes");
    let original: serde_json::Value = serde_json::from_slice(&encoded).expect("payload is JSON");

    for path in [Vec::new(), vec!["pins"], vec!["inventory", "0"]] {
        let mut changed = original.clone();
        let mut object = &mut changed;
        for &segment in &path {
            object = if let Ok(index) = segment.parse::<usize>() {
                &mut object.as_array_mut().expect("fixture segment is an array")[index]
            } else {
                object
                    .as_object_mut()
                    .expect("fixture segment is an object")
                    .get_mut(segment)
                    .expect("fixture segment exists")
            };
        }
        object
            .as_object_mut()
            .expect("fixture target is an object")
            .insert("unknown".to_owned(), serde_json::Value::Bool(true));

        let bytes = serde_json::to_vec(&changed).expect("changed payload encodes");
        assert!(
            wire::decode::<TargetPayload>(&bytes).is_err(),
            "unknown field at {path:?} must refuse"
        );
    }
}
