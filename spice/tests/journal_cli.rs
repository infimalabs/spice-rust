use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde::de::DeserializeOwned;
use serde_json::json;
use sill::{Acquisition, LockFile, LockName};
use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest};
use wire::{
    Actor, AnyFact, Envelope, FactKind, FoldReport, JournalBackup, JournalReindex, JournalReport,
    JournalRestore, JournalVerification, Origin, Outcome, Seq, ThreadReport, WireVersion,
};

#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    authority: PathBuf,
    folds: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-journal-cli-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let data = root.join(".git/.spice-rs/data");
        let authority = data.join("spicejournal.sqlite3");
        let folds = data.join("spicefolds.sqlite3");
        let bundle = root.join("operator-backup");
        fs::create_dir_all(&data).expect("create repository state fixture");
        staged_app::seed(&root);
        seed_authority(&authority);
        Self {
            root,
            authority,
            folds,
            bundle,
        }
    }

    fn invoke_form(&self, arguments: &[&str], json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        command
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("launch spice journal command")
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        self.invoke_form(arguments, true)
    }

    fn reply<B: DeserializeOwned>(&self, arguments: &[&str]) -> wire::Reply<B> {
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

    fn start(&self) -> RunningResident {
        RunningResident::spawn(self)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove journal CLI fixture");
    }
}

struct RunningResident {
    child: Option<Child>,
}

impl RunningResident {
    fn spawn(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--port", "0"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch foreground resident");
        let mut resident = Self { child: Some(child) };
        let canvas = fixture.root.join(".git/.spice-rs/canvas");
        let watched = canvas.parent().expect("canvas has runtime parent");
        fs::create_dir_all(watched).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("resident exited before publishing discovery"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&["down"]);
        assert!(
            down.status.success(),
            "resident down failed: {}",
            String::from_utf8_lossy(&down.stderr)
        );
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "foreground resident failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn terminate(mut self, fixture: &Fixture) {
        let mut child = self.child.take().expect("resident child");
        child.kill().expect("terminate damaged resident");
        child.wait().expect("reap damaged resident");
        let canvas = fixture.root.join(".git/.spice-rs/canvas");
        if canvas.exists() {
            fs::remove_file(canvas).expect("remove interrupted discovery");
        }
    }
}

impl Drop for RunningResident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn journal_replay_reindex_thread_backup_and_restore_round_trip() {
    let fixture = Fixture::new();
    let resident = fixture.start();
    assert_journal_thread_and_fold(&fixture);
    resident.stop(&fixture);
    assert_reindex(&fixture);
    assert_backup(&fixture);
    assert_restore(&fixture);
}

#[test]
fn authority_only_backup_v1_uses_the_sole_current_restore_path() {
    let fixture = Fixture::new();
    fixture.reply::<JournalBackup>(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    let manifest_path = fixture.bundle.join("manifest.json");
    let mut manifest: JournalBackupManifest =
        wire::decode(&fs::read(&manifest_path).expect("read current backup manifest"))
            .expect("decode current backup manifest");
    assert_eq!(manifest.version, JOURNAL_BACKUP_VERSION);
    assert_eq!(manifest.members.len(), 1, "fixture must be authority-only");
    manifest.version = 1;
    fs::write(
        &manifest_path,
        wire::encode(&manifest).expect("encode canonical backup v1 manifest"),
    )
    .expect("write backup v1 manifest");

    fs::remove_file(&fixture.authority).expect("remove current authority");
    drop(journal::Journal::open(&fixture.authority).expect("create empty authority"));
    let preview = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert!(!preview.body.applied);
    assert_eq!(preview.body.verification.members, 1);
    let applied = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
        &format!("--apply={}", preview.body.plan),
    ]);
    assert!(applied.body.applied);
    journal::JournalReader::open(&fixture.authority)
        .expect("restored v1 authority opens")
        .verify()
        .expect("restored v1 authority verifies through the current path");
}

