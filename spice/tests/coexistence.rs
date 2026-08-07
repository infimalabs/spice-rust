//! The predecessor's durable state survives this port running beside it.
//!
//! `spice/src/repository.rs` states the coexistence policy; this file enforces
//! it. Everything here works by identity -- seed files the predecessor owns
//! under a fixture state root, run the real binary, read the bytes back --
//! rather than by asserting that no offending code path exists. A policy proved
//! by grepping for the absence of a call is only as good as the grep.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use spice::repository::Repository;

/// Durable state the predecessor owns inside the shared root.
///
/// `data/` is this port's own directory, but only `spicejournal.sqlite3` and
/// `spicefolds.sqlite3` inside it belong to this port; every other file there is
/// the predecessor's. Ownership is decided per path, never per parent, which is
/// why seeding foreign files inside an owned directory is the interesting case
/// rather than an unfair one.
const PREDECESSOR_STATE: &[(&str, &[u8])] = &[
    ("data/taskchampion.sqlite3", b"taskchampion pages"),
    ("data/spiceacks.sqlite3", b"ack pages"),
    ("data/spicemaxims.sqlite3", b"maxim pages"),
    ("data/spiceprojections.sqlite3", b"projection pages"),
    ("data/spiceteams.sqlite3", b"team pages"),
    (
        "migrations/v0.31.0-repository-config-authority.json",
        br#"{"release": "v0.31.0", "schema_version": 1}"#,
    ),
    ("taskrc", b"data.location=/dev/null\n"),
    ("hooks/pre-commit", b"#!/bin/sh\nexit 0\n"),
];

/// One accepted release-0.31.0 export, the only input the bridge admits.
const EXPORT: &[u8] = br#"{
  "version": 1,
  "release": "0.31.0",
  "tasks": {"hidden_stems": ["sandbox"]},
  "teams": []
}"#;

/// The runtime must not claim a name the predecessor already answers for.
///
/// Every other test here subtracts the owned set to decide what is foreign, so
/// this is the assertion that keeps that subtraction honest: if this port ever
/// declares `migrations/` or `taskrc` as its own, the seeded fixtures below stop
/// describing another system's property and would start passing vacuously.
#[test]
fn this_runtime_claims_no_path_the_predecessor_owns() {
    let fixture = Fixture::seeded("claims");
    let owned = fixture.owned();

    for (relative, _) in PREDECESSOR_STATE {
        let seeded = fixture.state_root().join(relative);
        assert!(
            !owned.contains(&seeded),
            "{relative} is seeded as the predecessor's, yet this runtime claims it"
        );
    }
}

/// Two files in one directory, one on each side of the ownership boundary.
///
/// `data/` holds this port's journal beside the predecessor's databases, which
/// is the whole of what "shared by design" means here. The foreign set must
/// contain the predecessor's file and exclude this port's, and asserting both
/// directions is what makes the subtraction load-bearing rather than
/// decorative -- no command run by these tests writes an owned path, so nothing
/// else here would notice if the subtraction stopped subtracting.
#[test]
fn ownership_splits_inside_the_one_shared_data_directory() {
    let fixture = Fixture::seeded("split");
    let journal = fixture.state_root().join("data/spicejournal.sqlite3");
    fs::write(&journal, b"this port's own pages").expect("seed this port's journal");

    let foreign = fixture.foreign_identity();

    assert!(
        !foreign.contains_key(&journal),
        "this port's own journal was counted as the predecessor's state"
    );
    assert!(
        foreign.contains_key(&fixture.state_root().join("data/taskchampion.sqlite3")),
        "the predecessor's database in that same directory went missing from the foreign set"
    );
}

