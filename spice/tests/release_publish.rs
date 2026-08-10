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
    notes: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-release-publish-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture root");
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
        git(&root, &["add", "dist/stage-app"]);
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
        Self {
            root,
            journal,
            canvas,
            manifest,
            notes,
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

    // Keep the lifecycle phases explicit so a failure identifies whether
    // preview, application, recovery, or durable replay broke its contract.
    let (notes, plan) = preview_publication(&fixture);
    let (apply, first) = apply_publication(&fixture, &notes, &plan);
    recover_publication(&fixture, &notes);
    replay_and_stop(&fixture, &notes, &apply, resident);
    replay_after_restart(&fixture, &notes, &apply, &first);
}

fn preview_publication(fixture: &Fixture) -> (String, ReleasePlan) {
    let notes = fixture
        .notes
        .to_str()
        .expect("notes path is UTF-8")
        .to_owned();
    let text = fixture.invoke(&["release", "publish", "--notes-file", &notes]);
    assert!(
        text.status.success(),
        "text preview failed: {}",
        spoken(&text)
    );
    let text_plan: ReleasePlan = wire::decode(&text.stdout).expect("text body is a release plan");
    assert_eq!(text_plan.manifest, fixture.manifest);
    assert_eq!(text_plan.targets.len(), 2);
    assert_eq!(text_plan.action, "publish");
    assert_eq!(text_plan.requested_version.as_str(), "0.0.0");
    assert_eq!(text_plan.source_commit.len(), 40);
    assert_eq!(text_plan.release_commit.len(), 40);
    assert_eq!(text_plan.package_object.len(), 40);
    assert!(text_plan.observations.clean);
    assert!(text_plan.observations.bump_claimed);
    assert!(text_plan.observations.credentials_valid);
    assert_eq!(text_plan.observations.unrecorded_local_commits, 0);
    assert!(text_plan.observations.registry.is_none());
    assert_eq!(text_plan.operations, publish_operations());
    let draft = fixture.invoke(&["--json", "release", "notes"]);
    let Outcome::Reply(draft) =
        wire::decode::<Outcome<wire::ReleaseNotes>>(&draft.stdout).expect("draft is typed")
    else {
        panic!("release notes did not reply")
    };
    let candidate = fs::read_to_string(&fixture.notes).expect("curated notes read");
    let expected_notes =
        dist::curated_release_notes(&candidate, &draft.body).expect("notes are curated");
    assert_eq!(text_plan.notes, expected_notes.identity);
    assert_eq!(text_plan.notes_document, expected_notes.document);
    assert!(
        remote_refs(fixture).is_empty(),
        "preview changed remote refs"
    );

    let json = fixture.invoke(&["--json", "release", "publish", "--notes-file", &notes]);
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
    (notes, text_plan)
}

fn apply_publication(
    fixture: &Fixture,
    notes: &str,
    plan: &ReleasePlan,
) -> (String, (ReplyPosition, RecognitionAnswer)) {
    let apply = format!("--apply={}", plan.key);
    let first = fixture.invoke(&[
        "--json",
        "release",
        "publish",
        "--notes-file",
        notes,
        &apply,
    ]);
    assert!(
        first.status.success(),
        "first apply failed: {}",
        spoken(&first)
    );
    let first = recognition(&first);
    assert!(first.1.fresh);
    assert_eq!(remote_refs(fixture), published_refs());
    assert_eq!(
        remote_object_type(fixture, "refs/spice/packages/v0.0.0"),
        "tree"
    );
    assert_eq!(
        remote_object(fixture, "refs/spice/packages/v0.0.0"),
        plan.package_object
    );
    let operation_journal = fixture
        .journal
        .parent()
        .expect("journal data directory")
        .join("release-operations")
        .join(format!("{}.json", plan.key));
    let operation_journal: serde_json::Value = serde_json::from_slice(
        &fs::read(operation_journal).expect("external operation journal exists"),
    )
    .expect("external operation journal is typed JSON");
    assert_eq!(
        operation_journal["accepted"]
            .as_array()
            .expect("accepted operation rows")
            .len(),
        plan.operations.len()
    );
    assert_eq!(
        operation_journal["receipts"]
            .as_array()
            .expect("operation receipt rows")
            .len(),
        plan.operations.len()
    );
    (apply, first)
}

fn recover_publication(fixture: &Fixture, notes: &str) {
    let recovery = fixture.invoke(&[
        "--json",
        "release",
        "publish",
        "--notes-file",
        notes,
        "--action",
        "tag-and-announce",
    ]);
    assert!(
        recovery.status.success(),
        "recovery preview failed: {}",
        spoken(&recovery)
    );
    let Outcome::Reply(recovery) =
        wire::decode::<Outcome<ReleasePlan>>(&recovery.stdout).expect("recovery preview is typed")
    else {
        panic!("recovery preview did not reply")
    };
    assert_eq!(recovery.body.action, "tag-and-announce");
    assert_eq!(
        recovery
            .body
            .observations
            .registry
            .as_ref()
            .expect("recovery binds registry evidence")
            .release_commit,
        recovery.body.release_commit
    );
    assert_eq!(recovery.body.operations, recovery_operations());
    let recovery_apply = format!("--apply={}", recovery.body.key);
    let recovered = fixture.invoke(&[
        "--json",
        "release",
        "publish",
        "--notes-file",
        notes,
        "--action",
        "tag-and-announce",
        &recovery_apply,
    ]);
    assert!(
        recovered.status.success(),
        "recovery apply failed: {}",
        spoken(&recovered)
    );
    assert!(!recognition(&recovered).1.fresh);
}

