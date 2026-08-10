use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use wire::{Instant, TaskHandle, TaskStamp};

use super::{
    AddRequest, BackupFile, BackupImage, Error, ErrorClass, MediaType, Retention, Store,
    object_dir, payload_path,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    base: PathBuf,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "spice-artifact-test-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).expect("create artifact fixture");
        let store = Store::new(base.join("tasks"), base.join("locks/artifacts.lock"));
        Self { base, store }
    }

    fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let sources = self.base.join("sources");
        fs::create_dir_all(&sources).expect("create source directory");
        let path = sources.join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn task(text: &str) -> TaskStamp {
    let millis = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 1_000_000_000
    });
    TaskStamp::for_millis(millis).expect("fixture task stamp")
}

fn handle(task: &TaskStamp) -> TaskHandle {
    TaskHandle::for_project("task.artifact", task)
}

fn instant(text: &str) -> Instant {
    text.parse().expect("fixture instant")
}

fn add(
    store: &Store,
    task: &TaskStamp,
    source: &Path,
    retention: Retention,
    at: Instant,
) -> super::Artifact {
    store
        .add(AddRequest {
            task,
            source,
            name: None,
            media_type: None,
            retention,
            created_at: at,
        })
        .expect("add artifact")
}

#[test]
fn text_and_binary_artifacts_round_trip_with_stable_metadata() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-a1");
    let text = fixture.source("evidence.md", b"proof\n");
    let binary = fixture.source("image.png", b"\x89PNG\r\n\x1a\n");

    let first = add(
        &fixture.store,
        &task,
        &text,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let second = add(
        &fixture.store,
        &task,
        &binary,
        Retention::Prunable,
        instant("2030-01-02T00:00:00.000Z"),
    );

    assert_eq!(first.id, "A1");
    assert_eq!(first.media_type.as_str(), MediaType::TEXT_MARKDOWN);
    assert_eq!(second.id, "A2");
    assert_eq!(second.media_type.as_str(), MediaType::IMAGE_PNG);
    assert_eq!(
        fixture.store.list(&task).expect("list"),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(
        fixture.store.show(&task, "A1").expect("show text").1,
        b"proof\n"
    );
    assert_eq!(
        fixture.store.show(&task, "A2").expect("show binary").1,
        b"\x89PNG\r\n\x1a\n"
    );
    assert_eq!(
        fixture.store.capture().expect("capture task set").tasks(),
        std::slice::from_ref(&task)
    );
}

#[test]
fn opaque_binary_without_an_admitted_extension_refuses_without_publication() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-m3");
    let source = fixture.source("opaque", b"\xff\xfe\xfd");
    let failure = fixture
        .store
        .add(AddRequest {
            task: &task,
            source: &source,
            name: None,
            media_type: None,
            retention: Retention::Permanent,
            created_at: instant("2030-01-01T00:00:00.000Z"),
        })
        .expect_err("opaque binary requires an explicit media type");

    assert_eq!(failure.class(), ErrorClass::Refusal);
    let detail = failure.to_string();
    assert!(detail.contains("cannot infer a media type"), "{detail}");
    assert!(detail.contains("pass --type"), "{detail}");
    assert!(
        !fixture.store.root().exists(),
        "refused input published an artifact task directory"
    );
}

#[test]
fn concurrent_adds_serialize_without_losing_either_citation() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-b2");
    let first = fixture.source("first.txt", b"first");
    let second = fixture.source("second.txt", b"second");
    let store = Arc::new(fixture.store.clone());
    let handles = [first, second]
        .into_iter()
        .map(|source| {
            let store = Arc::clone(&store);
            let task = task.clone();
            thread::spawn(move || {
                add(
                    &store,
                    &task,
                    &source,
                    Retention::Permanent,
                    instant("2030-01-01T00:00:00.000Z"),
                )
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("writer thread");
    }

    let listed = store.list(&task).expect("list concurrent artifacts");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "A1");
    assert_eq!(listed[1].id, "A2");
    assert_ne!(listed[0].digest, listed[1].digest);
}

#[test]
fn interrupted_pre_manifest_publication_is_invisible_and_recoverable() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-c3");
    let source = fixture.source("proof.txt", b"durable proof");
    let failure = fixture
        .store
        .add_with_hook(
            AddRequest {
                task: &task,
                source: &source,
                name: None,
                media_type: None,
                retention: Retention::Permanent,
                created_at: instant("2030-01-01T00:00:00.000Z"),
            },
            || {
                Err(Error::environment(
                    "fixture interruption",
                    "before manifest",
                ))
            },
        )
        .expect_err("injected interruption");
    assert_eq!(failure.class(), ErrorClass::Environment);
    assert!(
        fixture
            .store
            .list(&task)
            .expect("orphan is invisible")
            .is_empty()
    );

    let artifact = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:01.000Z"),
    );
    assert_eq!(artifact.id, "A1");
    assert_eq!(
        fixture.store.show(&task, "A1").expect("recovered show").1,
        b"durable proof"
    );
}

