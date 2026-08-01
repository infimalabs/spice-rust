use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest, JournalBackupMember};
use wire::{Digest, Domain, FaultClass, Outcome, ReplyPosition, RestoreVerification};

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
const AUTHORITY: &str = "spicejournal.sqlite3";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-restore-verify-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let bundle = root.join("backup");
        fs::create_dir_all(root.join(".git")).expect("fixture Git directory");
        fs::create_dir_all(bundle.join("tape")).expect("bundle directories");
        drop(
            journal::Journal::open(bundle.join(AUTHORITY))
                .expect("finalized authority fixture opens"),
        );
        fs::write(bundle.join("tape/lane-a"), b"sealed transcript member")
            .expect("tape fixture writes");
        let fixture = Self { root, bundle };
        fixture.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
        fixture
    }

    fn write_manifest(&self, version: u16, journal_schema: u32) {
        let mut members = [AUTHORITY, "tape/lane-a"]
            .into_iter()
            .map(|path| {
                let bytes = fs::read(self.bundle.join(path)).expect("member reads");
                JournalBackupMember {
                    path: path.to_owned(),
                    bytes: u64::try_from(bytes.len()).expect("fixture length crosses"),
                    digest: Digest::compute(Domain::JournalBackupMember, &bytes),
                }
            })
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = JournalBackupManifest {
            version,
            journal_schema,
            authority: AUTHORITY.to_owned(),
            members,
        };
        fs::write(
            self.bundle.join("manifest.json"),
            wire::encode(&manifest).expect("manifest encodes"),
        )
        .expect("manifest writes");
    }

    fn invoke(&self, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        command
            .args(["restore", "verify"])
            .arg(&self.bundle)
            .current_dir(&self.root)
            .output()
            .expect("spice restore verify launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

#[test]
fn valid_bundle_verifies_in_both_forms_without_mutating_the_repository() {
    let fixture = Fixture::new("valid");
    let before = tree(&fixture.root);

    let json = fixture.invoke(true);
    assert!(
        json.status.success(),
        "JSON verify failed: {:?}",
        json.stderr
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<RestoreVerification>>(&json.stdout).expect("JSON outcome decodes")
    else {
        panic!("valid bundle must reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body.journal_schema, journal::SCHEMA_VERSION);
    assert_eq!(reply.body.members, 2);
    assert_eq!(reply.body.facts, 0);
    assert_eq!(reply.body.receipts, 0);
    assert_eq!(reply.body.heads, 0);
    assert_eq!(reply.body.threads, 0);
    assert_eq!(
        reply.body.authority,
        Digest::compute(
            Domain::JournalBackupMember,
            &fs::read(fixture.bundle.join(AUTHORITY)).expect("authority reads")
        )
    );

    let text = fixture.invoke(false);
    assert!(
        text.status.success(),
        "text verify failed: {:?}",
        text.stderr
    );
    let text_body = wire::decode::<RestoreVerification>(&text.stdout).expect("text body decodes");
    assert_eq!(text_body, reply.body, "forms report one typed result");
    assert_eq!(
        tree(&fixture.root),
        before,
        "verification mutated its inputs"
    );
}

#[test]
fn missing_forward_and_incomplete_bundles_refuse_by_class() {
    let missing = Fixture::new("missing");
    let absent = missing.root.join("absent");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "restore", "verify"])
        .arg(absent)
        .current_dir(&missing.root)
        .output()
        .expect("missing verify launches");
    assert_refusal(&output, "restore::bundle-incomplete");

    let forward = Fixture::new("forward");
    forward.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION + 1);
    assert_refusal(&forward.invoke(true), "restore::schema-forward");

    let hidden_forward = Fixture::new("hidden-forward");
    let connection = rusqlite::Connection::open(hidden_forward.bundle.join(AUTHORITY))
        .expect("authority opens for forward fixture");
    connection
        .execute(
            "UPDATE journal_schema SET version = ?1 WHERE singleton = 1",
            [journal::SCHEMA_VERSION + 1],
        )
        .expect("authority schema advances");
    connection
        .pragma_update(None, "user_version", journal::SCHEMA_VERSION + 1)
        .expect("authority pragma advances");
    drop(connection);
    hidden_forward.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    assert_refusal(&hidden_forward.invoke(true), "restore::schema-forward");

    let incomplete = Fixture::new("incomplete");
    fs::remove_file(incomplete.bundle.join("tape/lane-a")).expect("member removes");
    assert_refusal(&incomplete.invoke(true), "restore::bundle-incomplete");
}

#[test]
fn corrupt_manifest_digest_and_replay_closure_fault_as_store_evidence() {
    let manifest = Fixture::new("manifest-corrupt");
    fs::write(manifest.bundle.join("manifest.json"), b"not a manifest").expect("manifest corrupts");
    assert_store_fault(&manifest.invoke(true));

    let digest = Fixture::new("digest-corrupt");
    fs::write(digest.bundle.join("tape/lane-a"), b"changed after backup").expect("member corrupts");
    assert_store_fault(&digest.invoke(true));

    let replay = Fixture::new("replay-corrupt");
    let connection = rusqlite::Connection::open(replay.bundle.join(AUTHORITY))
        .expect("authority opens for corruption fixture");
    connection
        .execute("DELETE FROM cursor_head", [])
        .expect("cursor singleton corrupts");
    drop(connection);
    replay.write_manifest(JOURNAL_BACKUP_VERSION, journal::SCHEMA_VERSION);
    let before = tree(&replay.root);
    assert_store_fault(&replay.invoke(true));
    assert_eq!(tree(&replay.root), before, "failed replay mutated bundle");
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::<RestoreVerification>::Refusal(refusal) =
        wire::decode(&output.stderr).expect("refusal decodes")
    else {
        panic!("expected typed refusal: {:?}", output.stderr);
    };
    assert_eq!(refusal.reason, reason);
    assert!(output.stdout.is_empty(), "refusal spoke on stdout");
}

fn assert_store_fault(output: &Output) {
    assert_eq!(output.status.code(), Some(FAULT_EXIT));
    let Outcome::<RestoreVerification>::Fault(fault) =
        wire::decode(&output.stderr).expect("fault decodes")
    else {
        panic!("expected typed fault: {:?}", output.stderr);
    };
    assert_eq!(fault.class, FaultClass::Store);
    assert_eq!(fault.reason, "restore::bundle-invalid");
    assert!(output.stdout.is_empty(), "fault spoke on stdout");
}

fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut state = BTreeMap::new();
    walk(root, root, &mut state);
    state
}

fn walk(root: &Path, directory: &Path, state: &mut BTreeMap<String, Option<Vec<u8>>>) {
    let mut entries = fs::read_dir(directory)
        .expect("snapshot directory reads")
        .collect::<Result<Vec<_>, _>>()
        .expect("snapshot entries read");
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("snapshot entry belongs to root")
            .to_string_lossy()
            .into_owned();
        let kind = entry.file_type().expect("snapshot file type");
        if kind.is_dir() {
            state.insert(relative, None);
            walk(root, &path, state);
        } else {
            state.insert(relative, Some(fs::read(path).expect("snapshot file reads")));
        }
    }
}
