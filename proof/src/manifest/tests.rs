use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{
    BuildPins, InventoryEntry, ProofEnvironment, ProofReceipt, ReleaseManifest, ReleaseTarget,
    TargetPayload,
};
use wire::{Digest, Domain};

use super::{
    FileManifestPublisher, MANIFEST_POINTER, ManifestAdmissionError, ManifestError, ManifestInput,
    ManifestPublishError, ManifestPublisher, admit_manifest, object_path, publish_manifest,
};
use crate::{Battery, CheckKind, CheckStanding, EvidenceError, ReplayVerdict};

const OLD_POINTER: &[u8] = b"old release manifest";
static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Prepared {
    payload: Vec<u8>,
    receipt: Vec<u8>,
    replay: ReplayVerdict,
}

#[derive(Default)]
struct MemoryPublisher {
    files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
    writes: Vec<PathBuf>,
    fail_at: Option<usize>,
}

impl ManifestPublisher for MemoryPublisher {
    fn publish(
        &mut self,
        path: &Path,
        mode: u32,
        bytes: &[u8],
    ) -> Result<(), ManifestPublishError> {
        let attempt = self.writes.len();
        self.writes.push(path.to_path_buf());
        if self.fail_at == Some(attempt) {
            return Err(ManifestPublishError::Io {
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
fn both_verified_targets_form_one_stable_manifest_and_pointer_last() {
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source.clone(), true);
    let mut first = MemoryPublisher::default();
    let published =
        publish_manifest(inputs(&darwin, &linux), &mut first).expect("both clean proofs publish");
    let mut second = MemoryPublisher::default();
    let repeated = publish_manifest(inputs(&darwin, &linux), &mut second)
        .expect("the same proofs publish identically");

    assert_eq!(published, repeated);
    assert_eq!(first.files, second.files);
    assert_eq!(published.manifest.source, source);
    assert_eq!(published.manifest.targets.len(), 2);
    assert_eq!(
        published
            .manifest
            .targets
            .iter()
            .map(|entry| entry.target)
            .collect::<Vec<_>>(),
        [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
        ]
    );
    assert_eq!(
        published.digest,
        Digest::compute(Domain::DistReleaseManifest, &published.bytes)
    );
    let decoded: ReleaseManifest =
        wire::decode(&published.bytes).expect("manifest bytes are canonical wire");
    assert_eq!(decoded, published.manifest);
    assert_eq!(first.writes.last(), Some(&published.pointer));
    assert_eq!(published.pointer, PathBuf::from(MANIFEST_POINTER));
    assert!(
        first.writes[..first.writes.len() - 1]
            .iter()
            .all(|path| path.starts_with("objects")),
        "every immutable object precedes the pointer"
    );
}

#[test]
fn missing_extra_duplicate_and_reordered_targets_refuse_before_publication() {
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);

    let cases = [
        (
            vec![input(&darwin)],
            "missing",
            matches_error as fn(&ManifestError) -> bool,
        ),
        (
            vec![input(&darwin), input(&linux), input(&darwin)],
            "extra",
            matches_error,
        ),
        (
            vec![input(&darwin), input(&darwin)],
            "duplicate",
            duplicate_error,
        ),
        (
            vec![input(&linux), input(&darwin)],
            "reordered",
            order_error,
        ),
    ];

    for (inputs, label, expected) in cases {
        let mut publisher = MemoryPublisher::default();
        let error =
            publish_manifest(inputs, &mut publisher).expect_err("invalid target set must refuse");
        assert!(expected(&error), "{label} produced {error:?}");
        assert!(publisher.writes.is_empty(), "{label} wrote before refusal");
    }
}

fn matches_error(error: &ManifestError) -> bool {
    matches!(error, ManifestError::EntryCount(_))
}

fn duplicate_error(error: &ManifestError) -> bool {
    matches!(error, ManifestError::DuplicateTarget(_))
}

fn order_error(error: &ManifestError) -> bool {
    matches!(error, ManifestError::TargetOrder { .. })
}

#[test]
fn mixed_sources_mismatched_evidence_and_non_clean_proofs_refuse_without_writes() {
    let first_source = digest(Domain::DistArtifact, b"first source");
    let other_source = digest(Domain::DistArtifact, b"other source");
    let darwin = prepared(
        ReleaseTarget::Aarch64AppleDarwin,
        first_source.clone(),
        true,
    );
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, first_source, true);
    let other_linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, other_source, true);
    let drifted_linux = drift_toolchain(&linux);
    let refused_linux = prepared(
        ReleaseTarget::X86_64UnknownLinuxMusl,
        digest(Domain::DistArtifact, b"first source"),
        false,
    );

    let mut publisher = MemoryPublisher::default();
    let mixed = publish_manifest(inputs(&darwin, &other_linux), &mut publisher)
        .expect_err("mixed sources refuse");
    assert!(matches!(mixed, ManifestError::SourceMismatch { .. }));
    assert!(publisher.writes.is_empty());

    let mut publisher = MemoryPublisher::default();
    let mixed = publish_manifest(inputs(&darwin, &drifted_linux), &mut publisher)
        .expect_err("mixed toolchain pins refuse");
    assert!(matches!(mixed, ManifestError::Mixed { .. }));
    assert!(publisher.writes.is_empty());

    let mismatched = ManifestInput::new(&darwin.payload, &linux.receipt, &linux.replay);
    let mut publisher = MemoryPublisher::default();
    let mismatch = publish_manifest([mismatched, input(&linux)], &mut publisher)
        .expect_err("cross-target evidence refuses");
    assert!(matches!(mismatch, ManifestError::Evidence { .. }));
    assert!(publisher.writes.is_empty());

    let mut publisher = MemoryPublisher::default();
    let refused = publish_manifest(inputs(&darwin, &refused_linux), &mut publisher)
        .expect_err("non-clean proof refuses");
    assert!(matches!(
        refused,
        ManifestError::Evidence {
            source: EvidenceError::Refused,
            ..
        }
    ));
    assert!(publisher.writes.is_empty());
}

#[test]
fn every_receipt_binding_is_rechecked_before_the_first_write() {
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);