#[test]
fn duplicate_payload_survives_pruning_of_one_citation() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-d4");
    let source = fixture.source("same.txt", b"same bytes");
    let permanent = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let prunable = add(
        &fixture.store,
        &task,
        &source,
        Retention::Prunable,
        instant("2030-01-01T00:00:01.000Z"),
    );
    assert_eq!(permanent.digest, prunable.digest);

    let statuses = BTreeMap::from([(task.clone(), (handle(&task), "completed".to_owned()))]);
    let plan = fixture
        .store
        .plan_prune(&statuses, None, None)
        .expect("plan prune");
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].artifact_id, "A2");
    assert_eq!(
        fixture.store.list(&task).expect("preview is inert").len(),
        2
    );
    fixture.store.apply_prune(&plan).expect("apply prune");

    assert_eq!(
        fixture.store.list(&task).expect("remaining citation"),
        vec![permanent]
    );
    assert_eq!(
        fixture.store.show(&task, "A1").expect("payload remains").1,
        b"same bytes"
    );
}

#[test]
fn prune_requires_completed_status_and_honors_the_age_bound() {
    let fixture = Fixture::new();
    let completed = task("ARTIFACT-e5");
    let pending = task("ARTIFACT-f6");
    let old = fixture.source("old.txt", b"old");
    let recent = fixture.source("recent.txt", b"recent");
    add(
        &fixture.store,
        &completed,
        &old,
        Retention::Prunable,
        instant("2030-01-01T00:00:00.000Z"),
    );
    add(
        &fixture.store,
        &completed,
        &recent,
        Retention::Prunable,
        instant("2030-01-03T00:00:00.000Z"),
    );
    add(
        &fixture.store,
        &pending,
        &old,
        Retention::Prunable,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let statuses = BTreeMap::from([
        (
            completed.clone(),
            (handle(&completed), "completed".to_owned()),
        ),
        (pending.clone(), (handle(&pending), "pending".to_owned())),
    ]);

    let plan = fixture
        .store
        .plan_prune(
            &statuses,
            Some(instant("2030-01-02T00:00:00.000Z")),
            Some(86_400),
        )
        .expect("bounded plan");
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].incepted, completed);
    assert_eq!(plan.operations[0].handle, handle(&completed));
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.skipped[0].incepted, pending);
    assert_eq!(plan.skipped[0].handle, Some(handle(&pending)));
    assert_eq!(plan.skipped[0].status, "pending");
}

#[test]
fn missing_and_corrupt_payloads_fail_as_store_corruption() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-g7");
    let source = fixture.source("proof.txt", b"proof");
    let artifact = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let task_dir = fixture.store.root().join(task.as_str());
    let payload = payload_path(&task_dir, &artifact.digest);
    fs::remove_file(&payload).expect("remove cited payload");
    let missing = fixture
        .store
        .list(&task)
        .expect_err("missing payload refused");
    assert_eq!(missing.class(), ErrorClass::Corrupt);
    assert!(missing.to_string().contains("payload is missing"));

    fs::write(&payload, b"different").expect("write corrupt payload");
    let corrupt = fixture
        .store
        .list(&task)
        .expect_err("digest mismatch refused");
    assert_eq!(corrupt.class(), ErrorClass::Corrupt);
    assert!(corrupt.to_string().contains("observed"));
}

#[cfg(unix)]
#[test]
fn dangling_manifest_link_is_corruption_not_an_omitted_backup_task() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let task = task("ARTIFACT-l2");
    let task_dir = fixture.store.root().join(task.as_str());
    fs::create_dir_all(&task_dir).expect("create task directory");
    symlink("missing-manifest", task_dir.join("manifest.json"))
        .expect("plant dangling manifest link");

    let failure = fixture
        .store
        .capture()
        .expect_err("linked manifest must not disappear from backup discovery");
    assert_eq!(failure.class(), ErrorClass::Corrupt);
    assert!(
        failure
            .to_string()
            .contains("manifest is not a regular file")
    );
}

