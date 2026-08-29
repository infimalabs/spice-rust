use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use artifact::{AddRequest, ArtifactAnswer, ArtifactContent, Retention, Store};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use wire::intent::TaskAnswer;
use wire::{
    Actor, CanonicalUuid, JournalBackup, JournalRestore, Outcome, Producer, TaskHandle, TaskStamp,
};

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "task-artifact-cli");
        fs::create_dir(root.join(".git")).expect("create repository fixture");
        staged_app::seed(&root);
        fs::write(root.join("spice-rust.toml"), "[tasks]\nstems = [\"rim\"]\n")
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
            // The suite exercises operator-context commands; a lane identity
            // inherited from an agent shell would flip discovery into lane
            // context and miss the fixture resident.
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
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
            .expect("start resident");
        let mut resident = Self(Some(child));
        let canvas = fixture.root.join(".git/.spice-rs/canvas");
        let watched = canvas.parent().expect("canvas has runtime parent");
        fs::create_dir_all(watched).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.0.as_mut().expect("resident child"),
            watched,
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
        "--origin".to_owned(),
        "fixture:task-artifact-cli".to_owned(),
    ]);
    let task = task_reply.body.task;
    let text = fixture.source("evidence.md", b"observed proof\n");
    let binary = fixture.source("capture.png", b"\x89PNG\r\n\x1a\n");

    let (first, second) = assert_artifact_surface(&fixture, &task, &text, &binary);
    // Shape CASE 146 / INVARIANTS ID-01 reaches the sidecar store too: a
    // project edit changes the current handle without moving artifact identity.
    let rehomed = fixture
        .reply::<TaskAnswer>(&[
            "task".to_owned(),
            "modify".to_owned(),
            task.to_string(),
            "--project".to_owned(),
            "rim.evidence".to_owned(),
        ])
        .body
        .task;
    assert_ne!(rehomed, task);
    assert_eq!(rehomed.born(), task.born());
    let ArtifactAnswer::Listed { artifacts, .. } = fixture
        .reply::<ArtifactAnswer>(&[
            "task".to_owned(),
            "artifact".to_owned(),
            "list".to_owned(),
            rehomed.to_string(),
        ])
        .body
    else {
        panic!("artifact list returned another answer after rehome")
    };
    assert_eq!(artifacts, [first.clone(), second.clone()]);
    assert_invalid_task_refusals(&fixture);
    resident.stop(&fixture);
    assert_backup_restore(&fixture, &rehomed, first, second);
}

#[test]
fn task_add_from_ack_origin_publishes_steering_attachments_as_artifacts() {
    const KEY: &str = "1kJK6N3R";
    let fixture = Fixture::new();
    let media = seed_steering_attachment(&fixture, KEY);
    let resident = Resident::start(&fixture);
    let task = fixture
        .reply::<TaskAnswer>(&[
            "task".to_owned(),
            "add".to_owned(),
            "Inspect captured steering evidence".to_owned(),
            "--project".to_owned(),
            "rim.artifact".to_owned(),
            "--origin".to_owned(),
            format!("ack:{KEY}"),
        ])
        .body
        .task;

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
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, "A1");
    assert_eq!(artifacts[0].name, media.name);
    assert_eq!(artifacts[0].media_type.as_str(), media.content_type);

    let ArtifactAnswer::Shown { content, .. } = fixture
        .reply::<ArtifactAnswer>(&[
            "task".to_owned(),
            "artifact".to_owned(),
            "show".to_owned(),
            task.to_string(),
            "A1".to_owned(),
        ])
        .body
    else {
        panic!("artifact show returned another answer")
    };
    let ArtifactContent::Binary { path } = content else {
        panic!("steering image returned text content")
    };
    assert_eq!(
        fs::read(path).expect("read attached steering artifact"),
        b"\x89PNG\r\n\x1a\n"
    );

    resident.stop(&fixture);
}

#[test]
fn task_artifact_refuses_malformed_mount_registry_before_state_access() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("spice-rust.toml"),
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
    assert_eq!(refusal.reason(), "cli::mount-registry-refused");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::expected),
        Some("one valid tracked command mount registry")
    );
    let observed = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::observed)
        .expect("configuration diagnostic");
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
            "--origin".to_owned(),
            "fixture:task-artifact-cli-opaque".to_owned(),
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
    assert_eq!(refusal.reason(), "artifact::operation-refused");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
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
    let runtime = fixture.root.join(".git/.spice-rs");
    let authority = runtime.join("data/spicejournal.sqlite3");
    let store = Store::new(
        runtime.join("artifacts/tasks"),
        runtime.join("locks/task-artifacts.lock"),
    );
    let unknown = "12345678".parse::<TaskStamp>().expect("unknown task stamp");
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
    assert_eq!(refusal.reason(), "restore::artifact-task-unknown");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some(unknown.as_str())
    );
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

fn seed_steering_attachment(fixture: &Fixture, key: &str) -> wire::Media {
    let data = "iVBORw0KGgo=";
    let media =
        wire::Media::from_base64("operator-capture.png", "image/png", data).expect("media admits");
    let runtime = fixture.root.join(".git/.spice-rs");
    let journal_path = runtime.join("data/spicejournal.sqlite3");
    fs::create_dir_all(journal_path.parent().expect("journal path has parent"))
        .expect("create journal parent");
    let mut journal = journal::Journal::open(&journal_path).expect("fixture journal opens");
    let origin: wire::Origin = "fixture:steering-attachment"
        .parse()
        .expect("fixture origin");
    let at: wire::Instant = "2030-01-01T00:00:00.000Z"
        .parse()
        .expect("fixture publication instant");
    let expires: wire::Instant = "2030-01-02T00:00:00.000Z"
        .parse()
        .expect("fixture expiry instant");
    let body = wire::body::SteerSent {
        directive: uuid(1),
        fanout: None,
        key: key.to_owned(),
        lane: uuid(2),
        text: "inspect this operator capture".to_owned(),
        control: None,
        attachments: vec![media.clone()],
        expires,
    };
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("steering fact builds");
    write.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("steering head builds")
    });
    journal.apply(write).expect("steering attachment commits");
    media
}

fn uuid(tail: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail:02x}")
        .parse()
        .expect("fixture UUID")
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
    assert_eq!(refusal.reason(), "artifact::task-invalid");

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
    assert_eq!(refusal.reason(), "artifact::task-unknown");
}

fn assert_backup_restore(
    fixture: &Fixture,
    task: &TaskHandle,
    first: artifact::Artifact,
    second: artifact::Artifact,
) {
    fixture.reply::<JournalBackup>(&["backup".to_owned(), fixture.bundle.display().to_string()]);
    let born = task
        .born()
        .expect("fixture handle carries its inception stamp");
    assert!(
        fixture
            .bundle
            .join(format!(
                "artifacts/tasks/{born}/objects/{}/payload",
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
