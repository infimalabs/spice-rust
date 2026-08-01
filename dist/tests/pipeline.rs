use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use dist::{
    BuildError, Builder, PinError, PinName, Pipeline, PublishError, Publisher, RequiredPins,
    StageError,
};
use wire::{BuildPins, Digest, ReleaseTarget, TargetPayload};

const OLD_PAYLOAD: &[u8] = b"old-payload";

fn pins() -> BuildPins {
    BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: format!("registry.example/spice-proof@sha256:{}", "0".repeat(64)),
    }
}

fn source_digest() -> Digest {
    Digest::from_str(&"1".repeat(64)).expect("fixture digest is canonical")
}

#[derive(Default)]
struct RecordingBuilder {
    calls: Vec<String>,
}

impl Builder for RecordingBuilder {
    fn build_app(&mut self) -> Result<Vec<u8>, BuildError> {
        self.calls.push("app".to_owned());
        Ok(b"wasm-app".to_vec())
    }

    fn embed_app(&mut self, app: &[u8]) -> Result<Vec<u8>, BuildError> {
        self.calls.push("embed".to_owned());
        let mut embedded = b"embedded:".to_vec();
        embedded.extend_from_slice(app);
        Ok(embedded)
    }

    fn build_native(
        &mut self,
        target: ReleaseTarget,
        embedded_app: &[u8],
    ) -> Result<Vec<u8>, BuildError> {
        self.calls.push(format!("native:{target}"));
        let mut native = format!("native:{target}:").into_bytes();
        native.extend_from_slice(embedded_app);
        Ok(native)
    }
}

#[derive(Default)]
struct MemoryPublisher {
    files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
    writes: Vec<PathBuf>,
    fail_at: Option<usize>,
}

impl Publisher for MemoryPublisher {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), PublishError> {
        let attempt = self.writes.len();
        self.writes.push(path.to_path_buf());
        if self.fail_at == Some(attempt) {
            return Err(PublishError::Io {
                path: path.to_path_buf(),
                source: io::Error::other("fixture interruption"),
            });
        }
        self.files
            .insert(path.to_path_buf(), (mode, bytes.to_vec()));
        Ok(())
    }
}

#[test]
fn pin_mismatch_refuses_before_the_first_build_or_write() {
    let required = pins();
    let mut observed = required.clone();
    observed.trunk = "trunk 0.21.13".to_owned();
    let pipeline = Pipeline::new(RequiredPins::new(required).expect("pins are valid"));
    let mut builder = RecordingBuilder::default();
    let mut publisher = MemoryPublisher::default();

    let error = pipeline
        .stage(
            ReleaseTarget::Aarch64AppleDarwin,
            source_digest(),
            &observed,
            &mut builder,
            &mut publisher,
        )
        .expect_err("pin drift must refuse");

    assert!(matches!(
        error,
        StageError::Pin(PinError::Mismatch {
            name: PinName::Trunk,
            ..
        })
    ));
    assert!(builder.calls.is_empty());
    assert!(publisher.writes.is_empty());
}

#[test]
fn build_order_payload_bytes_and_restaging_are_deterministic() {
    let required = pins();
    let pipeline = Pipeline::new(RequiredPins::new(required.clone()).expect("pins are valid"));
    let first = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);
    let second = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);

    assert_eq!(first.staged, second.staged);
    assert_eq!(first.publisher.files, second.publisher.files);
    assert_eq!(
        first.builder.calls,
        ["app", "embed", "native:aarch64-apple-darwin"]
    );
    assert_eq!(
        first.publisher.writes.last(),
        Some(&PathBuf::from("targets/aarch64-apple-darwin/payload.json"))
    );
    let decoded: TargetPayload =
        wire::decode(&first.staged.payload_bytes).expect("payload is current wire");
    assert_eq!(decoded, first.staged.payload);
    assert_eq!(
        decoded
            .inventory
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["app/app.wasm", "bin/spice", "embed/app.bin"]
    );
}

#[test]
fn interrupted_object_publication_preserves_the_old_target_pointer() {
    let required = pins();
    let pipeline = Pipeline::new(RequiredPins::new(required.clone()).expect("pins are valid"));
    let payload_path = PathBuf::from("targets/aarch64-apple-darwin/payload.json");
    let mut publisher = MemoryPublisher {
        files: BTreeMap::from([(payload_path.clone(), (0o644, OLD_PAYLOAD.to_vec()))]),
        writes: Vec::new(),
        fail_at: Some(1),
    };
    let mut builder = RecordingBuilder::default();

    let error = pipeline
        .stage(
            ReleaseTarget::Aarch64AppleDarwin,
            source_digest(),
            &required,
            &mut builder,
            &mut publisher,
        )
        .expect_err("publication interruption must surface");

    assert!(matches!(error, StageError::Publish { .. }));
    assert_eq!(publisher.files[&payload_path].1, OLD_PAYLOAD);
    assert!(!publisher.writes.contains(&payload_path));
}

#[test]
fn target_payload_pointers_and_native_identities_do_not_alias() {
    let required = pins();
    let pipeline = Pipeline::new(RequiredPins::new(required.clone()).expect("pins are valid"));
    let darwin = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);
    let linux = run_stage(&pipeline, &required, ReleaseTarget::X86_64UnknownLinuxMusl);

    assert_ne!(darwin.staged.payload_path, linux.staged.payload_path);
    assert_ne!(darwin.staged.payload.native, linux.staged.payload.native);
    assert_ne!(darwin.staged.payload_digest, linux.staged.payload_digest);
    assert_eq!(darwin.staged.payload.app, linux.staged.payload.app);
    assert_eq!(
        darwin.staged.payload.embedded_app,
        linux.staged.payload.embedded_app
    );
}

#[test]
fn proof_container_must_be_bound_by_an_oci_digest() {
    let mut unpinned = pins();
    unpinned.proof_container = "registry.example/spice-proof:latest".to_owned();

    assert!(matches!(
        RequiredPins::new(unpinned),
        Err(PinError::ContainerNotDigestPinned { .. })
    ));
}

struct Run {
    builder: RecordingBuilder,
    publisher: MemoryPublisher,
    staged: dist::StagedTarget,
}

fn run_stage(pipeline: &Pipeline, pins: &BuildPins, target: ReleaseTarget) -> Run {
    let mut builder = RecordingBuilder::default();
    let mut publisher = MemoryPublisher::default();
    let staged = pipeline
        .stage(target, source_digest(), pins, &mut builder, &mut publisher)
        .expect("fixture stage succeeds");
    Run {
        builder,
        publisher,
        staged,
    }
}
