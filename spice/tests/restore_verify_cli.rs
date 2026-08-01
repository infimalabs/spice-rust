#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use journal::bundle::{BUNDLE_VERSION, BundleFile, BundleManifest};
use serde_json::Value;
use wire::{Digest, Domain, Outcome};

const DIGEST_SPELLING_LEN: usize = 64;

struct Fixture {
    root: PathBuf,
    bundle: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-restore-verify-{label}-{}-{sequence}",
            std::process::id()
        ));
        let bundle = root.join("backup bundle");
        fs::create_dir_all(bundle.join("data")).expect("create bundle data directory");
        fs::create_dir_all(bundle.join("tape/lane-a")).expect("create bundle tape directory");
        let journal_path = bundle.join(journal::bundle::JOURNAL_PATH);
        drop(journal::Journal::open(&journal_path).expect("create current fixture journal"));
        fs::write(
            bundle.join("tape/lane-a/member-0001.gz"),
            b"sealed tape bytes",
        )
        .expect("write fixture tape member");
        let manifest = manifest(&bundle);
        fs::write(
            bundle.join(journal::bundle::MANIFEST_PATH),
            wire::encode(&manifest).expect("encode canonical bundle manifest"),
        )
        .expect("write bundle manifest");
        Self { root, bundle }
    }

    fn invoke(&self, json: bool) -> Spoken {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        if json {
            command.arg("--json");
        }
        let output = command
            .args(["restore", "verify"])
            .arg(&self.bundle)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice restore verify process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[test]
fn current_enumerated_bundle_verifies_in_both_forms_without_mutation() {
    let fixture = Fixture::new("valid");
    let before = snapshot(&fixture.bundle);
    let json = reply_body(fixture.invoke(true));
    let text: Value = serde_json::from_slice(&fixture.invoke(false).out)
        .expect("text restore verification is the stable body document");
    assert_eq!(json, text);
    assert_eq!(json["journal_schema"], journal::SCHEMA_VERSION);
    assert_eq!(json["files"], 2);
    assert_eq!(json["tape_files"], 1);
    assert!(
        json["identity"]
            .as_str()
            .is_some_and(|digest| digest.len() == DIGEST_SPELLING_LEN)
    );
    assert_eq!(snapshot(&fixture.bundle), before);
}

#[test]
fn checksum_unlisted_and_tape_manifest_drift_are_refused_by_name() {
    for (label, corrupt) in [
        ("checksum", corrupt_checksum as fn(&Fixture)),
        ("unlisted", add_unlisted),
        ("tape", corrupt_tape_manifest),
    ] {
        let fixture = Fixture::new(label);
        corrupt(&fixture);
        refusal(fixture.invoke(true), "restore::bundle-refused");
    }
}

#[test]
fn symlinked_entries_and_noncanonical_or_traversing_manifests_are_refused() {
    let symlinked = Fixture::new("symlink");
    let outside = symlinked.root.join("outside");
    fs::write(&outside, b"outside bytes").expect("write symlink target");
    symlink(
        &outside,
        symlinked.bundle.join("tape/lane-a/member-0002.gz"),
    )
    .expect("plant bundle symlink");
    refusal(symlinked.invoke(true), "restore::bundle-refused");
    assert_eq!(
        fs::read(outside).expect("read symlink target"),
        b"outside bytes"
    );

    let noncanonical = Fixture::new("noncanonical");
    let path = noncanonical.bundle.join(journal::bundle::MANIFEST_PATH);
    let mut bytes = fs::read(&path).expect("read canonical manifest");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("write noncanonical manifest");
    refusal(noncanonical.invoke(true), "restore::bundle-refused");

    let traversal = Fixture::new("traversal");
    let mut manifest = manifest(&traversal.bundle);
    manifest.files[0].path = "../authority.sqlite3".to_owned();
    write_manifest(&traversal, &manifest);
    refusal(traversal.invoke(true), "restore::bundle-refused");
}

fn manifest(bundle: &Path) -> BundleManifest {
    let paths = [journal::bundle::JOURNAL_PATH, "tape/lane-a/member-0001.gz"];
    BundleManifest {
        version: BUNDLE_VERSION,
        journal_schema: journal::SCHEMA_VERSION,
        files: paths
            .into_iter()
            .map(|path| {
                let bytes = fs::read(bundle.join(path)).expect("read enumerated fixture file");
                BundleFile {
                    path: path.to_owned(),
                    bytes: bytes.len() as u64,
                    digest: Digest::compute(Domain::RestoreBundleFile, &bytes),
                }
            })
            .collect(),
        tape: vec!["tape/lane-a/member-0001.gz".to_owned()],
    }
}

fn write_manifest(fixture: &Fixture, manifest: &BundleManifest) {
    fs::write(
        fixture.bundle.join(journal::bundle::MANIFEST_PATH),
        wire::encode(manifest).expect("encode changed manifest"),
    )
    .expect("write changed manifest");
}

fn corrupt_checksum(fixture: &Fixture) {
    fs::write(
        fixture.bundle.join("tape/lane-a/member-0001.gz"),
        b"changed tape bytes",
    )
    .expect("corrupt tape member");
}

fn add_unlisted(fixture: &Fixture) {
    fs::write(fixture.bundle.join("unlisted"), b"not in manifest")
        .expect("write unlisted bundle file");
}

fn corrupt_tape_manifest(fixture: &Fixture) {
    let mut manifest = manifest(&fixture.bundle);
    manifest.tape.clear();
    write_manifest(fixture, &manifest);
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    snapshot_directory(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn snapshot_directory(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in fs::read_dir(directory).expect("read snapshot directory") {
        let path = entry.expect("read snapshot entry").path();
        if path.is_dir() {
            snapshot_directory(root, &path, files);
        } else {
            files.push((
                path.strip_prefix(root)
                    .expect("snapshot path is under root")
                    .to_path_buf(),
                fs::read(path).expect("read snapshot file"),
            ));
        }
    }
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(
        spoken.code,
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&spoken.err)
    );
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> =
        wire::decode(&spoken.out).expect("restore verify outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("restore verify must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn refusal(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("restore refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("restore verification must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}
