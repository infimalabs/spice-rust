//! Python Spice's durable state survives Rust Spice running beside it.
//!
//! `spice/src/repository.rs` states the coexistence policy; this file enforces
//! it. Everything here works by identity -- seed Python's complete `.spice`
//! root, run the real Rust binary against `.spice-rs`, read the bytes back --
//! rather than by asserting that no offending code path exists. A policy proved
//! by grepping for the absence of a call is only as good as the grep.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use spice::repository::{Repository, StatePaths};
use steer::ack::keys::Key;
use steer::directives::{Ledger, Steering};
use wire::body::SteerSent;
use wire::{CanonicalUuid, SpiceVariable};

/// Representative durable state Python Spice owns inside `.spice`.
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

/// Gate ledgers the predecessor owns and ordinary native commands must ignore.
///
/// These are deliberately not valid JSON. A native gate that tries to decode
/// even one of them refuses, so a successful real gate run proves that the
/// unchanged identity check below is observing an unreachable foreign surface,
/// not merely a compatible import that happened to preserve its inputs.
const PREDECESSOR_GATE_STATE: &[(&str, &[u8])] = &[
    ("file-loc-sticky.json", b"python file-loc state"),
    ("file-byte-sticky.json", b"python file-byte state"),
    ("complexity-ccn-sticky.json", b"python complexity-ccn state"),
    (
        "complexity-length-sticky.json",
        b"python complexity-length state",
    ),
    ("repo-doc-chars-sticky.json", b"python repo-doc state"),
    ("flex-slice-claims.json", b"python flex claims"),
];

/// Environment variables the predecessor owns for its live shell handoff.
const PREDECESSOR_HOOKS_ENV: &str = "SPICE_HOOKS"; // env-policy: allow
const PREDECESSOR_INBOX_ENV: &str = "SPICE_INBOX"; // env-policy: allow
const PREDECESSOR_LANE_ENV: &str = "SPICE_LANE"; // env-policy: allow
const PREDECESSOR_NATIVE_SHELL_ENV: &str = "SPICE_NATIVE_SHELL"; // env-policy: allow
const PREDECESSOR_ENVIRONMENT: &[&str] = &[
    PREDECESSOR_HOOKS_ENV,
    PREDECESSOR_INBOX_ENV,
    PREDECESSOR_LANE_ENV,
    PREDECESSOR_NATIVE_SHELL_ENV,
];

/// One accepted release-0.31.0 export, the only input the bridge admits.
const EXPORT: &[u8] = br#"{
  "version": 1,
  "release": "0.31.0",
  "tasks": {"hidden_stems": ["sandbox"]},
  "teams": []
}"#;

/// Rust Spice owns one complete root and no path below Python Spice's root.
#[test]
fn this_runtime_claims_no_path_the_predecessor_owns() {
    let fixture = Fixture::seeded("claims");
    let rust_root = fixture.state_root();
    let python_root = fixture.predecessor_root();

    assert_ne!(rust_root, python_root, "routing roots must be different");
    assert_eq!(
        fixture.paths().exclusive_directories(),
        [rust_root.as_path()],
        "the complete Rust state root must be exclusive"
    );

    for owned in fixture.owned() {
        assert!(
            owned.starts_with(&rust_root),
            "Rust claimed state outside its exclusive root: {}",
            owned.display()
        );
        assert!(
            !owned.starts_with(&python_root),
            "Rust claimed Python state: {}",
            owned.display()
        );
    }
}

