use wire::manifest::{
    BuildPins, InventoryEntry, PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt,
    ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseTarget, SealedTarget,
    TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain};

/// Hex width of an OCI `sha256:` reference.
///
/// This is a foreign registry's spelling, not WIRE's. It is unrelated to the
/// BLAKE3 digests [`Digest`] mints and only happens to be the same width.
const OCI_SHA256_HEX_LEN: usize = 64;
const GIT_SHA1_HEX_LEN: usize = 40;
const FIXTURE_JOURNAL_SCHEMA: u32 = 1;

/// Mint a fixture digest the way production mints one.
///
/// Fixtures compute rather than spell a digest out, so a fixture can never
/// carry a spelling that [`Digest::compute`] would not produce.
fn digest(marker: &str) -> Digest {
    Digest::compute(Domain::DistArtifact, marker.as_bytes())
}

fn payload(target: ReleaseTarget, marker: &str) -> TargetPayload {
    let artifact = digest(marker);
    TargetPayload {
        version: TARGET_PAYLOAD_VERSION,
        target,
        source: digest("source"),
        app: artifact.clone(),
        embedded_app: digest("embedded-app"),
        native: digest("native"),
        pins: BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!(
                "registry.example/proof@sha256:{}",
                "4".repeat(OCI_SHA256_HEX_LEN)
            ),
        },
        pins_digest: Digest::compute(Domain::DistPins, marker.as_bytes()),
        inventory: vec![InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: 12,
            digest: artifact,
        }],
        inventory_digest: Digest::compute(Domain::DistInventory, marker.as_bytes()),
    }
}

fn release_manifest() -> ReleaseManifest {
    let targets = ReleaseTarget::ALL
        .into_iter()
        .map(|target| {
            let payload = payload(target, target.as_str());
            let payload_digest = Digest::compute(
                Domain::DistTargetPayload,
                &wire::encode(&payload).expect("payload encodes"),
            );
            let mut receipt = receipt(target, target.as_str());
            receipt.payload = payload_digest.clone();
            let receipt_digest = Digest::compute(
                Domain::ProofReceiptV2,
                &wire::encode(&receipt).expect("receipt encodes"),
            );
            SealedTarget {
                target,
                payload,
                payload_digest,
                receipt,
                receipt_digest,
            }
        })
        .collect();
    ReleaseManifest {
        version: RELEASE_MANIFEST_VERSION,
        release: "1.0.0".to_owned(),
        git_from: "1".repeat(GIT_SHA1_HEX_LEN),
        git_to: "2".repeat(GIT_SHA1_HEX_LEN),
        previous_journal_schema: FIXTURE_JOURNAL_SCHEMA,
        journal_schema: FIXTURE_JOURNAL_SCHEMA,
        targets,
    }
}

fn receipt(target: ReleaseTarget, marker: &str) -> ProofReceipt {
    ProofReceipt {
        version: PROOF_RECEIPT_VERSION,
        target,
        payload: Digest::compute(Domain::DistTargetPayload, marker.as_bytes()),
        battery: Digest::compute(Domain::ProofBattery, marker.as_bytes()),
        replay: Digest::compute(Domain::ProofReplayV2, marker.as_bytes()),
        environment: ProofEnvironment::oci(format!(
            "registry.example/proof@sha256:{}",
            "4".repeat(64)
        ))
        .expect("fixture image is pinned"),
        standing: ProofStanding::Clean,
    }
}

