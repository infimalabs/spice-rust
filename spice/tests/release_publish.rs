#![cfg(all(
    unix,
    any(
        all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux", target_env = "musl")
    )
))]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use wire::body::ProofSealed;
use wire::manifest::{
    BuildPins, InventoryEntry, PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt,
    ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseManifestTarget, ReleaseTarget,
    TargetPayload,
};
use wire::{Actor, Digest, Domain, Instant, Outcome, Producer, ReleasePlan, ReplyPosition};

#[path = "support/staged_app.rs"]
mod staged_app;

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

const RELEASE_VERSION: &str = "0.0.1";
/// The client bound this battery needs for one durable publication.
///
/// Applying a plan publishes objects, signs receipts, and commits the journal
/// before the resident can answer, and every lane on the host contends for the
/// same disk while it happens. The battery states what it needs; `spice::cli`
/// states what the client grants; a client that grants less reports the
/// elapsed bound here rather than an unexplained transport failure.
const PUBLICATION_ANSWER: Duration = Duration::from_secs(20);
#[path = "support/fixture_root.rs"]
mod fixture_root;

struct Fixture {
    root: PathBuf,
    canvas: PathBuf,
    manifest: Digest,
    notes: PathBuf,
    cargo: PathBuf,
    effects: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), &format!("release-publish-{label}"));
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.name", "Spice Test"]);
        git(&root, &["config", "user.email", "spice@example.test"]);
        seed_release_authority(&root);
        let remote = root.join("remote.git");
        git(
            &root,
            &["init", "--bare", remote.to_str().expect("remote path")],
        );
        git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path"),
            ],
        );
        staged_app::seed(&root);
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "commit",
                "-m",
                concat!(
                    "fixture: Seed staged release app\n\n",
                    "Task-Project: fixture.release\nTask-Key: fixture\n",
                ),
            ],
        );
        let notes = root.join("release-notes.md");
        fs::write(
            &notes,
            "## Highlights\n\n- Published one proof-bound Rust release.\n",
        )
        .expect("curated notes write");
        let runtime = root.join(".git/.spice-rs");
        let journal = runtime.join("data/spicejournal.sqlite3");
        let canvas = runtime.join("canvas");
        let manifest = publish_stage(&root);
        seed_proof(&journal, &manifest);
        let tools = root.join("test-tools");
        fs::create_dir_all(&tools).expect("test tool directory");
        let cargo = tools.join("cargo");
        fs::write(
            &cargo,
            concat!(
                "#!/bin/sh\n",
                "effect_log=\"$(dirname \"$0\")/../release-effects.log\"\n",
                "printf '%s\\n' \"$*\" >> \"$effect_log\"\n",
                "case \"$*\" in\n",
                "  'xtask verify'|'xtask stage-target aarch64-apple-darwin'|",
                "'xtask stage-target x86_64-unknown-linux-musl') exit 0 ;;\n",
                "  *) exit 64 ;;\n",
                "esac\n",
            ),
        )
        .expect("test Cargo shim");
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))
            .expect("test Cargo shim mode");
        let effects = root.join("release-effects.log");
        Self {
            root,
            canvas,
            manifest,
            notes,
            cargo,
            effects,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        let path = format!(
            "{}:/usr/local/bin:/usr/bin:/bin",
            self.cargo
                .parent()
                .expect("test Cargo shim has a parent")
                .display()
        );
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .env("PATH", path)
            // The suite exercises operator-context commands; a lane identity
            // inherited from an agent shell would flip discovery into lane
            // context and miss the fixture resident.
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .output()
            .expect("spice release command launches")
    }
}