/// Ordinary native gate capture has no implicit predecessor-state bridge.
///
/// The fixture seeds every predecessor gate ledger with bytes that the removed
/// importer would reject, records their exact identities, and invokes the real
/// `spice gate` command. The run must succeed without creating current gate
/// state, and every predecessor file must remain present and byte-identical.
#[test]
fn ordinary_gate_ignores_predecessor_gate_state() {
    let fixture = Fixture::seeded("gate-state");
    fixture.prepare_gate_repository();
    let before = fixture.predecessor_gate_identity();
    assert_eq!(before.len(), PREDECESSOR_GATE_STATE.len());

    let paths = fixture.paths();
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    assert_eq!(
        gate::current_state_path(repository.common_dir()),
        paths.gate_state(),
        "the gate writer and StatePaths registry must name one ledger"
    );
    let owned = fixture.owned();
    assert!(owned.contains(&paths.gate_state().to_path_buf()));
    assert!(owned.contains(&paths.gate_state_lock().to_path_buf()));
    assert!(
        !paths.gate_state().exists(),
        "current gate state starts absent"
    );
    assert!(
        !paths.gate_state_lock().exists(),
        "current gate-state lock starts absent"
    );

    let output = fixture.invoke(&["gate"]);
    assert!(output.status.success(), "native gate must run: {output:?}");
    assert_eq!(
        fixture.predecessor_gate_identity(),
        before,
        "ordinary native gate changed predecessor gate state"
    );
    assert!(
        !paths.gate_state().exists(),
        "gate capture created current state from predecessor ledgers"
    );
    assert!(
        !paths.gate_state_lock().exists(),
        "gate capture created a publication lock while only reading state"
    );
}

/// Identical relative names still resolve to physically different places.
#[test]
fn state_roots_are_siblings_and_database_names_cannot_cross() {
    let fixture = Fixture::seeded("roots");
    let relative = "data/spiceacks.sqlite3";
    let python_database = fixture.predecessor_root().join(relative);
    let rust_database = fixture.state_root().join(relative);
    fs::create_dir_all(rust_database.parent().expect("database has a parent"))
        .expect("create Rust data directory");
    fs::write(&rust_database, b"Rust namespace sentinel").expect("seed Rust sentinel");

    assert_eq!(
        fs::read(python_database).expect("read Python database"),
        b"ack pages"
    );
    assert_eq!(
        fs::read(rust_database).expect("read Rust sentinel"),
        b"Rust namespace sentinel"
    );
}

/// A dynamically named Rust file remains outside Python's complete snapshot.
#[test]
fn ownership_covers_unnameable_files_inside_an_exclusive_directory() {
    let fixture = Fixture::seeded("exclusive");
    let before = fixture.predecessor_identity();
    let lock = fixture.paths().locks().join("journal.lock");
    fs::create_dir_all(lock.parent().expect("lock has a parent")).expect("create lock directory");
    fs::write(&lock, b"this port's own lock").expect("seed this port's lock");

    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "a dynamically named Rust file changed Python's state tree"
    );
}

/// Live Rust shell handoff variables are not the predecessor's names.
#[test]
fn runtime_environment_names_are_rust_specific() {
    let rust_environment = [
        steer::hook::HOOKS,
        steer::hook::INBOX,
        steer::hook::LANE,
        steer::hook::NATIVE_SHELL,
    ];

    assert_eq!(
        rust_environment,
        [
            "SPICE_RS_HOOKS",
            "SPICE_RS_INBOX",
            "SPICE_RS_LANE",
            "SPICE_RS_NATIVE_SHELL",
        ]
    );
    assert_eq!(SpiceVariable::Hooks.as_str(), steer::hook::HOOKS);
    assert_eq!(SpiceVariable::Inbox.as_str(), steer::hook::INBOX);
    assert_eq!(SpiceVariable::Lane.as_str(), steer::hook::LANE);
    assert_eq!(
        SpiceVariable::NativeShell.as_str(),
        steer::hook::NATIVE_SHELL
    );
    assert_eq!(spice::agent_launch::SPICE_RS_LANE, steer::hook::LANE);
    for name in rust_environment {
        assert!(
            !PREDECESSOR_ENVIRONMENT.contains(&name),
            "Rust environment name collides with predecessor variable {name}"
        );
    }
}

