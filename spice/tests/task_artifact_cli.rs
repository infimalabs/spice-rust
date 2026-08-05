use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use artifact::{AddRequest, ArtifactAnswer, ArtifactContent, Retention, Store};
use serde::de::DeserializeOwned;
use wire::intent::TaskAnswer;
use wire::{JournalBackup, JournalRestore, Outcome, TaskHandle};

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-task-artifact-cli-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".git")).expect("create repository fixture");
        fs::write(root.join("spice.toml"), "[tasks]\nstems = [\"rim\"]\n")
            .expect("write task project policy");
        let bundle = root.join("artifact-backup");
        Self { root, bundle }
    }

    fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let sources = self.root.join("sources");
        fs::create_dir_all(&sources).expect("create source directory");
        let path = sources.join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }

    fn invoke(&self, arguments: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .arg("--json")
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("launch spice")
    }

    fn reply<B: DeserializeOwned>(&self, arguments: &[String]) -> wire::Reply<B> {
        let output = self.invoke(arguments);
        assert!(
            output.status.success(),
            "command {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let Outcome::Reply(reply) = wire::decode::<Outcome<B>>(&output.stdout)
            .unwrap_or_else(|error| panic!("decode {arguments:?}: {error}"))
        else {
            panic!("command {arguments:?} did not reply")
        };
        reply
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Resident(Option<Child>);

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start resident");
        let mut resident = Self(Some(child));
        let canvas = fixture.root.join(".git/.spice/canvas");
        match readiness::wait_until_ready(
            resident.0.as_mut().expect("resident child"),
            &fixture.root.join(".git"),
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("resident exited before readiness"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let output = fixture.invoke(&["down".to_owned()]);
        assert!(
            output.status.success(),
            "resident down failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = self
            .0
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            String::from_utf8_lossy(&output.stderr)
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

#[test]
fn task_artifact_cli_and_backup_restore_preserve_provenance() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);
    let task_reply = fixture.reply::<TaskAnswer>(&[
        "task".to_owned(),
        "add".to_owned(),
        "Artifact evidence".to_owned(),
        "--project".to_owned(),
        "rim.artifact".to_owned(),
    ]);
    let task = task_reply.body.task;
    let text = fixture.source("evidence.md", b"observed proof\n");
    let binary = fixture.source("capture.png", b"\x89PNG\r\n\x1a\n");

    let (first, second) = assert_artifact_surface(&fixture, &task, &text, &binary);
    assert_invalid_task_refusals(&fixture);
    resident.stop(&fixture);
    assert_backup_restore(&fixture, &task, first, second);
}

#[test]
fn task_artifact_refuses_malformed_project_policy_before_state_access() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("spice.toml"),
        "[tasks]\nstems = [\"rim\"]\nhidden_stems = [\"sandbox\", \"bad-stem\"]\n",
    )
    .expect("write malformed hidden project policy");

    let output = fixture.invoke(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "list".to_owned(),
        "UNKNOWN-12345678".to_owned(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    let Outcome::<ArtifactAnswer>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("decode project policy refusal")
    else {
        panic!("malformed project policy did not refuse")
    };
    assert_eq!(refusal.reason, "artifact::configuration-refused");
    assert_eq!(
        refusal.expected.as_deref(),
        Some("one admitted tracked task-project policy and machine-local configuration")
    );
    let observed = refusal.observed.expect("configuration diagnostic");
    assert!(observed.contains("tasks.hidden_stems[1]"), "{observed}");
    assert!(observed.contains("bad-stem"), "{observed}");
}

#[test]
fn task_artifact_refuses_opaque_binary_without_panicking_or_publishing() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);
    let task = fixture
        .reply::<TaskAnswer>(&[
            "task".to_owned(),
            "add".to_owned(),
            "Opaque artifact evidence".to_owned(),
            "--project".to_owned(),
            "rim.artifact".to_owned(),
        ])
        .body
        .task;
    let opaque = fixture.source("opaque", b"\xff\xfe\xfd");
    let output = fixture.invoke(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "add".to_owned(),
        task.to_string(),
        opaque.display().to_string(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    let Outcome::<ArtifactAnswer>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("decode opaque binary refusal")
    else {
        panic!("opaque binary did not refuse")
    };
    assert_eq!(refusal.reason, "artifact::operation-refused");
    assert!(
        refusal
            .observed
            .as_deref()
            .is_some_and(|detail| detail.contains("pass --type")),
        "{refusal:?}"
    );
    let ArtifactAnswer::Listed { artifacts, .. } = fixture
        .reply::<ArtifactAnswer>(&[
            "task".to_owned(),
            "artifact".to_owned(),
            "list".to_owned(),
            task.to_string(),
        ])
        .body
    else {
        panic!("artifact list returned another answer")
    };
    assert!(artifacts.is_empty(), "refused input published a citation");
    resident.stop(&fixture);
}

#[test]
fn restore_refuses_an_internally_valid_artifact_for_an_unknown_task_inertly() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);
    resident.stop(&fixture);
    let runtime = fixture.root.join(".git/.spice");
    let authority = runtime.join("data/spicejournal.sqlite3");
    let store = Store::new(
        runtime.join("artifacts/tasks"),
        runtime.join("locks/task-artifacts.lock"),
    );
    let unknown = "UNKNOWN-12345678"
        .parse::<TaskHandle>()
        .expect("unknown task handle");
    let source = fixture.source("unknown.txt", b"internally valid unknown task artifact\n");
    store
        .add(AddRequest {
            task: &unknown,
            source: &source,
            name: None,
            media_type: None,
            retention: Retention::Permanent,
            created_at: "2030-01-01T00:00:00.000Z"
                .parse()
                .expect("artifact instant"),
        })
        .expect("seed internally valid unknown-task artifact");
    let backup = fixture
        .reply::<JournalBackup>(&["backup".to_owned(), fixture.bundle.display().to_string()]);
    assert!(backup.body.verification.members > 1);
    let authority_before = fs::read(&authority).expect("read authority before refusal");
    let artifacts_before = store.capture().expect("capture artifacts before refusal");

    let output = fixture.invoke(&["restore".to_owned(), fixture.bundle.display().to_string()]);
    assert_eq!(output.status.code(), Some(2));
    let Outcome::<JournalRestore>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("decode unknown artifact task refusal")
    else {
        panic!("unknown artifact task did not refuse")
    };
    assert_eq!(refusal.reason, "restore::artifact-task-unknown");
    assert_eq!(refusal.observed.as_deref(), Some(unknown.as_str()));
    assert_eq!(
        fs::read(&authority).expect("read authority after refusal"),
        authority_before,
        "unknown artifact task refusal changed authority"
    );
    assert_eq!(
        store.capture().expect("capture artifacts after refusal"),
        artifacts_before,
        "unknown artifact task refusal changed artifact authority"
    );
}

fn assert_artifact_surface(
    fixture: &Fixture,
    task: &TaskHandle,
    text: &std::path::Path,
    binary: &std::path::Path,
) -> (artifact::Artifact, artifact::Artifact) {
    let first = add(fixture, task, text, "permanent");
    let second = add(fixture, task, binary, "prunable");
    assert_eq!(first.id, "A1");
    assert_eq!(second.id, "A2");
    assert_ne!(first.digest, second.digest);

    let listed = fixture.reply::<ArtifactAnswer>(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "list".to_owned(),
        task.to_string(),
    ]);
    let ArtifactAnswer::Listed { artifacts, .. } = listed.body else {
        panic!("artifact list returned another answer")
    };
    assert_eq!(artifacts, vec![first.clone(), second.clone()]);

    let prune = fixture.reply::<ArtifactAnswer>(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "prune".to_owned(),
        "--older-than".to_owned(),
        "30d".to_owned(),
    ]);
    let ArtifactAnswer::Pruned { plan, applied } = prune.body else {
        panic!("artifact prune returned another answer")
    };
    assert!(!applied);
    assert_eq!(plan.older_than_seconds, Some(30 * 24 * 60 * 60));
    assert!(
        plan.operations.is_empty(),
        "pending task must not be pruned"
    );

    let shown = fixture.reply::<ArtifactAnswer>(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "show".to_owned(),
        task.to_string(),
        "A1".to_owned(),
    ]);
    let ArtifactAnswer::Shown { content, .. } = shown.body else {
        panic!("artifact show returned another answer")
    };
    assert_eq!(
        content,
        ArtifactContent::Text {
            text: "observed proof\n".to_owned()
        }
    );

    (first, second)
}