fn assert_journal_thread_and_fold(fixture: &Fixture) {
    let reply = fixture.reply::<JournalReport>(&["journal"]);
    let wire::ReplyPosition::Snapshot(position) = reply.position else {
        panic!("journal read did not carry a snapshot position")
    };
    let report = reply.body;
    assert_eq!(report.schema, journal::SCHEMA_VERSION);
    assert_eq!(report.cursor, Some(position));
    assert_eq!(
        usize::try_from(report.facts).expect("fact count fits usize"),
        report.entries.rows.len()
    );
    assert!(report.entries.rows.len() >= 2);
    assert_eq!(
        report.entries.rows[0].seq,
        Seq::new(1).expect("first sequence")
    );
    assert_eq!(report.entries.horizon, position);
    assert!(report.entries.overflow.is_none());
    let text = fixture.invoke_form(&["journal"], false);
    assert!(text.status.success());
    let text_body: JournalReport = wire::decode(&text.stdout).expect("text journal body");
    assert_eq!(text_body, report, "text and JSON carry the same body");

    let verified = fixture.reply::<JournalVerification>(&["journal", "verify"]);
    assert_eq!(verified.position, wire::ReplyPosition::Snapshot(position));
    assert_eq!(verified.body.facts, report.facts);

    let thread = fixture.reply::<ThreadReport>(&["thread", "2"]);
    assert_eq!(thread.position, wire::ReplyPosition::Snapshot(position));
    let thread = thread.body;
    assert_eq!(thread.root, Seq::new(1).expect("root sequence"));
    assert_eq!(
        thread
            .entries
            .rows
            .iter()
            .map(|entry| entry.seq.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    let missing = fixture.reply::<FoldReport>(&["fold"]).body;
    assert!(missing.folds.iter().all(|fold| fold.generation.is_none()));
    assert!(
        !fixture.folds.exists(),
        "read-only fold report created a store"
    );
    let first = fixture
        .reply::<FoldReport>(&["journal", "replay", "canvas::flow"])
        .body;
    let second = fixture
        .reply::<FoldReport>(&["journal", "replay", "canvas::flow"])
        .body;
    assert_eq!(
        second, first,
        "fresh deterministic replay minted a generation"
    );
    let fold_bytes = fs::read(&fixture.folds).expect("read folds before report");
    assert_eq!(fixture.reply::<FoldReport>(&["fold"]).body, second);
    assert_eq!(
        fs::read(&fixture.folds).expect("read folds after report"),
        fold_bytes,
        "read-only fold report changed the materialization store"
    );
    let flow = first
        .folds
        .iter()
        .find(|fold| fold.fold == "canvas::flow")
        .expect("canvas flow state");
    assert_eq!(flow.generation, Some(1));
    assert_eq!(flow.cursor, Some(position));
    assert!(!flow.sources.is_empty(), "fold provenance is explicit");
    assert_eq!(
        flow.payload.as_ref().expect("flow payload")["facts"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

fn assert_reindex(fixture: &Fixture) {
    Connection::open(&fixture.authority)
        .expect("open authority for index damage")
        .execute("DELETE FROM thread_index WHERE seq = 2", [])
        .expect("damage disposable index");
    assert!(
        journal::JournalReader::open(&fixture.authority)
            .expect("damaged authority opens")
            .verify()
            .is_err(),
        "damaged index unexpectedly verified"
    );
    let resident = fixture.start();
    let invalid = fixture.invoke(&["journal", "verify"]);
    assert_eq!(invalid.status.code(), Some(2));
    let Outcome::Refusal(refusal) = wire::decode::<Outcome<JournalVerification>>(&invalid.stderr)
        .expect("verification refusal decodes")
    else {
        panic!("damaged authority did not refuse verification")
    };
    assert!(
        refusal
            .observed
            .as_deref()
            .is_some_and(|observed| observed.contains("Thread")),
        "verification omitted the named thread defect: {refusal:?}"
    );
    resident.terminate(fixture);
    let rebuilt = fixture.reply::<JournalReindex>(&["journal", "reindex"]);
    let expected = journal::JournalReader::open(&fixture.authority)
        .expect("reindexed authority cursor opens")
        .cursor()
        .expect("reindexed authority cursor reads")
        .expect("reindexed authority cursor exists");
    assert_eq!(rebuilt.position, wire::ReplyPosition::Snapshot(expected));
    assert_eq!(rebuilt.body.facts, rebuilt.body.threads);
    let reader = journal::JournalReader::open(&fixture.authority).expect("rebuilt authority opens");
    reader.verify().expect("rebuilt authority verifies");
    assert_eq!(reader.thread("2").expect("rebuilt thread").entries.len(), 2);
}

fn assert_backup(fixture: &Fixture) {
    let before = journal::JournalReader::open(&fixture.authority)
        .expect("authority before backup")
        .fact_snapshot()
        .expect("authority snapshot before backup");
    let backup = fixture.reply::<JournalBackup>(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    let wire::ReplyPosition::Committed(range) = backup.position else {
        panic!("backup did not commit its recognition")
    };
    assert_eq!(range.first, range.last);
    assert_eq!(
        usize::try_from(backup.body.verification.facts).expect("fact count fits usize"),
        before.facts().len()
    );
    assert_eq!(
        usize::try_from(backup.body.verification.threads).expect("thread count fits usize"),
        before.facts().len()
    );
    let after = journal::JournalReader::open(&fixture.authority)
        .expect("authority after backup")
        .fact_snapshot()
        .expect("authority snapshot after backup");
    assert_eq!(after.facts().len(), before.facts().len() + 1);
    assert_eq!(
        after.facts().last().expect("backup fact").fact.kind(),
        FactKind::JournalBacked
    );
    #[cfg(unix)]
    {
        assert_eq!(mode(&fixture.bundle), 0o700);
        assert_eq!(mode(&fixture.bundle.join("manifest.json")), 0o600);
        assert_eq!(mode(&fixture.bundle.join("spicejournal.sqlite3")), 0o600);
    }
    let backup_authority = fs::read(fixture.bundle.join("spicejournal.sqlite3"))
        .expect("read published backup authority");
    let duplicate = fixture.invoke(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert_eq!(duplicate.status.code(), Some(2));
    assert_eq!(
        fs::read(fixture.bundle.join("spicejournal.sqlite3"))
            .expect("read backup authority after duplicate refusal"),
        backup_authority,
        "duplicate backup replaced the existing bundle"
    );
}

fn assert_restore(fixture: &Fixture) {
    fs::remove_file(&fixture.authority).expect("remove authority for empty replacement");
    drop(journal::Journal::open(&fixture.authority).expect("create empty replacement authority"));
    assert!(
        journal::JournalReader::open(&fixture.authority)
            .expect("empty replacement authority")
            .fact_snapshot()
            .expect("empty replacement snapshot")
            .facts()
            .is_empty()
    );

    let preview = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert_eq!(preview.position, wire::ReplyPosition::Direct {});
    assert!(!preview.body.applied);
    let before_stale = fs::read(&fixture.authority).expect("read predecessor before stale plan");
    let stale = fixture.invoke(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
        &format!("--apply={}", "0".repeat(64)),
    ]);
    assert_eq!(stale.status.code(), Some(2));
    assert_eq!(
        fs::read(&fixture.authority).expect("read predecessor after stale plan"),
        before_stale,
        "stale restore plan changed authority"
    );

    let applied = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
        &format!("--apply={}", preview.body.plan),
    ]);
    assert!(matches!(
        applied.position,
        wire::ReplyPosition::Committed(_)
    ));
    assert!(applied.body.applied);
    #[cfg(unix)]
    assert_eq!(mode(&fixture.authority), 0o600);
    let restored_snapshot = journal::JournalReader::open(&fixture.authority)
        .expect("restored authority opens")
        .fact_snapshot()
        .expect("restored authority snapshot");
    assert_eq!(
        restored_snapshot.facts().len(),
        usize::try_from(applied.body.verification.facts).expect("fact count fits usize") + 1
    );
    assert_eq!(
        restored_snapshot
            .facts()
            .last()
            .expect("restore fact")
            .fact
            .kind(),
        FactKind::JournalRestored
    );
    journal::JournalReader::open(&fixture.authority)
        .expect("restored authority opens")
        .verify()
        .expect("restored authority verifies");

    let resident = fixture.start();
    let report = fixture.reply::<JournalReport>(&["journal"]);
    assert!(matches!(report.position, wire::ReplyPosition::Snapshot(_)));
    fixture.reply::<JournalVerification>(&["journal", "verify"]);
    resident.stop(fixture);

    let restored = fs::read(&fixture.authority).expect("read restored authority");
    fs::write(
        fixture.bundle.join("spicejournal.sqlite3"),
        b"corrupt replacement",
    )
    .expect("corrupt backup member");
    let corrupt = fixture.invoke(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert!(!corrupt.status.success());
    assert_eq!(
        fs::read(&fixture.authority).expect("read authority after corrupt preview"),
        restored,
        "invalid bundle preview changed authority"
    );
}

#[test]
fn active_writer_refuses_maintenance_in_a_typed_envelope() {
    let fixture = Fixture::new();
    let lock = LockFile::new(
        LockName::new("resident").expect("resident lock name"),
        fixture.root.join(".git/.spice-rs/resident"),
    );
    let Acquisition::Acquired(_held) = lock.try_acquire().expect("acquire writer fixture") else {
        panic!("writer fixture lock was unexpectedly held")
    };
    let output = fixture.invoke(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert_eq!(output.status.code(), Some(2));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<JournalBackup>>(&output.stderr).expect("decode lock refusal")
    else {
        panic!("contended backup did not return a refusal")
    };
    assert_eq!(refusal.reason, "journal::writer-active");
    assert!(!fixture.bundle.exists(), "contended backup created output");
}

#[test]
fn resident_reads_require_authentication_without_mutating_authority() {
    let fixture = Fixture::new();
    let before = fs::read(&fixture.authority).expect("authority before unauthenticated reads");
    for arguments in [
        &["journal"][..],
        &["journal", "verify"][..],
        &["journal", "replay", "canvas::flow"][..],
        &["fold"][..],
        &["thread", "2"][..],
    ] {
        let output = fixture.invoke(arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        let Outcome::Fault(fault) = wire::decode::<Outcome<serde_json::Value>>(&output.stderr)
            .expect("unauthenticated read has a typed fault")
        else {
            panic!("unauthenticated read did not fault: {arguments:?}")
        };
        assert_eq!(fault.reason, "canvas::discovery-unavailable");
    }
    assert_eq!(
        fs::read(&fixture.authority).expect("authority after unauthenticated reads"),
        before,
        "unauthenticated reads changed authority"
    );
    assert!(
        !fixture.folds.exists(),
        "unauthenticated replay created folds"
    );
}

#[test]
fn journal_and_thread_reports_bound_rows_and_name_continuation() {
    let fixture = Fixture::new();
    append_note_facts(&fixture.authority, 300);
    let resident = fixture.start();

    let journal = fixture.reply::<JournalReport>(&["journal"]).body;
    assert_eq!(journal.entries.rows.len(), 256);
    let journal_overflow = journal.entries.overflow.expect("journal overflow marker");
    assert!(journal_overflow.omitted > 0);
    assert_eq!(journal_overflow.continuation.get(), 257);

    let thread = fixture.reply::<ThreadReport>(&["thread", "300"]).body;
    assert_eq!(thread.entries.rows.len(), 256);
    let thread_overflow = thread.entries.overflow.expect("thread overflow marker");
    assert_eq!(thread_overflow.omitted, 44);
    assert_eq!(thread_overflow.continuation.get(), 257);
    resident.stop(&fixture);
}

#[test]
fn live_resident_excludes_every_lock_held_recovery_mutation() {
    let fixture = Fixture::new();
    fixture.reply::<JournalBackup>(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    let resident = fixture.start();
    let before = journal::JournalReader::open(&fixture.authority)
        .expect("live authority opens")
        .fact_snapshot()
        .expect("live authority snapshot");

    let alternate = fixture.root.join("second-backup");
    let backup = fixture.invoke(&["backup", alternate.to_str().expect("UTF-8 backup path")]);
    assert_refusal(&backup, "journal::writer-active");
    assert!(!alternate.exists());

    let reindex = fixture.invoke(&["journal", "reindex"]);
    assert_refusal(&reindex, "journal::writer-active");

    let preview = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 bundle path"),
    ]);
    let restore = fixture.invoke(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 bundle path"),
        &format!("--apply={}", preview.body.plan),
    ]);
    assert_refusal(&restore, "restore::writer-active");
    let after = journal::JournalReader::open(&fixture.authority)
        .expect("live authority reopens")
        .fact_snapshot()
        .expect("live authority resnapshot");
    assert_eq!(after, before, "refused recovery changed authority");
    resident.stop(&fixture);
}

fn assert_refusal(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(2));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<serde_json::Value>>(&output.stderr).expect("typed refusal")
    else {
        panic!("recovery did not refuse")
    };
    assert_eq!(refusal.reason, reason);
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .expect("backup member metadata")
        .permissions()
        .mode()
        & 0o7777
}

fn seed_authority(path: &Path) {
    drop(journal::Journal::open(path).expect("baseline authority fixture"));
    let connection = Connection::open(path).expect("open authority fixture for facts");
    for envelope in [
        fact(
            1,
            None,
            FactKind::TaskAdded,
            json!({
                "task": "JOURNAL-a1",
                "uuid": "018f82d4-6f59-7d06-92e3-bce9131163c2",
                "title": "Journal fixture",
                "project": "journal",
                "tags": [],
                "depends": [],
                "flow": ["todo", "review"],
                "phase": "todo",
                "revision": 1
            }),
        ),
        fact(
            2,
            Some(1),
            FactKind::TaskPhased,
            json!({"task": "JOURNAL-a1", "phase": "review"}),
        ),
    ] {
        let bytes = wire::encode(&envelope).expect("encode fact fixture");
        connection
            .execute(
                concat!(
                    "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, ",
                    "cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                ),
                params![
                    i64::try_from(envelope.seq.get()).expect("fixture seq fits"),
                    envelope.v.0,
                    envelope.at.to_string(),
                    envelope.actor.to_string(),
                    envelope.origin.as_str(),
                    envelope
                        .cause
                        .map(Seq::get)
                        .map(|cause| i64::try_from(cause).expect("cause fits")),
                    envelope.fact.kind().as_str(),
                    bytes,
                ],
            )
            .expect("insert fact fixture");
    }
    connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES
                (1, 'task:JOURNAL-a1', NULL, 1, 0),
                (2, 'task:JOURNAL-a1', 1, 1, 1);
             UPDATE cursor_head SET seq = 2, wake = 1 WHERE singleton = 1;",
        )
        .expect("insert index and cursor fixture");
}

fn append_note_facts(path: &Path, through: u64) {
    let connection = Connection::open(path).expect("open authority for bounded-report facts");
    for seq in 3..=through {
        let envelope = fact(
            seq,
            Some(seq - 1),
            FactKind::TaskNoted,
            json!({"task": "JOURNAL-a1", "note": format!("note {seq}")}),
        );
        let bytes = wire::encode(&envelope).expect("encode bounded-report fact");
        connection
            .execute(
                concat!(
                    "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, ",
                    "cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                ),
                params![
                    i64::try_from(seq).expect("fixture sequence fits"),
                    envelope.v.0,
                    envelope.at.to_string(),
                    envelope.actor.to_string(),
                    envelope.origin.as_str(),
                    i64::try_from(seq - 1).expect("fixture cause fits"),
                    envelope.fact.kind().as_str(),
                    bytes,
                ],
            )
            .expect("insert bounded-report fact");
        connection
            .execute(
                "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, ?2, ?3, 1, ?4)",
                params![
                    i64::try_from(seq).expect("fixture sequence fits"),
                    envelope.origin.as_str(),
                    i64::try_from(seq - 1).expect("fixture cause fits"),
                    i64::try_from(seq - 1).expect("fixture depth fits"),
                ],
            )
            .expect("insert bounded-report thread");
    }
    connection
        .execute(
            "UPDATE cursor_head SET seq = ?1, wake = 1 WHERE singleton = 1",
            [i64::try_from(through).expect("fixture cursor fits")],
        )
        .expect("advance bounded-report cursor");
}

fn fact(
    seq: u64,
    cause: Option<u64>,
    kind: FactKind,
    body: serde_json::Value,
) -> Envelope<AnyFact> {
    let mut fields = body.as_object().cloned().expect("fact body is an object");
    fields.insert(
        "kind".to_owned(),
        serde_json::to_value(kind).expect("encode fact kind"),
    );
    let fact = wire::decode(&serde_json::to_vec(&fields).expect("encode fact fields"))
        .expect("registered fact body");
    Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(seq).expect("nonzero fixture seq"),
        at: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
        actor: Actor::Spice,
        origin: "task:JOURNAL-a1".parse::<Origin>().expect("fixture origin"),
        cause: cause.map(|cause| Seq::new(cause).expect("nonzero fixture cause")),
        fact,
    }
}