/// A predecessor lane with the same UUID does not activate this runtime.
#[test]
fn predecessor_lane_environment_does_not_activate_rust_hook_context() {
    let fixture = Fixture::seeded("lane-environment");
    let before = fixture.predecessor_identity();
    let lane = "018f82d4-6f59-7d06-92e3-bce9131163c7";

    let output = fixture.invoke_with_predecessor_environment(&["--json", "hook", "claude"], lane);

    assert!(
        !output.status.success(),
        "predecessor lane context must not satisfy Rust hook admission: {output:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("steer::lane-context-absent")
            || stderr.contains("steer::lane-context-absent"),
        "refusal must be missing Rust lane context, stdout={stdout:?} stderr={stderr:?}",
    );
    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "refused Rust hook read or changed predecessor state"
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
    let before = fixture.predecessor_identity();

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
        fixture.predecessor_identity(),
        before,
        "running this port changed state the predecessor owns"
    );
}

/// Installing and uninstalling leaves every predecessor file as found.
///
/// Install writes its receipt only below the Rust root.
#[test]
fn installing_this_port_leaves_predecessor_state_byte_identical() {
    let fixture = Fixture::seeded("install");
    let before = fixture.predecessor_identity();

    let preview = fixture.invoke(&["--json", "install"]);
    assert!(preview.status.success(), "install preview: {preview:?}");
    let preview: Value = serde_json::from_slice(&preview.stdout).expect("preview is JSON");
    let digest = preview["body"]["digest"]
        .as_str()
        .expect("preview names the plan digest an operator must echo back");

    let applied = fixture.invoke(&["install", &format!("--apply={digest}")]);
    assert!(applied.status.success(), "install apply: {applied:?}");

    let receipt = fixture.paths().install_record().to_path_buf();
    assert!(
        receipt.is_file(),
        "install wrote no receipt at the path this runtime claims"
    );
    assert!(receipt.starts_with(fixture.state_root()));
    assert!(!receipt.starts_with(fixture.predecessor_root()));
    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "installing this port changed state the predecessor owns"
    );

    let removed = fixture.invoke(&["install", "--unapply"]);
    assert!(removed.status.success(), "install unapply: {removed:?}");
    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "uninstalling this port changed state the predecessor owns"
    );
}

/// The one bridge that exists reads its export and writes nothing.
#[test]
fn the_bridge_leaves_predecessor_state_byte_identical() {
    let fixture = Fixture::seeded("bridge");
    let export = fixture.export();
    let before = fixture.predecessor_identity();

    let output = fixture.invoke(&["config", "migrate-tasks", &export]);
    assert!(
        output.status.success(),
        "accepted export must land: {output:?}"
    );

    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "the bridge changed state the predecessor owns"
    );
}

/// Bridging is refused by default: it runs only on an operator-named export.
#[test]
fn the_bridge_refuses_without_an_operator_named_export() {
    let fixture = Fixture::seeded("refusal");
    let before = fixture.predecessor_identity();

    let output = fixture.invoke(&["config", "migrate-tasks"]);

    assert!(
        !output.status.success(),
        "a bridge with no operator-named export must refuse: {output:?}"
    );
    assert_eq!(
        fixture.predecessor_identity(),
        before,
        "a refused bridge still changed state the predecessor owns"
    );
}

