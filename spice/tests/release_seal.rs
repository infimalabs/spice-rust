#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use wire::manifest::{
    BuildPins, InventoryEntry, MacSymbolMetadata, PROOF_RECEIPT_VERSION, ProofEnvironment,
    ProofReceipt, ProofStanding, ReleaseTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain, Outcome, ReplyPosition, SealedManifest};

#[path = "support/git_repository.rs"]
mod git_repository;

const READY: Duration = Duration::from_secs(10);
static NEXT: AtomicU64 = AtomicU64::new(0);

#[test]
fn release_seal_publishes_and_recognizes_one_current_manifest() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);

    let first = fixture.invoke(&["--json", "release", "seal"]);
    assert!(first.status.success(), "seal failed: {}", spoken(&first));
    let Outcome::Reply(first): Outcome<SealedManifest> =
        wire::decode(&first.stdout).expect("seal reply decodes")
    else {
        panic!("seal did not reply: {}", spoken(&first));
    };
    assert!(matches!(first.position, ReplyPosition::Committed(_)));
    assert_eq!(first.body.manifest.targets.len(), 2);
    assert_eq!(
        fs::read(fixture.root.join("dist/stage/release/manifest.json"))
            .expect("manifest pointer is regular and readable"),
        wire::encode(&first.body.manifest).expect("reply manifest encodes"),
    );

    let repeated = fixture.invoke(&["--json", "release", "seal"]);
    assert!(
        repeated.status.success(),
        "repeated seal failed: {}",
        spoken(&repeated)
    );
    let Outcome::Reply(repeated): Outcome<SealedManifest> =
        wire::decode(&repeated.stdout).expect("repeated seal reply decodes")
    else {
        panic!("repeated seal did not reply: {}", spoken(&repeated));
    };
    assert_eq!(repeated.body, first.body);

    let snapshot = journal::JournalReader::open(
        fixture
            .root
            .join(".git/.spice-rs/data/spicejournal.sqlite3"),
    )
    .expect("fixture journal opens")
    .fact_snapshot()
    .expect("fixture facts read");
    let sealed = snapshot
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<wire::body::ProofSealed>())
        .collect::<Vec<_>>();
    assert_eq!(sealed.len(), 1);
    assert_eq!(sealed[0].manifest, first.body.digest);
    assert_eq!(sealed[0].commit, fixture.commit);
    assert_eq!(sealed[0].tree, fixture.tree);

    resident.stop(&fixture);
}

#[test]
fn release_seal_refuses_a_missing_receipt_before_manifest_publication() {
    let fixture = Fixture::new();
    fs::remove_file(
        fixture
            .root
            .join(".git/.spice-rs/proof")
            .join(fixture.source.as_str())
            .join(ReleaseTarget::X86_64UnknownLinuxMusl.as_str())
            .join("receipt.json"),
    )
    .expect("remove the exact Linux receipt");

    let output = fixture.invoke(&["--json", "release", "seal"]);

    assert_eq!(output.status.code(), Some(2), "{}", spoken(&output));
    assert!(!fixture.root.join("dist/stage/release").exists());
}

#[test]
fn release_seal_refuses_a_moved_head_without_publishing_a_manifest_pointer() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("later.txt"), "later source\n").expect("later source");
    git_repository::stage(&fixture.root, &["later.txt"]);
    git_repository::commit(&fixture.root, "later fixture");

    let output = fixture.invoke(&["--json", "release", "seal"]);

    assert_refusal(&output, "release::seal-source-mismatch");
    assert!(!fixture.root.join("dist/stage/release").exists());
}

#[test]
fn release_seal_refuses_an_untracked_non_manifest_stage_file() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("dist/stage/unexpected.txt"),
        "unexpected stage file\n",
    )
    .expect("write unexpected stage file");

    let output = fixture.invoke(&["--json", "release", "seal"]);

    assert_refusal(&output, "release::seal-source-refused");
    assert!(!fixture.root.join("dist/stage/release").exists());
}

#[test]
fn release_seal_refuses_a_moved_app_and_preserves_the_existing_pointer() {
    let fixture = Fixture::new();
    let pointer = fixture.root.join("dist/stage/release/manifest.json");
    fs::create_dir_all(pointer.parent().expect("pointer has a parent")).expect("release directory");
    let existing = b"existing manifest pointer\n";
    fs::write(&pointer, existing).expect("existing manifest pointer");
    stage_app_named(&fixture.root, "moved app generation");

    let output = fixture.invoke(&["--json", "release", "seal"]);

    assert_refusal(&output, "release::seal-app-mismatch");
    assert_eq!(fs::read(pointer).expect("existing pointer reads"), existing);
}

struct Fixture {
    root: PathBuf,
    commit: String,
    tree: String,
    source: Digest,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-seal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture root");
        git_repository::initialize(&root);
        fs::write(
            root.join(".gitignore"),
            "/dist/stage/objects/\n/dist/stage/targets/\n/dist/stage-app/\n",
        )
        .expect("fixture ignores only generated release output");
        fs::write(root.join("source.txt"), "source\n").expect("fixture source");
        git_repository::stage(&root, &[".gitignore", "source.txt"]);
        let commit = git_repository::commit(&root, "fixture");
        let tree = git_repository::write_tree(&root);
        let app = stage_app(&root);
        let source = stage_proofs(&root, &commit, &app);
        Self {
            root,
            commit,
            tree,
            source,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .output()
            .expect("fixture command launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture root is removed");
    }
}