fn seed_release_authority(root: &Path) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice is one workspace member");
    fs::copy(workspace.join("Cargo.toml"), root.join("Cargo.toml"))
        .expect("copy workspace manifest");
    let manifest = toml::from_str::<toml::Value>(
        &fs::read_to_string(workspace.join("Cargo.toml")).expect("read workspace manifest"),
    )
    .expect("workspace manifest decodes");
    for member in manifest["workspace"]["members"]
        .as_array()
        .expect("workspace members")
    {
        let member = member.as_str().expect("member path");
        fs::create_dir_all(root.join(member)).expect("fixture member directory");
        fs::copy(
            workspace.join(member).join("Cargo.toml"),
            root.join(member).join("Cargo.toml"),
        )
        .expect("copy member manifest");
    }
    fs::copy(
        workspace.join("dist/release-authority.json"),
        root.join("dist/release-authority.json"),
    )
    .expect("copy release authority");
    fs::copy(workspace.join("dist/VERSION"), root.join("dist/VERSION"))
        .expect("copy product version");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.root)
            // The fixture resident serves this root in operator context, never
            // as a seat of the launching agent shell.
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("resident starts");
        let mut resident = Self { child: Some(child) };
        let watched = fixture.canvas.parent().expect("canvas has runtime parent");
        fs::create_dir_all(watched).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || fixture.canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("resident exited before readiness"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .child
            .take()
            .expect("resident child")
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
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn the_client_grants_a_publication_the_bound_this_battery_needs() {
    assert!(
        spice::cli::answer_bound(wire::CommandKind::ReleasePublish) >= PUBLICATION_ANSWER,
        "the client gives up on a publication before this battery finishes one",
    );
}

#[test]
fn publication_preview_is_typed_and_zero_effect() {
    let fixture = Fixture::new("durable");
    let resident = Resident::start(&fixture);

    preview_publication(&fixture);
    resident.stop(&fixture);
}

fn preview_publication(fixture: &Fixture) {
    let notes = fixture
        .notes
        .to_str()
        .expect("notes path is UTF-8")
        .to_owned();
    let json = fixture.invoke(&[
        "--json",
        "release",
        "publish",
        "--version",
        RELEASE_VERSION,
        "--notes-file",
        &notes,
    ]);
    assert!(
        json.status.success(),
        "JSON preview failed: {}",
        spoken(&json)
    );
    let Outcome::Reply(json_plan) =
        wire::decode::<Outcome<ReleasePlan>>(&json.stdout).expect("JSON preview is typed")
    else {
        panic!("JSON preview did not reply")
    };
    assert_eq!(json_plan.position, ReplyPosition::Direct {});
    let plan = json_plan.body;
    assert_eq!(plan.manifest, fixture.manifest);
    assert_eq!(plan.targets.len(), 2);
    assert_eq!(plan.action, "publish");
    assert_eq!(plan.requested_version.as_str(), RELEASE_VERSION);
    assert!(plan.observations.clean);
    assert!(plan.observations.credentials_valid);
    assert!(plan.observations.registry.is_none());
    assert_eq!(plan.operations, publish_operations());
    assert!(!fixture.effects.exists(), "preview invoked a Cargo effect");
    assert!(
        remote_refs(fixture).is_empty(),
        "preview changed remote refs"
    );
}

fn publish_operations() -> [&'static str; 12] {
    [
        "resolve-preconditions-and-credentials",
        "run-correctness-gate",
        "bump-and-verify-version",
        "create-release-commit",
        "rebuild-artifacts",
        "install-and-probe-artifacts",
        "rehearse-upload",
        "publish-source",
        "upload-package",
        "await-registry-visibility",
        "create-immutable-tag",
        "announce",
    ]
}

fn publish_stage(root: &Path) -> Digest {
    let stage = root.join("dist/stage");
    let source = Digest::compute(Domain::DistArtifact, b"release source");
    let embedded = canvas::AppSource::resolve(root)
        .and_then(|source| source.digest())
        .expect("fixture app digests");
    let app = Digest::compute(Domain::DistArtifact, b"release app");
    let current = proof::compiled_target().expect("fixture runs on a release host");
    let native = Digest::compute(
        Domain::DistArtifact,
        &fs::read(env!("CARGO_BIN_EXE_spice")).expect("current spice binary"),
    );
    let pins = BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: format!("registry.example/spice-proof@sha256:{}", "0".repeat(64)),
    };
    let mut targets = Vec::new();
    for target in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ] {
        let target_native = if target == current {
            native.clone()
        } else {
            Digest::compute(Domain::DistArtifact, target.as_str().as_bytes())
        };
        let executable = if target == current {
            fs::read(env!("CARGO_BIN_EXE_spice")).expect("current spice binary")
        } else {
            target.as_str().as_bytes().to_vec()
        };
        let executable_path = stage
            .join("targets")
            .join(target.as_str())
            .join("bin/spice");
        fs::create_dir_all(executable_path.parent().expect("executable parent"))
            .expect("target executable directory");
        fs::write(&executable_path, executable).expect("target executable bytes");
        let inventory = vec![
            inventory("app/app.wasm", 0o644, &app),
            inventory("bin/spice", 0o755, &target_native),
            inventory("embed/app.bin", 0o644, &embedded),
        ];
        let payload = TargetPayload {
            version: 1,
            target,
            source: source.clone(),
            app: app.clone(),
            embedded_app: embedded.clone(),
            native: target_native,
            pins: pins.clone(),
            pins_digest: Digest::compute(
                Domain::DistPins,
                &wire::encode(&pins).expect("pins encode"),
            ),
            inventory_digest: Digest::compute(
                Domain::DistInventory,
                &wire::encode(&inventory).expect("inventory encodes"),
            ),
            inventory,
        };
        let payload_bytes = wire::encode(&payload).expect("payload encodes");
        let payload_digest = Digest::compute(Domain::DistTargetPayload, &payload_bytes);
        let environment = match target {
            ReleaseTarget::Aarch64AppleDarwin => ProofEnvironment::native(Digest::compute(
                Domain::ProofNativeRunner,
                b"native proof runner",
            )),
            ReleaseTarget::X86_64UnknownLinuxMusl => {
                ProofEnvironment::oci(&pins.proof_container).expect("proof image is pinned")
            }
        };
        let replay = Digest::compute(Domain::ProofReplayV3, target.as_str().as_bytes());
        let receipt = ProofReceipt {
            version: PROOF_RECEIPT_VERSION,
            target,
            payload: payload_digest.clone(),
            battery: proof::Battery::current().digest().clone(),
            replay: replay.clone(),
            environment: environment.clone(),
            standing: ProofStanding::Clean,
        };
        let receipt_bytes = wire::encode(&receipt).expect("receipt encodes");
        let receipt_digest = Digest::compute(Domain::ProofReceiptV3, &receipt_bytes);
        write_object(&stage, "payload", &payload_digest, &payload_bytes);
        write_object(&stage, "receipt", &receipt_digest, &receipt_bytes);
        targets.push(ReleaseManifestTarget {
            target,
            payload: payload_digest,
            receipt: receipt_digest,
            battery: receipt.battery,
            replay,
            environment,
            standing: ProofStanding::Clean,
        });
    }
    let manifest = ReleaseManifest {
        version: RELEASE_MANIFEST_VERSION,
        source,
        targets,
    };
    let bytes = wire::encode(&manifest).expect("manifest encodes");
    let digest = Digest::compute(Domain::DistReleaseManifest, &bytes);
    write_object(&stage, "manifest", &digest, &bytes);
    fs::create_dir_all(stage.join("release")).expect("release pointer directory");
    fs::write(stage.join("release/manifest.json"), bytes).expect("manifest pointer");
    digest
}