    for field in 0..5 {
        let mut receipt: ProofReceipt =
            wire::decode(&linux.receipt).expect("fixture receipt decodes");
        match field {
            0 => receipt.target = ReleaseTarget::Aarch64AppleDarwin,
            1 => receipt.payload = digest(Domain::DistTargetPayload, b"other payload"),
            2 => receipt.battery = digest(Domain::ProofBattery, b"other battery"),
            3 => receipt.replay = digest(Domain::ProofReplayV3, b"other replay"),
            4 => {
                receipt.environment = ProofEnvironment::native(digest(
                    Domain::ProofNativeRunner,
                    b"wrong environment",
                ));
            }
            _ => unreachable!("the field sweep is closed"),
        }
        let receipt = wire::encode(&receipt).expect("tampered receipt remains canonical");
        let tampered = ManifestInput::new(&linux.payload, &receipt, &linux.replay);
        let mut publisher = MemoryPublisher::default();

        let error = publish_manifest([input(&darwin), tampered], &mut publisher)
            .expect_err("every receipt binding is load-bearing");

        assert!(matches!(error, ManifestError::Evidence { .. }));
        assert!(
            publisher.writes.is_empty(),
            "receipt field {field} wrote before refusal"
        );
    }
}

#[test]
fn interrupted_object_publication_preserves_the_previous_pointer() {
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
    let pointer = PathBuf::from(MANIFEST_POINTER);
    let mut publisher = MemoryPublisher {
        files: BTreeMap::from([(pointer.clone(), (0o644, OLD_POINTER.to_vec()))]),
        writes: Vec::new(),
        fail_at: Some(2),
    };

    let error = publish_manifest(inputs(&darwin, &linux), &mut publisher)
        .expect_err("interrupted object write fails");

    assert!(matches!(error, ManifestError::Publish { .. }));
    assert_eq!(publisher.files[&pointer].1, OLD_POINTER);
    assert!(!publisher.writes.contains(&pointer));
}

