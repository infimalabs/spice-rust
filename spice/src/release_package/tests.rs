use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{
    BuildPins, MacSymbolMetadata, ProofEnvironment, ProofReceipt, ProofStanding,
    RELEASE_MANIFEST_VERSION, ReleaseManifestTarget, TargetPayload,
};

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn admitted_executables_are_read_from_their_admitted_generation() {
    let fixture = Fixture::new();
    let index = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest_identity,
    )
    .unwrap_or_else(|error| panic!("content-addressed package admission failed: {error}"));
    let expected_executables = fixture
        .executables
        .iter()
        .map(|path| fs::read(path).expect("read admitted executable fixture"))
        .collect::<Vec<_>>();
    let replaced = fixture.root.join("replaced-executable");
    fs::rename(&fixture.executables[0], &replaced).expect("replace admitted executable path");
    fs::write(&fixture.executables[0], b"later generation")
        .expect("publish later executable generation");
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
    ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest_identity,
    )
    .expect_err("mismatched executable object must refuse");

    let fixture = Fixture::new();
    fs::remove_file(&fixture.executables[0]).expect("remove executable object");
    ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest_identity,
    )
    .expect_err("missing executable object must refuse");

    assert_symbol_object_refusals();

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
        &fixture.manifest_identity,
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
    fixture.manifest_identity = Digest::compute(Domain::DistReleaseManifest, &fixture.manifest);
    fs::write(&fixture.manifest_path, &fixture.manifest)
        .expect("publish oversized manifest fixture");
    let overflow = ReleaseIndex::admit(
        &fixture.root,
        &fixture.version,
        fixture.notes_identity.clone(),
        &fixture.notes,
        &fixture.manifest_identity,
    )
    .expect_err("late aggregate overflow refuses before an earlier body is read");
    assert!(
        overflow.to_string().contains("1073741824 aggregate bytes"),
        "preflight must outrank the unread corrupt payload: {overflow}",
    );
}

fn assert_symbol_object_refusals() {
    for (remove, expected) in [(false, "manifest declares"), (true, "cannot read staged")] {
        let fixture = Fixture::new();
        let symbols = fixture.symbols[0]
            .as_ref()
            .expect("Darwin fixture has a symbol object");
        if remove {
            fs::remove_file(symbols).expect("remove symbol object");
        } else {
            fs::write(symbols, b"changed symbols").expect("replace symbol object");
        }
        let refusal = ReleaseIndex::admit(
            &fixture.root,
            &fixture.version,
            fixture.notes_identity.clone(),
            &fixture.notes,
            &fixture.manifest_identity,
        )
        .expect_err("missing or mismatched symbol object must refuse");
        assert!(refusal.to_string().contains(expected));
    }
}

struct Fixture {
    root: PathBuf,
    version: ProductVersion,
    notes: Vec<u8>,
    notes_identity: Digest,
    manifest: Vec<u8>,
    manifest_identity: Digest,
    manifest_path: PathBuf,
    executables: Vec<PathBuf>,
    payloads: Vec<PathBuf>,
    symbols: Vec<Option<PathBuf>>,
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
        let app = Digest::compute(Domain::DistArtifact, b"app generation");
        let pins = BuildPins {
            rust: "rustc 1.97.1".to_owned(),
            wasm_target: "wasm32-unknown-unknown".to_owned(),
            trunk: "trunk 0.21.14".to_owned(),
            cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
            proof_container: format!("sha256:{}", "1".repeat(64)),
        };
        let battery = Digest::compute(Domain::ProofBattery, b"battery");
        let staged = [
            ReleaseTarget::Aarch64AppleDarwin,
            ReleaseTarget::X86_64UnknownLinuxMusl,
        ]
        .map(|target| stage_target(&stage, target, &source, &app, &pins, &battery));
        let targets = staged
            .iter()
            .map(|target| target.manifest.clone())
            .collect();
        let executables = staged
            .iter()
            .map(|target| target.executable.clone())
            .collect();
        let symbols = staged.iter().map(|target| target.symbols.clone()).collect();
        let payloads = staged.into_iter().map(|target| target.payload).collect();
        let manifest = wire::encode(&ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets,
        })
        .expect("manifest encodes");
        let manifest_identity = Digest::compute(Domain::DistReleaseManifest, &manifest);
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
            manifest_identity,
            manifest_path,
            executables,
            payloads,
            symbols,
        }
    }
}

struct StagedFixtureTarget {
    manifest: ReleaseManifestTarget,
    executable: PathBuf,
    payload: PathBuf,
    symbols: Option<PathBuf>,
}

fn stage_target(
    stage: &Path,
    target: ReleaseTarget,
    source: &Digest,
    app: &Digest,
    pins: &BuildPins,
    battery: &Digest,
) -> StagedFixtureTarget {
    let executable = format!("native:{target}").into_bytes();
    let mut inventory = stage_inventory(stage, &executable);
    let native = Digest::compute(Domain::DistArtifact, &executable);
    let symbols = (target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
        let metadata = MacSymbolMetadata {
            path: "symbols/spice.dSYM.json".to_owned(),
            mode: 0o644,
            bytes: 14,
            digest: Digest::compute(Domain::DistArtifact, b"symbol fixture"),
            uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
            executable: native.clone(),
        };
        let entry = InventoryEntry {
            path: metadata.path.clone(),
            mode: metadata.mode,
            bytes: metadata.bytes,
            digest: metadata.digest.clone(),
        };
        write_object_file(stage, &entry, b"symbol fixture");
        inventory.push(entry);
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        metadata
    });
    let manifest_inventory = inventory.clone();
    let payload = TargetPayload {
        version: wire::manifest::TARGET_PAYLOAD_VERSION,
        target,
        source: source.clone(),
        app: app.clone(),
        canvas: Digest::compute(Domain::DistArtifact, b"canvas archive"),
        native,
        symbols: symbols.clone(),
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
    let symbol_path = symbols
        .as_ref()
        .map(|metadata| stage.join(format!("objects/{}-{:04o}", metadata.digest, metadata.mode)));
    StagedFixtureTarget {
        executable: stage.join(format!(
            "objects/{}-0755",
            Digest::compute(Domain::DistArtifact, &executable)
        )),
        payload: payload_path,
        symbols: symbol_path,
        manifest: ReleaseManifestTarget {
            target,
            payload: payload_identity,
            payload_bytes: payload.len() as u64,
            inventory: manifest_inventory,
            symbols,
            receipt: proof_identity,
            receipt_bytes: proof.len() as u64,
            battery: battery.clone(),
            replay,
            environment,
            standing: ProofStanding::Clean,
        },
    }
}

fn stage_inventory(stage: &Path, executable: &[u8]) -> Vec<InventoryEntry> {
    let entry = InventoryEntry {
        path: "bin/spice".to_owned(),
        mode: 0o755,
        bytes: executable.len() as u64,
        digest: Digest::compute(Domain::DistArtifact, executable),
    };
    write_object_file(stage, &entry, executable);
    vec![entry]
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