struct Resident(Option<Child>);

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args([
                "--json",
                "up",
                "--foreground",
                "--port",
                "0",
                spice::install::TEST_APP_STAGE_ARGUMENT,
            ])
            .current_dir(&fixture.root)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("fixture resident starts");
        let mut resident = Self(Some(child));
        let discovery = fixture.root.join(".git/.spice-rs/serve");
        let deadline = Instant::now() + READY;
        while !discovery.is_file() {
            if resident
                .0
                .as_mut()
                .expect("resident is present")
                .try_wait()
                .expect("resident status reads")
                .is_some()
            {
                let output = resident
                    .0
                    .take()
                    .expect("resident is present")
                    .wait_with_output()
                    .expect("resident output reads");
                panic!("resident exited before readiness: {}", spoken(&output));
            }
            assert!(Instant::now() < deadline, "resident readiness timed out");
            thread::sleep(Duration::from_millis(10));
        }
        resident
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .0
            .take()
            .expect("resident is present")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn stage_app(root: &Path) -> Digest {
    stage_app_named(root, "fixture app generation")
}

fn stage_app_named(root: &Path, marker: &str) -> Digest {
    let identity = Digest::compute(Domain::DistArtifact, marker.as_bytes());
    let generation = root
        .join(serve::APP_STAGE)
        .join(serve::APP_STAGE_GENERATIONS)
        .join(identity.as_str());
    fs::create_dir_all(&generation).expect("app generation directory");
    for asset in serve::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("{marker} {}\n", asset.file()),
        )
        .expect("app asset");
    }
    fs::write(
        root.join(serve::APP_STAGE).join(serve::APP_STAGE_CURRENT),
        format!("{identity}\n"),
    )
    .expect("app pointer");
    identity
}

fn stage_proofs(root: &Path, commit: &str, app: &Digest) -> Digest {
    let source = Digest::compute(Domain::DistSourceCommit, commit.as_bytes());
    let pins = BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: proof::DEFAULT_IMAGE.to_owned(),
    };
    let app_inventory = serve::APP
        .assets()
        .iter()
        .map(|asset| {
            let bytes = fs::read(
                root.join(serve::APP_STAGE)
                    .join(serve::APP_STAGE_GENERATIONS)
                    .join(app.as_str())
                    .join(asset.file()),
            )
            .expect("staged app asset reads");
            InventoryEntry {
                path: format!("app/{}", asset.file()),
                mode: 0o644,
                bytes: bytes.len() as u64,
                digest: Digest::compute(Domain::DistArtifact, &bytes),
            }
        })
        .collect::<Vec<_>>();
    for target in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ] {
        let native = Digest::compute(Domain::DistArtifact, target.as_str().as_bytes());
        let mut inventory = app_inventory.clone();
        inventory.push(InventoryEntry {
            path: "bin/spice".to_owned(),
            mode: 0o755,
            bytes: target.as_str().len() as u64,
            digest: native.clone(),
        });
        let symbols = (target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
            let metadata = MacSymbolMetadata {
                path: "symbols/spice.dSYM.json".to_owned(),
                mode: 0o644,
                bytes: 14,
                digest: Digest::compute(Domain::DistArtifact, b"symbol fixture"),
                uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
                executable: native.clone(),
            };
            inventory.push(InventoryEntry {
                path: metadata.path.clone(),
                mode: metadata.mode,
                bytes: metadata.bytes,
                digest: metadata.digest.clone(),
            });
            metadata
        });
        inventory.sort_by(|left, right| left.path.cmp(&right.path));
        let payload = TargetPayload {
            version: TARGET_PAYLOAD_VERSION,
            target,
            source: source.clone(),
            app: app.clone(),
            native,
            symbols,
            pins_digest: Digest::compute(
                Domain::DistPins,
                &wire::encode(&pins).expect("pins encode"),
            ),
            inventory_digest: Digest::compute(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            pins: pins.clone(),
            inventory,
        };
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let payload_digest = Digest::compute(Domain::DistTargetPayload, &payload_bytes);
        let environment = match target {
            ReleaseTarget::Aarch64AppleDarwin => ProofEnvironment::native(Digest::compute(
                Domain::ProofNativeRunner,
                b"fixture native runner",
            )),
            ReleaseTarget::X86_64UnknownLinuxMusl => {
                ProofEnvironment::oci(proof::DEFAULT_IMAGE).expect("default image is canonical")
            }
        };
        let receipt = ProofReceipt {
            version: PROOF_RECEIPT_VERSION,
            target,
            payload: payload_digest,
            battery: proof::Battery::current().digest().clone(),
            replay: Digest::compute(Domain::ProofReplayV3, target.as_str().as_bytes()),
            environment,
            standing: ProofStanding::Clean,
        };
        let target_root = root.join("dist/stage/targets").join(target.as_str());
        fs::create_dir_all(&target_root).expect("target stage directory");
        fs::write(target_root.join("payload.json"), payload_bytes).expect("target payload");
        let proof_root = root
            .join(".git/.spice-rs/proof")
            .join(source.as_str())
            .join(target.as_str());
        fs::create_dir_all(&proof_root).expect("proof directory");
        fs::write(
            proof_root.join("receipt.json"),
            wire::encode(&receipt).expect("receipt encodes"),
        )
        .expect("proof receipt");
    }
    source
}

fn spoken(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(2), "{}", spoken(output));
    let Outcome::Refusal(refusal): Outcome<SealedManifest> =
        wire::decode(&output.stderr).expect("refusal decodes")
    else {
        panic!("command did not refuse: {}", spoken(output));
    };
    assert_eq!(refusal.reason(), reason);
}