#[test]
fn proof_environments_have_one_strict_canonical_wire_shape() {
    let oci = ProofEnvironment::oci(format!("registry.example/proof@sha256:{}", "4".repeat(64)))
        .expect("canonical OCI identity");
    let native = ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"runner"));

    assert_eq!(
        wire::encode(&oci).expect("OCI identity encodes"),
        format!(
            r#"{{"oci":{{"image":"registry.example/proof@sha256:{}"}}}}"#,
            "4".repeat(64)
        )
        .as_bytes()
    );
    assert_eq!(
        wire::decode::<ProofEnvironment>(&wire::encode(&native).expect("native identity encodes"))
            .expect("native identity decodes"),
        native
    );

    let digest = "4".repeat(64);
    let refused = [
        format!(r#"{{"container":{{"image":"sha256:{digest}"}}}}"#),
        r#"{"oci":{"image":"proof:latest"}}"#.to_owned(),
        format!(r#"{{"oci":{{"image":"Registry/proof@sha256:{digest}"}}}}"#),
        format!(r#"{{"oci":{{"image":"registry.example/proof:latest@sha256:{digest}"}}}}"#),
        format!(r#"{{"oci":{{"image":"registry.example//proof@sha256:{digest}"}}}}"#),
        format!(r#"{{"oci":{{"image":"registry.example/proof..alias@sha256:{digest}"}}}}"#),
        format!(r#"{{"oci":{{"image":"sha256:{}"}}}}"#, "A".repeat(64)),
        format!(r#"{{"oci":{{"image":"sha256:{digest}","unknown":true}}}}"#),
        format!(r#"{{"native":{{"runner":"{digest}","unknown":true}}}}"#),
    ];
    for refused in refused {
        assert!(
            wire::decode::<ProofEnvironment>(refused.as_bytes()).is_err(),
            "noncanonical environment must refuse: {refused}"
        );
    }
}

#[test]
fn release_payloads_round_trip_without_aliasing_targets() {
    let darwin = payload(ReleaseTarget::Aarch64AppleDarwin, "darwin");
    let linux = payload(ReleaseTarget::X86_64UnknownLinuxMusl, "linux");
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
fn detached_proof_receipts_round_trip_in_the_current_shape() {
    let receipt = receipt(ReleaseTarget::X86_64UnknownLinuxMusl, "linux-proof");
    let bytes = wire::encode(&receipt).expect("proof receipt encodes");

    assert_eq!(
        wire::decode::<ProofReceipt>(&bytes).expect("proof receipt decodes"),
        receipt
    );

    let mut changed: serde_json::Value = serde_json::from_slice(&bytes).expect("receipt is JSON");
    changed["unknown"] = serde_json::Value::Bool(true);
    assert!(
        wire::decode::<ProofReceipt>(
            &serde_json::to_vec(&changed).expect("changed receipt is JSON")
        )
        .is_err(),
        "a receipt outside the current shape must refuse"
    );

    let mut retired: serde_json::Value = serde_json::from_slice(&bytes).expect("receipt is JSON");
    retired["version"] = serde_json::Value::from(1);
    let retired_environment = format!("registry.example/proof@sha256:{}", "4".repeat(64));
    retired["environment"] = serde_json::Value::String(retired_environment);
    assert!(
        wire::decode::<ProofReceipt>(
            &serde_json::to_vec(&retired).expect("retired receipt is JSON")
        )
        .is_err(),
        "the retired string-valued environment must not remain as an alternate schema"
    );
}

#[test]
fn payloads_refuse_unknown_fields_at_every_structured_level() {
    let encoded = wire::encode(&payload(ReleaseTarget::Aarch64AppleDarwin, "darwin"))
        .expect("payload encodes");
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

#[test]
fn sealed_release_manifests_round_trip_and_refuse_unknown_nested_fields() {
    let manifest = release_manifest();
    let encoded = wire::encode(&manifest).expect("release manifest encodes");
    assert_eq!(
        wire::decode::<ReleaseManifest>(&encoded).expect("release manifest decodes"),
        manifest
    );
    let original: serde_json::Value =
        serde_json::from_slice(&encoded).expect("release manifest is JSON");
    for path in [Vec::new(), vec!["targets", "0"]] {
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
        assert!(
            wire::decode::<ReleaseManifest>(
                &serde_json::to_vec(&changed).expect("changed manifest encodes")
            )
            .is_err(),
            "unknown field at {path:?} must refuse"
        );
    }
}