fn inventory(path: &str, mode: u32, digest: &Digest) -> InventoryEntry {
    InventoryEntry {
        path: path.to_owned(),
        mode,
        bytes: 3,
        digest: digest.clone(),
    }
}

fn write_object(stage: &Path, kind: &str, digest: &Digest, bytes: &[u8]) {
    fs::create_dir_all(stage.join("objects")).expect("object directory");
    fs::write(stage.join(format!("objects/{kind}-{digest}.json")), bytes).expect("release object");
}

fn seed_proof(path: &Path, manifest: &Digest) {
    fs::create_dir_all(path.parent().expect("journal parent")).expect("journal directory");
    let journal = journal::Journal::open(path).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "release:fixture".parse().expect("fixture origin"),
        "2030-04-12T18:44:09.271Z"
            .parse::<Instant>()
            .expect("fixture instant"),
        Producer::ResidentRecognition,
    );
    write
        .fact(
            ProofSealed {
                commit: "fixture-commit".to_owned(),
                tree: "fixture-tree".to_owned(),
                manifest: manifest.clone(),
            },
            FactCause::Root,
        )
        .expect("proof recognition fact");
    assert!(matches!(
        bus.commit(write).expect("proof commit reaches writer"),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().expect("fixture bus stops");
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git launches");
    assert!(output.status.success(), "{}", spoken(&output));
    String::from_utf8(output.stdout)
        .expect("Git stdout is UTF-8")
        .trim()
        .to_owned()
}

fn remote_refs(fixture: &Fixture) -> Vec<String> {
    let remote = fixture.root.join("remote.git");
    git(
        &fixture.root,
        &[
            "--git-dir",
            remote.to_str().expect("remote path"),
            "for-each-ref",
            "--format=%(refname)",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