#[test]
fn filesystem_publication_commits_exact_objects_and_pointer() {
    let scratch = Scratch::new();
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
    let mut publisher = FileManifestPublisher::new(&scratch.root);

    let published = publish_manifest(inputs(&darwin, &linux), &mut publisher)
        .expect("filesystem publication succeeds");
    let darwin_payload: TargetPayload =
        wire::decode(&darwin.payload).expect("Darwin payload decodes");
    let admitted = admit_manifest(
        &scratch.root,
        &darwin_payload.app,
        ReleaseTarget::Aarch64AppleDarwin,
        &darwin_payload.native,
    )
    .expect("the complete published object graph admits");

    assert_eq!(admitted, published);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let linked = scratch.root.join("linked-stage");
        symlink(&scratch.root, &linked).expect("stage symlink fixture");
        assert!(matches!(
            admit_manifest(
                &linked,
                &darwin_payload.app,
                ReleaseTarget::Aarch64AppleDarwin,
                &darwin_payload.native,
            ),
            Err(ManifestAdmissionError::StageSymlink(_))
        ));
    }
    assert_eq!(
        fs::read(scratch.root.join(&published.pointer)).expect("pointer is readable"),
        published.bytes
    );
    let objects = fs::read_dir(scratch.root.join("objects"))
        .expect("objects directory is readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("every object entry is readable");
    assert_eq!(objects.len(), 5, "two payloads, two receipts, one manifest");
    assert!(
        objects
            .iter()
            .all(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        assert_eq!(
            fs::metadata(scratch.root.join(&published.pointer))
                .expect("pointer metadata is readable")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
}

#[test]
fn manifest_admission_refuses_missing_stale_and_mixed_objects() {
    let source = digest(Domain::DistArtifact, b"source");
    let darwin = prepared(ReleaseTarget::Aarch64AppleDarwin, source.clone(), true);
    let linux = prepared(ReleaseTarget::X86_64UnknownLinuxMusl, source, true);
    let expected: TargetPayload = wire::decode(&darwin.payload).expect("payload decodes");

    let missing = Scratch::new();
    let mut publisher = FileManifestPublisher::new(&missing.root);
    let published =
        publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
    fs::remove_file(missing.root.join(object_path(
        "receipt",
        &published.manifest.targets[1].receipt,
    )))
    .expect("remove receipt object");
    assert!(matches!(
        admit_manifest(
            &missing.root,
            &expected.app,
            expected.target,
            &expected.native,
        ),
        Err(ManifestAdmissionError::Input { .. })
    ));

    let stale = Scratch::new();
    let mut publisher = FileManifestPublisher::new(&stale.root);
    publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
    let mut pointer: ReleaseManifest =
        wire::decode(&fs::read(stale.root.join(MANIFEST_POINTER)).expect("read pointer"))
            .expect("pointer decodes");
    pointer.version += 1;
    fs::write(
        stale.root.join(MANIFEST_POINTER),
        wire::encode(&pointer).expect("stale pointer encodes"),
    )
    .expect("write stale pointer");
    assert!(matches!(
        admit_manifest(
            &stale.root,
            &expected.app,
            expected.target,
            &expected.native,
        ),
        Err(ManifestAdmissionError::ManifestVersion(_))
    ));

    let mixed = Scratch::new();
    let mut publisher = FileManifestPublisher::new(&mixed.root);
    let published =
        publish_manifest(inputs(&darwin, &linux), &mut publisher).expect("fixture publishes");
    fs::write(
        mixed.root.join(object_path(
            "payload",
            &published.manifest.targets[1].payload,
        )),
        &darwin.payload,
    )
    .expect("replace Linux object with Darwin bytes");
    assert!(matches!(
        admit_manifest(
            &mixed.root,
            &expected.app,
            expected.target,
            &expected.native,
        ),
        Err(ManifestAdmissionError::Binding { .. })
    ));
}

#[test]
fn filesystem_publisher_refuses_every_path_outside_its_root() {
    let scratch = Scratch::new();
    let mut publisher = FileManifestPublisher::new(&scratch.root);

    for path in [Path::new(""), Path::new("../escape"), Path::new("/escape")] {
        assert!(matches!(
            publisher.publish(path, 0o644, b"escape"),
            Err(ManifestPublishError::InvalidPath(observed)) if observed == path
        ));
    }
    assert_eq!(
        fs::read_dir(&scratch.root)
            .expect("scratch remains readable")
            .count(),
        0,
        "a refused path creates nothing"
    );
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-proof-manifest-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("unique scratch root is created");
        Self { root }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("scratch root is removed");
    }
}

fn inputs<'a>(darwin: &'a Prepared, linux: &'a Prepared) -> [ManifestInput<'a>; 2] {
    [input(darwin), input(linux)]
}

fn input(prepared: &Prepared) -> ManifestInput<'_> {
    ManifestInput::new(&prepared.payload, &prepared.receipt, &prepared.replay)
}

