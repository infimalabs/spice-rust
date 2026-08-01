use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde::de::DeserializeOwned;
use serde_json::json;
use sill::{Acquisition, LockFile, LockName};
use wire::{
    Actor, AnyFact, Envelope, FactKind, FoldReport, JournalBackup, JournalReindex, JournalReport,
    JournalRestore, JournalVerification, Origin, Outcome, Seq, ThreadReport, WireVersion,
};

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
        let data = root.join(".git/.spice-rust/data");
        let authority = data.join("spicejournal.sqlite3");
        let folds = data.join("spicefolds.sqlite3");
        let bundle = root.join("operator-backup");
        fs::create_dir_all(&data).expect("create repository state fixture");
        seed_authority(&authority);
        Self {
            root,
            authority,
            folds,
            bundle,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .arg("--json")
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("launch spice journal command")
    }

    fn reply<B: DeserializeOwned>(&self, arguments: &[&str]) -> B {
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
        reply.body
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove journal CLI fixture");
    }
}

#[test]
fn journal_replay_reindex_thread_backup_and_restore_round_trip() {
    let fixture = Fixture::new();
    assert_journal_thread_and_fold(&fixture);
    assert_reindex(&fixture);
    assert_backup(&fixture);
    assert_restore(&fixture);
}

fn assert_journal_thread_and_fold(fixture: &Fixture) {
    let report = fixture.reply::<JournalReport>(&["journal"]);
    assert_eq!(report.schema, journal::SCHEMA_VERSION);
    assert_eq!(report.cursor, Seq::new(2));
    assert_eq!(report.facts, 2);
    let verified = fixture.reply::<JournalVerification>(&["journal", "verify"]);
    assert_eq!((verified.facts, verified.threads), (2, 2));

    let thread = fixture.reply::<ThreadReport>(&["thread", "2"]);
    assert_eq!(thread.root, Seq::new(1).expect("root sequence"));
    assert_eq!(
        thread
            .entries
            .iter()
            .map(|entry| entry.seq.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    let missing = fixture.reply::<FoldReport>(&["fold"]);
    assert!(missing.folds.iter().all(|fold| fold.generation.is_none()));
    assert!(
        !fixture.folds.exists(),
        "read-only fold report created a store"
    );
    let first = fixture.reply::<FoldReport>(&["journal", "replay", "canvas::flow"]);
    let second = fixture.reply::<FoldReport>(&["journal", "replay", "canvas::flow"]);
    assert_eq!(
        second, first,
        "fresh deterministic replay minted a generation"
    );
    let fold_bytes = fs::read(&fixture.folds).expect("read folds before report");
    assert_eq!(fixture.reply::<FoldReport>(&["fold"]), second);
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
    assert_eq!(flow.cursor, Seq::new(2));
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
    let invalid = fixture.invoke(&["journal", "verify"]);
    assert_eq!(invalid.status.code(), Some(2));
    let rebuilt = fixture.reply::<JournalReindex>(&["journal", "reindex"]);
    assert_eq!((rebuilt.facts, rebuilt.threads), (2, 2));
    fixture.reply::<JournalVerification>(&["journal", "verify"]);
    fixture.reply::<ThreadReport>(&["thread", "2"]);
}

fn assert_backup(fixture: &Fixture) {
    let backup = fixture.reply::<JournalBackup>(&[
        "backup",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert_eq!(backup.verification.facts, 2);
    assert_eq!(backup.verification.threads, 2);
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
    assert_eq!(fixture.reply::<JournalReport>(&["journal"]).facts, 0);

    let preview = fixture.reply::<JournalRestore>(&[
        "restore",
        fixture.bundle.to_str().expect("UTF-8 backup path"),
    ]);
    assert!(!preview.applied);
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
        &format!("--apply={}", preview.plan),
    ]);
    assert!(applied.applied);
    assert_eq!(fixture.reply::<JournalReport>(&["journal"]).facts, 2);
    fixture.reply::<JournalVerification>(&["journal", "verify"]);

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
        fixture.root.join(".git/.spice-rust/resident"),
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