/// Python's `.spice/migrations/` is not Rust Spice's release ledger.
///
/// This port names no migrations path anywhere, so the directory must stay
/// absent when the predecessor has not made one. A runtime that created it
/// would be writing into another system's ledger by name alone.
#[test]
fn the_predecessor_migrations_ledger_is_never_created_here() {
    let fixture = Fixture::bare("migrations");
    let migrations = fixture.predecessor_root().join("migrations");
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

/// Tracked paths spelling the Rust state layout cannot become inbox or ACK
/// authority, and writes to the real Git-owned surfaces remain unstageable.
///
/// The impostors are committed first so this is stronger than an assertion
/// over path strings: production publishes a real sealed rendering and opens
/// the real journal while the committed bytes stay unchanged. A subsequent
/// `git add -A` observes neither authority write because both are inside Git's
/// own directory rather than the worktree tree.
#[test]
fn repository_lookalikes_cannot_replace_or_stage_inbox_and_ack_authority() {
    let fixture = Fixture::bare("inbox-authority");
    let lane: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce913116302"
        .parse()
        .expect("fixture lane");
    let impostor_inbox = fixture.root.join(".spice-rs/inbox").join(lane.to_string());
    let impostor_journal = fixture.root.join(".spice-rs/data/spicejournal.sqlite3");
    fs::create_dir_all(impostor_inbox.parent().expect("impostor inbox parent"))
        .expect("create impostor inbox parent");
    fs::create_dir_all(impostor_journal.parent().expect("impostor journal parent"))
        .expect("create impostor journal parent");
    fs::write(&impostor_inbox, b"repository-injected direction\n").expect("write impostor inbox");
    fs::write(&impostor_journal, b"repository-injected acknowledgment\n")
        .expect("write impostor journal");
    git(&fixture.root, &["add", ".spice-rs"]);
    git(&fixture.root, &["commit", "-qm", "tracked lookalikes"]);

    let paths = fixture.paths();
    let impostor_root = fixture.root.join(".spice-rs");
    assert!(!paths.inbox().starts_with(&impostor_root));
    assert!(!paths.journal().starts_with(&impostor_root));
    let mut ledger = Ledger::new();
    ledger
        .absorb(Steering::Sent(&SteerSent {
            directive: "018f82d4-6f59-7d06-92e3-bce913116399"
                .parse()
                .expect("fixture directive"),
            key: "1kJK6M0V".to_owned(),
            lane,
            text: "authoritative rendered direction".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires: "2026-08-11T07:07:19.000Z".parse().expect("fixture expiry"),
        }))
        .expect("fixture directive absorbs");
    let seal: Key = "1kJK6N3R".parse().expect("fixture seal");
    steer::inbox::publish(paths.inbox(), lane, &seal, &ledger.pending(lane))
        .expect("authoritative inbox publishes");
    fs::create_dir_all(paths.data()).expect("create authoritative journal parent");
    drop(journal::Journal::open(paths.journal()).expect("ACK authority journal opens"));

    assert_eq!(
        fs::read(&impostor_inbox).expect("impostor inbox remains readable"),
        b"repository-injected direction\n"
    );
    assert_eq!(
        fs::read(&impostor_journal).expect("impostor journal remains readable"),
        b"repository-injected acknowledgment\n"
    );
    assert!(
        fs::read_to_string(steer::inbox::path(paths.inbox(), lane))
            .expect("authoritative inbox reads")
            .contains("authoritative rendered direction")
    );
    git(&fixture.root, &["add", "-A"]);
    let status = git_output(
        &fixture.root,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert!(
        status.stdout.is_empty(),
        "Git observed authority state as stageable: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

/// One temporary repository with physically separate Python and Rust roots.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// A repository whose state root already holds the predecessor's files.
    fn seeded(label: &str) -> Self {
        let fixture = Self::bare(label);
        for (relative, bytes) in PREDECESSOR_STATE.iter().chain(PREDECESSOR_GATE_STATE) {
            let path = fixture.predecessor_root().join(relative);
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
        git(&root, &["init", "-q"]);
        git(
            &root,
            &["config", "user.email", "coexistence@example.invalid"],
        );
        git(&root, &["config", "user.name", "Coexistence Fixture"]);
        // Canonical from here on: the runtime canonicalizes while discovering,
        // and on macOS the temporary directory is reached through a symlink, so
        // an uncanonicalized root would never equal the paths it reports.
        let root = fs::canonicalize(&root).expect("canonicalize fixture root");
        Self { root }
    }

    /// Add the smallest tracked repository admitted by the full native gate.
    fn prepare_gate_repository(&self) {
        fs::create_dir_all(self.root.join("src")).expect("create gate source directory");
        fs::create_dir_all(self.root.join("gate/src")).expect("create camp loci");
        fs::write(
            self.root.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write gate policy");
        fs::write(
            self.root.join("baseline.toml"),
            "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
        )
        .expect("write gate baseline");
        fs::write(
            self.root.join("Cargo.toml"),
            "[package]\nname = \"coexistence-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write fixture manifest");
        fs::write(
            self.root.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "# It is not intended for manual editing.\n",
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"coexistence-fixture\"\n",
                "version = \"0.0.0\"\n",
            ),
        )
        .expect("write fixture lockfile");
        fs::write(self.root.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write gate source");
        for path in [
            "gate/src/camp.rs",
            "gate/src/input.rs",
            "gate/src/kill.rs",
            "gate/src/verdict.rs",
        ] {
            fs::write(self.root.join(path), "// coexistence fixture\n").expect("write camp locus");
        }
        git(&self.root, &["add", "."]);
        git(&self.root, &["commit", "-qm", "fixture"]);
    }

    fn state_root(&self) -> PathBuf {
        self.root
            .join(".git")
            .join(spice::repository::STATE_DIRECTORY)
    }

    /// Python Spice's state root, which Rust Spice treats as foreign.
    fn predecessor_root(&self) -> PathBuf {
        self.root.join(".git").join(".spice")
    }

    /// Write the operator-named export outside both state roots.
    fn export(&self) -> String {
        let path = self.root.join("export.json");
        fs::write(&path, EXPORT).expect("write predecessor export");
        path.display().to_string()
    }

    fn invoke(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command.args(args).output().expect("run the spice binary")
    }

    fn invoke_with_predecessor_environment(&self, args: &[&str], lane: &str) -> Output {
        let mut command = self.command();
        command
            .args(args)
            .env(PREDECESSOR_HOOKS_ENV, self.predecessor_root().join("hooks"))
            .env(
                PREDECESSOR_INBOX_ENV,
                self.predecessor_root().join("inbox").join(lane),
            )
            .env(PREDECESSOR_LANE_ENV, lane)
            .env(PREDECESSOR_NATIVE_SHELL_ENV, "/bin/sh")
            .output()
            .expect("run the spice binary with predecessor environment")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .current_dir(&self.root)
            .env("PATH", "")
            .env_remove(steer::hook::HOOKS)
            .env_remove(steer::hook::INBOX)
            .env_remove(steer::hook::LANE)
            .env_remove(steer::hook::NATIVE_SHELL)
            .env_remove(PREDECESSOR_HOOKS_ENV)
            .env_remove(PREDECESSOR_INBOX_ENV)
            .env_remove(PREDECESSOR_LANE_ENV)
            .env_remove(PREDECESSOR_NATIVE_SHELL_ENV);
        command
    }

    /// The runtime's own declaration of where its state lives.
    fn paths(&self) -> StatePaths {
        let repository = Repository::discover(&self.root).expect("fixture is a repository");
        repository.admit_state().expect("fixture state admits")
    }

    /// Every path this runtime declares as its own.
    fn owned(&self) -> Vec<PathBuf> {
        let paths = self.paths();
        paths
            .owned_directories()
            .into_iter()
            .chain(paths.owned_files())
            .map(Path::to_path_buf)
            .collect()
    }

    /// Exact identities of the six predecessor gate-state files.
    fn predecessor_gate_identity(&self) -> BTreeMap<PathBuf, Entry> {
        PREDECESSOR_GATE_STATE
            .iter()
            .map(|(relative, _)| {
                let path = self.predecessor_root().join(relative);
                let bytes = fs::read(&path).expect("read predecessor gate state");
                (path, Entry::File(bytes))
            })
            .collect()
    }

    /// Complete byte-level identity of Python Spice's foreign state root.
    fn predecessor_identity(&self) -> BTreeMap<PathBuf, Entry> {
        let mut found = BTreeMap::new();
        walk(&self.predecessor_root(), &mut found);
        found
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = git_output(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(root: &Path, arguments: &[&str]) -> Output {
    Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("coexistence fixture Git launches")
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