fn assert_invalid_task_refusals(fixture: &Fixture) {
    let invalid = fixture.invoke(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "list".to_owned(),
        "not-a-task".to_owned(),
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    let Outcome::<ArtifactAnswer>::Refusal(refusal) =
        wire::decode(&invalid.stderr).expect("decode invalid task refusal")
    else {
        panic!("invalid task did not refuse")
    };
    assert_eq!(refusal.reason, "artifact::task-invalid");

    let unknown = fixture.invoke(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "list".to_owned(),
        "UNKNOWN-12345678".to_owned(),
    ]);
    assert_eq!(unknown.status.code(), Some(2));
    let Outcome::<ArtifactAnswer>::Refusal(refusal) =
        wire::decode(&unknown.stderr).expect("decode unknown task refusal")
    else {
        panic!("unknown task did not refuse")
    };
    assert_eq!(refusal.reason, "artifact::task-unknown");
}

fn assert_backup_restore(
    fixture: &Fixture,
    task: &TaskHandle,
    first: artifact::Artifact,
    second: artifact::Artifact,
) {
    fixture.reply::<JournalBackup>(&["backup".to_owned(), fixture.bundle.display().to_string()]);
    assert!(
        fixture
            .bundle
            .join(format!(
                "artifacts/tasks/{task}/objects/{}/payload",
                first.digest
            ))
            .is_file(),
        "backup omitted the cited artifact payload"
    );

    let later = fixture.source("later.txt", b"not in backup");
    let third = add(fixture, task, &later, "permanent");
    assert_eq!(third.id, "A3");
    let preview = fixture
        .reply::<JournalRestore>(&["restore".to_owned(), fixture.bundle.display().to_string()]);
    assert!(!preview.body.applied);
    let apply = format!("--apply={}", preview.body.plan);
    let restored = fixture.reply::<JournalRestore>(&[
        "restore".to_owned(),
        fixture.bundle.display().to_string(),
        apply,
    ]);
    assert!(restored.body.applied);

    let listed = fixture.reply::<ArtifactAnswer>(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "list".to_owned(),
        task.to_string(),
    ]);
    let ArtifactAnswer::Listed { artifacts, .. } = listed.body else {
        panic!("restored artifact list returned another answer")
    };
    assert_eq!(artifacts, vec![first, second]);
}

fn add(
    fixture: &Fixture,
    task: &TaskHandle,
    source: &std::path::Path,
    retention: &str,
) -> artifact::Artifact {
    let reply = fixture.reply::<ArtifactAnswer>(&[
        "task".to_owned(),
        "artifact".to_owned(),
        "add".to_owned(),
        task.to_string(),
        source.display().to_string(),
        "--retention".to_owned(),
        retention.to_owned(),
    ]);
    let ArtifactAnswer::Added { artifact, .. } = reply.body else {
        panic!("artifact add returned another answer")
    };
    artifact
}