#[test]
fn rendered_handle_directories_are_refused_as_release_one_not_read_as_a_fallback() {
    let fixture = Fixture::new();
    let legacy = fixture.store.root().join("ARTIFACT-a1");
    fs::create_dir_all(&legacy).expect("create legacy task directory");

    let failure = fixture
        .store
        .capture()
        .expect_err("release-one task directory must be refused");

    assert_eq!(failure.class(), ErrorClass::Refusal);
    assert!(failure.to_string().contains("artifact store release 1"));
    assert!(failure.to_string().contains("version 2"));
    assert!(failure.to_string().contains("inception-stamp directory"));
}

#[test]
fn backup_image_rejects_invalid_task_identity_and_missing_provenance() {
    let invalid = BackupImage::admit(vec![BackupFile {
        path: "not-a-task/manifest.json".to_owned(),
        mode: 0o600,
        bytes: b"{}".to_vec(),
    }])
    .expect_err("invalid task directory refused");
    assert_eq!(invalid.class(), ErrorClass::Corrupt);

    let fixture = Fixture::new();
    let task = task("ARTIFACT-h8");
    let source = fixture.source("proof.txt", b"proof");
    let artifact = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let mut image = fixture.store.capture().expect("capture image");
    let suffix = format!("objects/{}/payload", artifact.digest);
    image.files.retain(|file| !file.path.ends_with(&suffix));
    let missing = BackupImage::admit(image.files).expect_err("missing payload refused");
    assert_eq!(missing.class(), ErrorClass::Corrupt);
}

#[test]
fn replacement_commits_or_rolls_back_complete_provenance_generations() {
    let first = Fixture::new();
    let second = Fixture::new();
    let task = task("ARTIFACT-i9");
    let old_source = first.source("old.txt", b"old");
    let new_source = second.source("new.txt", b"new");
    add(
        &first.store,
        &task,
        &old_source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    add(
        &second.store,
        &task,
        &new_source,
        Retention::Permanent,
        instant("2030-01-02T00:00:00.000Z"),
    );
    let old = first.store.capture().expect("old image");
    let new = second.store.capture().expect("new image");

    first
        .store
        .begin_replace(&new, &old)
        .expect("begin replacement")
        .rollback()
        .expect("rollback replacement");
    assert_eq!(
        first.store.show(&task, "A1").expect("old restored").1,
        b"old"
    );

    first
        .store
        .begin_replace(&new, &old)
        .expect("begin replacement")
        .commit()
        .expect("commit replacement");
    assert_eq!(
        first.store.show(&task, "A1").expect("new restored").1,
        b"new"
    );

    let stale = first
        .store
        .begin_replace(&old, &old)
        .expect_err("stale expected generation refused");
    assert_eq!(stale.class(), ErrorClass::Refusal);
}

#[test]
fn failed_commit_cleanup_never_removes_the_published_generation() {
    let first = Fixture::new();
    let second = Fixture::new();
    let task = task("ARTIFACT-k1");
    let old_source = first.source("old.txt", b"old");
    let new_source = second.source("new.txt", b"new");
    add(
        &first.store,
        &task,
        &old_source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    add(
        &second.store,
        &task,
        &new_source,
        Retention::Permanent,
        instant("2030-01-02T00:00:00.000Z"),
    );
    let old = first.store.capture().expect("old image");
    let new = second.store.capture().expect("new image");
    let replacement = first
        .store
        .begin_replace(&new, &old)
        .expect("begin replacement");
    let previous = replacement
        .previous
        .clone()
        .expect("replacement retains the old generation");
    fs::remove_dir_all(&previous).expect("remove old generation fixture");
    fs::write(&previous, b"cleanup obstruction").expect("plant cleanup obstruction");

    let failure = replacement.commit().expect_err("commit cleanup fails");
    assert_eq!(failure.class(), ErrorClass::Environment);
    assert_eq!(
        first
            .store
            .show(&task, "A1")
            .expect("published generation remains")
            .1,
        b"new"
    );
}

#[test]
fn object_path_is_content_addressed_once_for_duplicate_bytes() {
    let fixture = Fixture::new();
    let task = task("ARTIFACT-j0");
    let source = fixture.source("same.txt", b"same");
    let first = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:00.000Z"),
    );
    let second = add(
        &fixture.store,
        &task,
        &source,
        Retention::Permanent,
        instant("2030-01-01T00:00:01.000Z"),
    );
    assert_eq!(first.digest, second.digest);
    let objects = fs::read_dir(
        object_dir(&fixture.store.root().join(task.as_str()), &first.digest)
            .parent()
            .expect("object parent"),
    )
    .expect("read objects")
    .count();
    assert_eq!(objects, 1);
}
