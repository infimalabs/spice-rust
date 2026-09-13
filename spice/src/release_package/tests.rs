use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{
    BuildPins, ProofEnvironment, ProofReceipt, ProofStanding, RELEASE_MANIFEST_VERSION,
    ReleaseManifestTarget, TargetPayload,
};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn package_preview_reads_only_content_addressed_executables() {
    let fixture = Fixture::new();
    let index = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest,
    )
    .unwrap_or_else(|error| panic!("content-addressed package admission failed: {error}"));
    let preview = index.preview().clone();
    let expected_executables = fixture
        .executables
        .iter()
        .map(|path| fs::read(path).expect("read admitted executable fixture"))
        .collect::<Vec<_>>();
    let replaced = fixture.root.join("replaced-executable");
    fs::rename(&fixture.executables[0], &replaced).expect("replace admitted executable path");
    fs::write(&fixture.executables[0], b"later generation")
        .expect("publish later executable generation");
    let replaced_manifest = fixture.root.join("replaced-manifest");
    fs::rename(&fixture.manifest_path, &replaced_manifest).expect("replace admitted manifest path");
    fs::write(&fixture.manifest_path, b"later generation")
        .expect("publish later manifest generation");
    let materialized = index.materialize(&fixture.root).unwrap_or_else(|error| {
        panic!("content-addressed package materialization failed: {error}")
    });
    assert_eq!(materialized, preview);
    for (target_index, target) in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ]
    .into_iter()
    .enumerate()
    {
        let mut executable = Vec::new();
        index
            .target_executable(target)
            .unwrap_or_else(|error| panic!("sealed executable load failed: {error}"))
            .copy_to(&mut executable)
            .expect("fixture executable copies through its admitted budget");
        assert_eq!(executable, expected_executables[target_index]);
        assert!(
            !fixture
                .root
                .join("dist/stage/targets")
                .join(target.as_str())
                .join("bin/spice")
                .exists()
        );
    }
}

#[test]
fn package_preview_refuses_missing_and_mismatched_executable_objects() {
    let fixture = Fixture::new();
    fs::write(&fixture.executables[0], b"changed").expect("replace executable object");
    let mismatch = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest,
    )
    .expect_err("mismatched executable object must refuse");
    assert!(mismatch.to_string().contains("manifest declares"));

    let fixture = Fixture::new();
    fs::remove_file(&fixture.executables[0]).expect("remove executable object");
    let missing = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest,
    )
    .expect_err("missing executable object must refuse");
    assert!(missing.to_string().contains("cannot read staged"));

    let fixture = Fixture::new();
    let mut moved_manifest = fixture.manifest.clone();
    moved_manifest[0] = b'[';
    fs::write(&fixture.manifest_path, moved_manifest)
        .expect("replace staged manifest without changing its length");
    let moved = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest,
    )
    .expect_err("staged manifest mismatch must refuse");
    assert!(
        moved
            .to_string()
            .contains("staged release manifest object disagrees")
    );

    let mut count = CaptureBudget::default();
    for index in 0..MAX_RELEASE_MEMBERS {
        count
            .admit(&format!("member-{index}"), 0, MAX_RELEASE_MEMBER_BYTES)
            .expect("member through exact count bound");
    }
    assert!(
        count
            .admit("one-too-many", 0, MAX_RELEASE_MEMBER_BYTES)
            .expect_err("member count beyond fixed ceiling refuses")
            .to_string()
            .contains("32 members")
    );

    let mut aggregate = CaptureBudget::default();
    aggregate
        .admit("first", MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBER_BYTES)
        .expect("first maximum member fits aggregate");
    aggregate
        .admit("second", MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBER_BYTES)
        .expect("second maximum member fits aggregate");
    assert!(
        aggregate
            .admit("aggregate-overflow", 1, MAX_RELEASE_MEMBER_BYTES)
            .expect_err("aggregate beyond fixed ceiling refuses")
            .to_string()
            .contains("1073741824 aggregate bytes")
    );

    let mut fixture = Fixture::new();
    let mut first_payload = fs::read(&fixture.payloads[0]).expect("read first payload fixture");
    first_payload[0] = b'[';
    fs::write(&fixture.payloads[0], first_payload)
        .expect("corrupt earlier payload without changing its length");
    let mut manifest: ReleaseManifest =
        wire::decode(&fixture.manifest).expect("fixture manifest decodes");
    for (target, executable) in manifest.targets.iter_mut().zip(&fixture.executables) {
        inventory_entry(&target.inventory, "bin/spice").expect("fixture executable exists");
        target
            .inventory
            .iter_mut()
            .find(|entry| entry.path == "bin/spice")
            .expect("fixture executable is mutable")
            .bytes = MAX_RELEASE_MEMBER_BYTES;
        fs::OpenOptions::new()
            .write(true)
            .open(executable)
            .expect("open sparse executable")
            .set_len(MAX_RELEASE_MEMBER_BYTES)
            .expect("extend sparse executable");
    }
    fixture.manifest = wire::encode(&manifest).expect("oversized manifest encodes");
    fs::write(&fixture.manifest_path, &fixture.manifest)
        .expect("publish oversized manifest fixture");
    let overflow = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest,
    )
    .expect_err("late aggregate overflow refuses before an earlier body is read");
    assert!(
        overflow.to_string().contains("1073741824 aggregate bytes"),
        "preflight must outrank the unread corrupt payload: {overflow}",
    );
}