/// Running this port leaves every predecessor file byte-for-byte as found.
///
/// The run is first proved to have resolved this exact state root. That is the
/// assertion the unchanged-bytes comparison rests on: a binary that resolved
/// some other root would leave these files alone for the uninteresting reason
/// that it never looked at them, and the comparison would pass while proving
/// nothing at all.
#[test]
fn running_this_port_leaves_predecessor_state_byte_identical() {
    let fixture = Fixture::seeded("running");
    let before = fixture.foreign_identity();

    let output = fixture.invoke(&["doctor"]);
    assert!(output.status.success(), "doctor must run: {output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).expect("doctor emits JSON");
    let local = report["locations"]["local_configuration"]
        .as_str()
        .expect("doctor names the local configuration it would read");
    assert!(
        Path::new(local).starts_with(fixture.state_root()),
        "the runtime resolved {local}, so the comparison below would watch a root it never opened"
    );

    assert_eq!(
        fixture.foreign_identity(),
        before,
        "running this port changed state the predecessor owns"
    );
}

/// The one bridge that exists reads its export and writes nothing.
#[test]
fn the_bridge_leaves_predecessor_state_byte_identical() {
    let fixture = Fixture::seeded("bridge");
    let export = fixture.export();
    let before = fixture.foreign_identity();

    let output = fixture.invoke(&["config", "migrate-tasks", &export]);
    assert!(
        output.status.success(),
        "accepted export must land: {output:?}"
    );

    assert_eq!(
        fixture.foreign_identity(),
        before,
        "the bridge changed state the predecessor owns"
    );
}

/// Bridging is refused by default: it runs only on an operator-named export.
#[test]
fn the_bridge_refuses_without_an_operator_named_export() {
    let fixture = Fixture::seeded("refusal");
    let before = fixture.foreign_identity();

    let output = fixture.invoke(&["config", "migrate-tasks"]);

    assert!(
        !output.status.success(),
        "a bridge with no operator-named export must refuse: {output:?}"
    );
    assert_eq!(
        fixture.foreign_identity(),
        before,
        "a refused bridge still changed state the predecessor owns"
    );
}

/// `.spice/migrations/` is the predecessor's release ledger, not this port's.
///
/// This port names no migrations path anywhere, so the directory must stay
/// absent when the predecessor has not made one. A runtime that created it
/// would be writing into another system's ledger by name alone.
#[test]
fn the_predecessor_migrations_ledger_is_never_created_here() {
    let fixture = Fixture::bare("migrations");
    let migrations = fixture.state_root().join("migrations");
    assert!(!migrations.exists(), "the fixture starts without a ledger");

    let export = fixture.export();
    let output = fixture.invoke(&["config", "migrate-tasks", &export]);
    assert!(
        output.status.success(),
        "accepted export must land: {output:?}"
    );
    fixture.invoke(&["doctor"]);

    assert!(
        !migrations.exists(),
        "this runtime created the predecessor's migration ledger"
    );
}

/// One temporary repository whose shared state root stands in for the real one.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// A repository whose state root already holds the predecessor's files.
    fn seeded(label: &str) -> Self {
        let fixture = Self::bare(label);
        for (relative, bytes) in PREDECESSOR_STATE {
            let path = fixture.state_root().join(relative);
            fs::create_dir_all(path.parent().expect("seeded path has a parent"))
                .expect("create predecessor directory");
            fs::write(&path, bytes).expect("seed predecessor state");
        }
        fixture
    }

    /// A repository with no state root yet, as a fresh checkout has.
    fn bare(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-coexistence-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create coexistence fixture root");
        fs::create_dir_all(root.join(".git/info")).expect("create fixture git directory");
        fs::write(root.join(".git/info/exclude"), b"").expect("write fixture exclude");
        // Canonical from here on: the runtime canonicalizes while discovering,
        // and on macOS the temporary directory is reached through a symlink, so
        // an uncanonicalized root would never equal the paths it reports.
        let root = fs::canonicalize(&root).expect("canonicalize fixture root");
        Self { root }
    }

    fn state_root(&self) -> PathBuf {
        self.root
            .join(".git")
            .join(spice::repository::STATE_DIRECTORY)
    }

    /// Write the operator-named export outside the shared state root.
    fn export(&self) -> String {
        let path = self.root.join("export.json");
        fs::write(&path, EXPORT).expect("write predecessor export");
        path.display().to_string()
    }

    fn invoke(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(args)
            .current_dir(&self.root)
            .env("PATH", "")
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("run the spice binary")
    }

    /// Every path under the shared root this runtime declares as its own.
    fn owned(&self) -> Vec<PathBuf> {
        let repository = Repository::discover(&self.root).expect("fixture is a repository");
        let paths = repository.admit_state().expect("fixture state admits");
        paths
            .owned_directories()
            .into_iter()
            .chain(paths.owned_files())
            .map(Path::to_path_buf)
            .collect()
    }

    /// Identity of everything under the shared root this port does not own.
    ///
    /// The owned set is subtracted from the runtime's own declaration rather
    /// than from a list repeated here, so claiming one more name moves it out of
    /// this set automatically instead of leaving a stale copy to update by hand.
    fn foreign_identity(&self) -> BTreeMap<PathBuf, Entry> {
        let owned = self.owned();
        let mut found = BTreeMap::new();
        walk(&self.state_root(), &mut found);
        found.retain(|path, _| !owned.contains(path));
        found
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One filesystem entry compared by content, not merely by presence.
#[derive(Debug, Eq, PartialEq)]
enum Entry {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn walk(path: &Path, found: &mut BTreeMap<PathBuf, Entry>) {
    let children = match fs::read_dir(path) {
        Ok(children) => children,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => panic!("read {}: {error}", path.display()),
    };
    for child in children {
        let child = child.expect("read fixture entry").path();
        let metadata = fs::symlink_metadata(&child).expect("stat fixture entry");
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&child).expect("read fixture link");
            found.insert(child, Entry::Symlink(target));
        } else if metadata.is_dir() {
            found.insert(child.clone(), Entry::Directory);
            walk(&child, found);
        } else {
            let bytes = fs::read(&child).expect("read fixture file");
            found.insert(child, Entry::File(bytes));
        }
    }
}
