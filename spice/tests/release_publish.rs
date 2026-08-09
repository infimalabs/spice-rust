#![cfg(all(
    unix,
    any(
        all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "linux", target_env = "musl")
    )
))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use wire::body::ProofSealed;
use wire::intent::RecognitionAnswer;
use wire::manifest::{
    BuildPins, InventoryEntry, PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt,
    ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseManifestTarget, ReleaseTarget,
    TargetPayload,
};
use wire::{
    Actor, Digest, Domain, FactKind, Instant, Outcome, Producer, ReleasePlan, ReplyPosition,
};

#[path = "support/staged_app.rs"]
mod staged_app;

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

const REFUSAL_EXIT: i32 = 2;
/// The client bound this battery needs for one durable publication.
///
/// Applying a plan publishes objects, signs receipts, and commits the journal
/// before the resident can answer, and every lane on the host contends for the
/// same disk while it happens. The battery states what it needs; `spice::cli`
/// states what the client grants; a client that grants less reports the
/// elapsed bound here rather than an unexplained transport failure.
const PUBLICATION_ANSWER: Duration = Duration::from_secs(20);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    journal: PathBuf,
    canvas: PathBuf,
    manifest: Digest,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-publish-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".git")).expect("fixture Git directory");
        staged_app::seed(&root);
        let runtime = root.join(".git/.spice-rs");
        let journal = runtime.join("data/spicejournal.sqlite3");
        let canvas = runtime.join("canvas");
        let manifest = publish_stage(&root);
        seed_proof(&journal, &manifest);
        Self {
            root,
            journal,
            canvas,
            manifest,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("spice release command launches")
    }

    fn receipt_path(&self, target: usize) -> PathBuf {
        let pointer: ReleaseManifest = wire::decode(
            &fs::read(self.root.join("dist/stage/release/manifest.json"))
                .expect("manifest pointer"),
        )
        .expect("manifest decodes");
        self.root.join(format!(
            "dist/stage/objects/receipt-{}.json",
            pointer.targets[target].receipt
        ))
    }
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
            .args(["--json", "up", "--port", "0"])
            .current_dir(&fixture.root)
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
fn publication_plans_commits_replays_and_survives_restart() {
    let fixture = Fixture::new("durable");
    let resident = Resident::start(&fixture);

    let text = fixture.invoke(&["release", "publish"]);
    assert!(
        text.status.success(),
        "text preview failed: {}",
        spoken(&text)
    );
    let text_plan: ReleasePlan = wire::decode(&text.stdout).expect("text body is a release plan");
    assert_eq!(text_plan.manifest, fixture.manifest);
    assert_eq!(text_plan.targets.len(), 2);

    let json = fixture.invoke(&["--json", "release", "publish"]);
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
    assert_eq!(json_plan.body, text_plan);

    let apply = format!("--apply={}", text_plan.key);
    let first = fixture.invoke(&["--json", "release", "publish", &apply]);
    assert!(
        first.status.success(),
        "first apply failed: {}",
        spoken(&first)
    );
    let first = recognition(&first);
    assert!(first.1.fresh);

    let replay = fixture.invoke(&["release", "publish", &apply]);
    assert!(
        replay.status.success(),
        "text replay failed: {}",
        spoken(&replay)
    );
    let replay_answer: RecognitionAnswer =
        wire::decode(&replay.stdout).expect("text replay body is typed");
    assert!(!replay_answer.fresh);
    let snapshot = facts(&fixture);
    assert_eq!(kind_count(&snapshot, FactKind::ReleasePublished), 1);
    resident.stop(&fixture);
    assert_eq!(
        journal::Journal::open(&fixture.journal)
            .expect("journal opens")
            .verify()
            .expect("authority verifies")
            .receipts,
        1
    );

    let restarted = Resident::start(&fixture);
    let after_restart = fixture.invoke(&["--json", "release", "publish", &apply]);
    assert!(
        after_restart.status.success(),
        "restart replay failed: {}",
        spoken(&after_restart)
    );
    let after_restart = recognition(&after_restart);
    assert!(!after_restart.1.fresh);
    assert_eq!(after_restart.0, first.0);
    assert_eq!(kind_count(&facts(&fixture), FactKind::ReleasePublished), 1);
    restarted.stop(&fixture);
}

#[test]
fn missing_evidence_and_a_stale_apply_key_refuse_without_publication() {
    let fixture = Fixture::new("refusal");
    let resident = Resident::start(&fixture);

    let stale = Digest::compute(Domain::DistReleasePlan, b"stale plan");
    let apply = format!("--apply={stale}");
    let stale = fixture.invoke(&["--json", "release", "publish", &apply]);
    assert_eq!(stale.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<RecognitionAnswer>>(&stale.stderr).expect("stale refusal is typed")
    else {
        panic!("stale apply did not refuse")
    };
    assert_eq!(refusal.reason, "release::plan-moved");

    fs::remove_file(fixture.receipt_path(1)).expect("remove one detached receipt");

    let missing = fixture.invoke(&["--json", "release", "publish"]);
    assert_eq!(missing.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = wire::decode::<Outcome<ReleasePlan>>(&missing.stderr)
        .expect("missing evidence refusal is typed")
    else {
        panic!("missing evidence did not refuse")
    };
    assert_eq!(refusal.reason, "release::manifest-refused");
    assert_eq!(kind_count(&facts(&fixture), FactKind::ReleasePublished), 0);
    resident.stop(&fixture);
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

fn recognition(output: &Output) -> (ReplyPosition, RecognitionAnswer) {
    let Outcome::Reply(reply) = wire::decode::<Outcome<RecognitionAnswer>>(&output.stdout)
        .expect("publication answer is typed")
    else {
        panic!("publication did not reply")
    };
    (reply.position, reply.body)
}

fn facts(fixture: &Fixture) -> journal::FactSnapshot {
    journal::JournalReader::open(&fixture.journal)
        .expect("journal reader opens")
        .fact_snapshot()
        .expect("fact snapshot")
}

fn kind_count(snapshot: &journal::FactSnapshot, kind: FactKind) -> usize {
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == kind)
        .count()
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