struct Fixture {
    root: PathBuf,
    version: ProductVersion,
    notes: Vec<u8>,
    notes_identity: Digest,
    manifest: Vec<u8>,
    manifest_path: PathBuf,
    executables: Vec<PathBuf>,
    payloads: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-package-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create package fixture root");
        crate::test_repository::initialize(&root);
        let stage = root.join("dist/stage");
        fs::create_dir_all(stage.join("objects")).expect("create object stage");
        fs::create_dir_all(stage.join("release")).expect("create release stage");
        let source = Digest::compute(Domain::DistSourceCommit, b"source");
        let app = Digest::compute(Domain::DistArtifact, b"Canvas generation");
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!("sha256:{}", "1".repeat(64)),
        };
        let app_members = CANVAS_APP_FILES
            .iter()
            .map(|file| (format!("app/{file}"), format!("asset:{file}").into_bytes()))
            .collect::<Vec<_>>();
        let battery = Digest::compute(Domain::ProofBattery, b"battery");
        let staged = [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
        ]
        .map(|target| stage_target(&stage, target, &source, &app, &pins, &app_members, &battery));
        let targets = staged
            .iter()
            .map(|target| target.manifest.clone())
            .collect();
        let executables = staged
            .iter()
            .map(|target| target.executable.clone())
            .collect();
        let payloads = staged.into_iter().map(|target| target.payload).collect();
        let manifest = wire::encode(&ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets,
        })
        .expect("manifest encodes");
        let manifest_path = stage.join("release/manifest.json");
        fs::write(&manifest_path, &manifest).expect("write staged manifest");
        let notes = b"release notes\n".to_vec();
        let notes_identity = Digest::compute(Domain::DistReleaseNotes, &notes);
        Self {
            root,
            version: ProductVersion::parse("0.1.0").expect("version is canonical"),
            notes,
            notes_identity,
            manifest,
            manifest_path,
            executables,
            payloads,
        }
    }
}

struct StagedFixtureTarget {
    manifest: ReleaseManifestTarget,
    executable: PathBuf,
    payload: PathBuf,
}