fn prepared(target: ReleaseTarget, source: Digest, clean: bool) -> Prepared {
    let app = digest(Domain::DistArtifact, b"app");
    let native = digest(Domain::DistArtifact, target.as_str().as_bytes());
    let pins = BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: format!("registry.example/spice-proof@sha256:{}", "0".repeat(64)),
    };
    let mut inventory = wire::manifest::CANVAS_APP_FILES
        .iter()
        .map(|file| InventoryEntry {
            path: format!("app/{file}"),
            mode: 0o644,
            bytes: file.len() as u64,
            digest: digest(Domain::DistArtifact, file.as_bytes()),
        })
        .collect::<Vec<_>>();
    inventory.push(InventoryEntry {
        path: "bin/spice".to_owned(),
        mode: 0o755,
        bytes: target.as_str().len() as u64,
        digest: native.clone(),
    });
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    let payload = TargetPayload {
        version: wire::manifest::TARGET_PAYLOAD_VERSION,
        target,
        source,
        app,
        native,
        pins_digest: digest(Domain::DistPins, &wire::encode(&pins).expect("pins encode")),
        inventory_digest: digest(
            Domain::DistInventory,
            &wire::encode(&inventory).expect("inventory encodes"),
        ),
        pins,
        inventory,
    };
    let payload_bytes = wire::encode(&payload).expect("payload encodes");
    let payload_digest = digest(Domain::DistTargetPayload, &payload_bytes);
    let environment = match target {
        ReleaseTarget::Aarch64AppleDarwin => {
            ProofEnvironment::native(digest(Domain::ProofNativeRunner, b"native runner"))
        }
        ReleaseTarget::X86_64UnknownLinuxMusl => {
            ProofEnvironment::oci(&payload.pins.proof_container)
                .expect("fixture OCI image is pinned")
        }
    };
    let battery = Battery::current().digest().clone();
    let replay = if clean {
        ReplayVerdict::fixture_clean(target, environment, battery)
    } else {
        let mut standings = [CheckStanding::Clean; CheckKind::ALL.len()];
        standings[0] = CheckStanding::Failed { code: Some(1) };
        ReplayVerdict::fixture_with(target, environment, battery, standings)
    };
    let receipt = replay
        .receipt(&payload, payload_digest)
        .expect("matching replay makes a receipt");
    Prepared {
        payload: payload_bytes,
        receipt: wire::encode(&receipt).expect("receipt encodes"),
        replay,
    }
}

fn drift_toolchain(prepared: &Prepared) -> Prepared {
    let mut payload: TargetPayload =
        wire::decode(&prepared.payload).expect("fixture payload decodes");
    payload.pins.trunk.push_str("-drifted");
    payload.pins_digest = digest(
        Domain::DistPins,
        &wire::encode(&payload.pins).expect("drifted pins encode"),
    );
    let payload = wire::encode(&payload).expect("drifted payload encodes");
    let receipt = prepared
        .replay
        .receipt(
            &wire::decode(&payload).expect("drifted payload decodes"),
            digest(Domain::DistTargetPayload, &payload),
        )
        .expect("drifted payload remains target matched");
    Prepared {
        payload,
        receipt: wire::encode(&receipt).expect("drifted receipt encodes"),
        replay: prepared.replay.clone(),
    }
}

fn digest(domain: Domain, bytes: &[u8]) -> Digest {
    Digest::compute(domain, bytes)
}