fn replay_and_stop(fixture: &Fixture, notes: &str, apply: &str, resident: Resident) {
    let replay = fixture.invoke(&["release", "publish", "--notes-file", notes, apply]);
    assert!(
        replay.status.success(),
        "text replay failed: {}",
        spoken(&replay)
    );
    let replay_answer: RecognitionAnswer =
        wire::decode(&replay.stdout).expect("text replay body is typed");
    assert!(!replay_answer.fresh);
    let snapshot = facts(fixture);
    assert_eq!(kind_count(&snapshot, FactKind::ReleasePublished), 1);
    resident.stop(fixture);
    assert_eq!(
        journal::Journal::open(&fixture.journal)
            .expect("journal opens")
            .verify()
            .expect("authority verifies")
            .receipts,
        1
    );
}

fn replay_after_restart(
    fixture: &Fixture,
    notes: &str,
    apply: &str,
    first: &(ReplyPosition, RecognitionAnswer),
) {
    let restarted = Resident::start(fixture);
    let after_restart =
        fixture.invoke(&["--json", "release", "publish", "--notes-file", notes, apply]);
    assert!(
        after_restart.status.success(),
        "restart replay failed: {}",
        spoken(&after_restart)
    );
    let after_restart = recognition(&after_restart);
    assert!(!after_restart.1.fresh);
    assert_eq!(after_restart.0, first.0);
    assert_eq!(kind_count(&facts(fixture), FactKind::ReleasePublished), 1);
    restarted.stop(fixture);
}

#[test]
fn missing_evidence_and_a_stale_apply_key_refuse_without_publication() {
    let fixture = Fixture::new("refusal");
    let resident = Resident::start(&fixture);

    let stale = Digest::compute(Domain::DistReleasePlan, b"stale plan");
    let apply = format!("--apply={stale}");
    let notes = fixture.notes.to_str().expect("notes path is UTF-8");
    let stale = fixture.invoke(&[
        "--json",
        "release",
        "publish",
        "--notes-file",
        notes,
        &apply,
    ]);
    assert_eq!(stale.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<RecognitionAnswer>>(&stale.stderr).expect("stale refusal is typed")
    else {
        panic!("stale apply did not refuse")
    };
    assert_eq!(refusal.reason(), "release::plan-moved");

    fs::remove_file(fixture.receipt_path(1)).expect("remove one detached receipt");

    let missing = fixture.invoke(&["--json", "release", "publish", "--notes-file", notes]);
    assert_eq!(missing.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = wire::decode::<Outcome<ReleasePlan>>(&missing.stderr)
        .expect("missing evidence refusal is typed")
    else {
        panic!("missing evidence did not refuse")
    };
    assert_eq!(refusal.reason(), "release::manifest-refused");
    assert_eq!(kind_count(&facts(&fixture), FactKind::ReleasePublished), 0);
    resident.stop(&fixture);
}

#[test]
fn publication_refuses_every_uncurated_notes_shape_before_resident_work() {
    let fixture = Fixture::new("uncurated");

    let untouched = fixture.invoke(&["--json", "release", "publish"]);
    assert_notes_refusal(&untouched);

    fs::write(
        &fixture.notes,
        format!("## Highlights\n\n{}\n", dist::HIGHLIGHTS_PLACEHOLDER),
    )
    .expect("placeholder notes write");
    let notes = fixture.notes.to_str().expect("notes path is UTF-8");
    let placeholder = fixture.invoke(&["--json", "release", "publish", "--notes-file", notes]);
    assert_notes_refusal(&placeholder);

    fs::write(&fixture.notes, "> draft banner\n\n## Highlights\n")
        .expect("heading-only notes write");
    let heading_only = fixture.invoke(&["--json", "release", "publish", "--notes-file", notes]);
    assert_notes_refusal(&heading_only);
    assert_eq!(kind_count(&facts(&fixture), FactKind::ReleasePublished), 0);
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

fn recovery_operations() -> [&'static str; 4] {
    [
        "resolve-preconditions-and-credentials",
        "await-registry-visibility",
        "create-immutable-tag",
        "announce",
    ]
}

fn published_refs() -> [&'static str; 4] {
    [
        "refs/heads/main",
        "refs/spice/packages/v0.0.0",
        "refs/spice/releases/v0.0.0",
        "refs/tags/v0.0.0",
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

fn recognition(output: &Output) -> (ReplyPosition, RecognitionAnswer) {
    let Outcome::Reply(reply) = wire::decode::<Outcome<RecognitionAnswer>>(&output.stdout)
        .expect("publication answer is typed")
    else {
        panic!("publication did not reply")
    };
    (reply.position, reply.body)
}

fn assert_notes_refusal(output: &Output) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::<ReleasePlan>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("notes refusal is typed")
    else {
        panic!("uncurated notes did not refuse: {}", spoken(output))
    };
    assert_eq!(refusal.reason(), "release::notes-refused");
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

fn remote_object(fixture: &Fixture, reference: &str) -> String {
    let remote = fixture.root.join("remote.git");
    git(
        &fixture.root,
        &[
            "--git-dir",
            remote.to_str().expect("remote path"),
            "rev-parse",
            "--verify",
            reference,
        ],
    )
}

fn remote_object_type(fixture: &Fixture, reference: &str) -> String {
    let remote = fixture.root.join("remote.git");
    git(
        &fixture.root,
        &[
            "--git-dir",
            remote.to_str().expect("remote path"),
            "cat-file",
            "-t",
            reference,
        ],
    )
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