fn stage_target(
    stage: &Path,
    target: ReleaseTarget,
    source: &Digest,
    app: &Digest,
    pins: &BuildPins,
    app_members: &[(String, Vec<u8>)],
    battery: &Digest,
) -> StagedFixtureTarget {
    let executable = format!("native:{target}").into_bytes();
    let inventory = stage_inventory(stage, app_members, &executable);
    let manifest_inventory = inventory.clone();
    let payload = TargetPayload {
        version: wire::manifest::TARGET_PAYLOAD_VERSION,
        target,
        source: source.clone(),
        app: app.clone(),
        native: Digest::compute(Domain::DistArtifact, &executable),
        pins: pins.clone(),
        pins_digest: Digest::compute(Domain::DistPins, &wire::encode(pins).expect("pins encode")),
        inventory_digest: Digest::compute(
            Domain::DistInventory,
            &wire::encode(&inventory).expect("inventory encodes"),
        ),
        inventory,
    };
    let payload = wire::encode(&payload).expect("payload encodes");
    let payload_identity = Digest::compute(Domain::DistTargetPayload, &payload);
    let payload_path = stage.join(format!("objects/payload-{payload_identity}.json"));
    fs::write(&payload_path, &payload).expect("write payload object");
    let replay = Digest::compute(Domain::ProofReplayV3, target.as_str().as_bytes());
    let environment = match target {
        ReleaseTarget::Aarch64AppleDarwin => {
            ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"runner"))
        }
        ReleaseTarget::X86_64UnknownLinuxMusl => {
            ProofEnvironment::oci(&pins.proof_container).expect("image is pinned")
        }
    };
    let proof = wire::encode(&ProofReceipt {
        version: wire::manifest::PROOF_RECEIPT_VERSION,
        target,
        payload: payload_identity.clone(),
        battery: battery.clone(),
        replay: replay.clone(),
        environment: environment.clone(),
        standing: ProofStanding::Clean,
    })
    .expect("proof encodes");
    let proof_identity = Digest::compute(Domain::ProofReceiptV3, &proof);
    fs::write(
        stage.join(format!("objects/receipt-{proof_identity}.json")),
        &proof,
    )
    .expect("write proof object");
    StagedFixtureTarget {
        executable: stage.join(format!(
            "objects/{}-0755",
            Digest::compute(Domain::DistArtifact, &executable)
        )),
        payload: payload_path,
        manifest: ReleaseManifestTarget {
            target,
            payload: payload_identity,
            payload_bytes: payload.len() as u64,
            inventory: manifest_inventory,
            receipt: proof_identity,
            receipt_bytes: proof.len() as u64,
            battery: battery.clone(),
            replay,
            environment,
            standing: ProofStanding::Clean,
        },
    }
}

fn stage_inventory(
    stage: &Path,
    app_members: &[(String, Vec<u8>)],
    executable: &[u8],
) -> Vec<InventoryEntry> {
    let mut inventory = app_members
        .iter()
        .map(|(path, bytes)| InventoryEntry {
            path: path.clone(),
            mode: 0o644,
            bytes: bytes.len() as u64,
            digest: Digest::compute(Domain::DistArtifact, bytes),
        })
        .collect::<Vec<_>>();
    inventory.push(InventoryEntry {
        path: "bin/spice".to_owned(),
        mode: 0o755,
        bytes: executable.len() as u64,
        digest: Digest::compute(Domain::DistArtifact, executable),
    });
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    for entry in &inventory {
        let bytes = if entry.path == "bin/spice" {
            executable
        } else {
            app_members
                .iter()
                .find(|(path, _)| path == &entry.path)
                .map(|(_, bytes)| bytes.as_slice())
                .expect("inventory app path has fixture bytes")
        };
        write_object_file(stage, entry, bytes);
    }
    inventory
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove package fixture");
    }
}

fn write_object_file(stage: &Path, entry: &InventoryEntry, bytes: &[u8]) {
    fs::write(
        stage.join(format!("objects/{}-{:04o}", entry.digest, entry.mode)),
        bytes,
    )
    .expect("write staged object");
}
