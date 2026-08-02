#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant as Monotonic};

use sill::{Acquisition, LockFile, LockName};
use wire::Outcome;

const READINESS_HORIZON: Duration = Duration::from_secs(10);

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    ambient_bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-coexistence-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create exclusive coexistence fixture");
        let initialized = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .expect("Git launches for coexistence fixture");
        assert!(initialized.success(), "Git initializes coexistence fixture");

        fs::write(root.join("spice.toml"), include_bytes!("../../spice.toml"))
            .expect("plant tracked Rust configuration");
        plant_python_authority(&root);
        plant_active_hooks(&root);

        let configured = Command::new("git")
            .args(["config", "core.hooksPath", ".spice/hooks"])
            .current_dir(&root)
            .status()
            .expect("Git config launches for coexistence fixture");
        assert!(configured.success(), "fixture activates Python hooks");

        let ambient_bin = root.join("ambient-bin");
        fs::create_dir(&ambient_bin).expect("create ambient command directory");
        let ambient_spice = ambient_bin.join("spice");
        fs::write(
            &ambient_spice,
            b"#!/bin/sh\nprintf 'python-spice-sentinel\\n'\n",
        )
        .expect("plant ambient Python command");
        make_executable(&ambient_spice);

        Self { root, ambient_bin }
    }

    fn common(&self) -> PathBuf {
        self.root.join(".git")
    }

    fn python_authority(&self) -> PathBuf {
        self.common().join(".spice")
    }

    fn hooks(&self) -> PathBuf {
        self.root.join(".spice/hooks")
    }

    fn rust_state(&self) -> PathBuf {
        self.common().join(".spice-rust")
    }

    fn discovery(&self) -> canvas::Discovery {
        wire::decode(&fs::read(self.rust_state().join("canvas")).expect("read Rust discovery"))
            .expect("decode Rust discovery")
    }

    fn admitted_rust_state(&self) -> PathBuf {
        fs::canonicalize(self.common())
            .expect("canonical Git-common directory")
            .join(".spice-rust")
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        self.command(arguments)
            .output()
            .expect("explicit Rust command launches")
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(arguments)
            .current_dir(&self.root)
            .env("PATH", &self.ambient_bin)
            .env_remove(steer::hook::INBOX);
        command
    }

    fn ambient_spice(&self) -> PathBuf {
        let output = Command::new("/bin/sh")
            .args(["-c", "command -v spice"])
            .current_dir(&self.root)
            .env("PATH", &self.ambient_bin)
            .output()
            .expect("shell resolves ambient Spice command");
        assert!(output.status.success(), "ambient Spice command resolves");
        assert!(output.stderr.is_empty(), "ambient resolution is quiet");
        PathBuf::from(
            String::from_utf8(output.stdout)
                .expect("ambient path is UTF-8")
                .trim(),
        )
    }

    fn hooks_path(&self) -> String {
        let output = Command::new("git")
            .args(["config", "--get", "core.hooksPath"])
            .current_dir(&self.root)
            .output()
            .expect("Git reads active hook path");
        assert!(output.status.success(), "active hook path is configured");
        String::from_utf8(output.stdout)
            .expect("hook path is UTF-8")
            .trim()
            .to_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove coexistence fixture");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = fixture
            .command(&["--json", "up"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Rust resident starts");
        Self { child: Some(child) }
    }

    fn wait_until_ready(&mut self, fixture: &Fixture) {
        let deadline = Monotonic::now() + READINESS_HORIZON;
        loop {
            assert!(
                self.child
                    .as_mut()
                    .expect("resident child exists")
                    .try_wait()
                    .expect("resident child status")
                    .is_none(),
                "resident exited before publishing Rust discovery",
            );
            if fixture.rust_state().join("canvas").is_file() {
                return;
            }
            assert!(
                Monotonic::now() < deadline,
                "resident did not publish discovery within {READINESS_HORIZON:?}",
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("resident child exists")
            .wait_with_output()
            .expect("resident exits after down")
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

#[derive(Debug, Eq, PartialEq)]
struct Entry {
    bytes: Vec<u8>,
    mode: u32,
    link: Option<PathBuf>,
}

#[test]
fn explicit_rust_lifecycle_uses_only_isolated_state() {
    let fixture = Fixture::new();
    let ambient_before = fixture.ambient_spice();
    let hook_setting_before = fixture.hooks_path();
    let python_before = snapshot(&fixture.python_authority());
    let hooks_before = snapshot(&fixture.hooks());
    let repository_before = snapshot(&fixture.root);

    assert_ne!(ambient_before, PathBuf::from(env!("CARGO_BIN_EXE_spice")));
    assert_eq!(hook_setting_before, ".spice/hooks");
    assert!(!fixture.rust_state().exists());

    assert_reply(fixture.invoke(&["--json", "doctor"]), "doctor");
    assert!(
        !fixture.rust_state().exists(),
        "daemonless Doctor created Rust state",
    );

    let mut resident = Resident::start(&fixture);
    resident.wait_until_ready(&fixture);
    let first_epoch = fixture.discovery().epoch;
    assert_reply(fixture.invoke(&["--json", "version"]), "version");
    assert_reply(fixture.invoke(&["--json", "board"]), "board");
    assert_reply(
        fixture.invoke(&["--json", "task", "add", "coexistence smoke"]),
        "task add",
    );
    assert_reply(fixture.invoke(&["--json", "lane"]), "lane");
    assert_reply(fixture.invoke(&["--json", "inbox"]), "inbox");
    assert_reply(fixture.invoke(&["--json", "down"]), "down");
    assert_reply(resident.wait(), "up");

    let mut restarted = Resident::start(&fixture);
    restarted.wait_until_ready(&fixture);
    assert_ne!(fixture.discovery().epoch, first_epoch);
    assert_text_reply(fixture.invoke(&["version"]), "restarted version");
    assert_text_reply(fixture.invoke(&["board"]), "restarted board");
    assert_text_reply(fixture.invoke(&["down"]), "restarted down");
    assert_reply(restarted.wait(), "restarted up");

    let resident_lock = LockFile::new(
        LockName::new("resident").expect("resident lock name is valid"),
        fixture.rust_state().join("resident"),
    );
    let Acquisition::Acquired(possession) = resident_lock
        .try_acquire()
        .expect("released resident lock can be inspected")
    else {
        panic!("resident lock remained held after down");
    };
    drop(possession);

    let reader =
        journal::JournalReader::open(fixture.rust_state().join("data/spicejournal.sqlite3"))
            .expect("Rust journal is finalized and readable");
    assert_eq!(
        reader.schema_version().expect("Rust schema is readable"),
        journal::SCHEMA_VERSION,
    );
    assert!(!fixture.rust_state().join("canvas").exists());

    assert_eq!(snapshot(&fixture.python_authority()), python_before);
    assert_eq!(snapshot(&fixture.hooks()), hooks_before);
    assert_eq!(fixture.hooks_path(), hook_setting_before);
    assert_eq!(fixture.ambient_spice(), ambient_before);

    let repository_after = snapshot(&fixture.root);
    for (path, entry) in &repository_before {
        assert_eq!(
            repository_after.get(path),
            Some(entry),
            "pre-existing fixture file changed: {}",
            path.display(),
        );
    }
    let additions: Vec<_> = repository_after
        .keys()
        .filter(|path| !repository_before.contains_key(*path))
        .collect();
    assert!(
        !additions.is_empty(),
        "Rust lifecycle produced no durable state"
    );
    assert!(
        additions
            .iter()
            .all(|path| path.starts_with(".git/.spice-rust")),
        "Rust lifecycle created files outside its state root: {additions:?}",
    );
}

#[test]
fn a_rust_root_linked_to_python_authority_refuses_every_state_entry_point() {
    let fixture = Fixture::new();
    symlink(".spice", fixture.rust_state()).expect("link Rust root to Python authority");
    let before = snapshot(&fixture.root);
    let expected = fixture.admitted_rust_state();

    for arguments in [
        ["--json", "up"],
        ["--json", "version"],
        ["--json", "down"],
        ["--json", "canvas"],
        ["--json", "lock"],
        ["--json", "proof"],
    ] {
        assert_state_refusal(fixture.invoke(&arguments), &expected, arguments[1]);
    }

    assert_eq!(
        snapshot(&fixture.root),
        before,
        "a linked Rust root changed Python authority, hooks, or worktree state"
    );
}

#[test]
fn linked_state_components_refuse_before_any_outside_mutation() {
    for (component, command) in [
        ("data", "version"),
        ("locks", "lock"),
        ("proof", "proof"),
        ("resident", "down"),
        ("canvas", "canvas"),
        ("degraded", "up"),
        ("tape", "up"),
    ] {
        let fixture = Fixture::new();
        fs::create_dir(fixture.rust_state()).expect("create real Rust state root");
        let outside = fixture.root.join(format!("outside-{component}"));
        fs::create_dir(&outside).expect("create outside target");
        fs::write(outside.join("sentinel"), format!("outside {component}\n"))
            .expect("plant outside sentinel");
        symlink(&outside, fixture.rust_state().join(component))
            .expect("link fixed state component outside");
        let before = snapshot(&fixture.root);
        let expected = fixture.admitted_rust_state().join(component);

        assert_state_refusal(fixture.invoke(&["--json", command]), &expected, command);
        assert_eq!(
            snapshot(&fixture.root),
            before,
            "{command} mutated state through linked {component}"
        );
    }
}

fn assert_reply(output: Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stderr.is_empty(), "{command} wrote stderr");
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&output.stdout).expect("Rust command emits one typed outcome");
    assert!(
        matches!(outcome, Outcome::Reply(_)),
        "{command} did not reply: {outcome:?}",
    );
}

fn assert_text_reply(output: Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stderr.is_empty(), "{command} wrote stderr");
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .unwrap_or_else(|error| panic!("{command} text reply did not decode: {error}"));
}

fn assert_state_refusal(output: Output, expected: &Path, command: &str) {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{command} did not refuse: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.stdout.is_empty(),
        "{command} wrote refusal to stdout"
    );
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("state refusal is one typed outcome");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("{command} did not emit a refusal: {outcome:?}");
    };
    assert_eq!(refusal.reason, "repository::state-path-refused");
    assert_eq!(refusal.witness.as_deref(), Some("repository-state-root"));
    let observed = refusal.observed.expect("state refusal names observation");
    assert!(
        observed.contains(&expected.display().to_string()),
        "{command} refusal did not name {}: {observed}",
        expected.display()
    );
}

fn plant_python_authority(root: &Path) {
    let authority = root.join(".git/.spice/data");
    fs::create_dir_all(&authority).expect("create Python authority directory");
    fs::write(
        authority.join("spicejournal.sqlite3"),
        b"python authority must remain opaque\n",
    )
    .expect("plant Python authority sentinel");
    fs::write(
        root.join(".git/.spice/resident"),
        b"python resident sentinel\n",
    )
    .expect("plant Python runtime sentinel");
}

fn plant_active_hooks(root: &Path) {
    let hooks = root.join(".spice/hooks");
    fs::create_dir_all(&hooks).expect("create active Python hook directory");
    for hook in ["pre-commit", "commit-msg", "reference-transaction"] {
        let path = hooks.join(hook);
        fs::write(&path, format!("#!/bin/sh\nexec spice dev {hook} \"$@\"\n"))
            .expect("plant active Python hook");
        make_executable(&path);
    }
}

fn make_executable(path: &Path) {
    let mut permissions = fs::metadata(path)
        .expect("read executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("set executable mode");
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut entries = BTreeMap::new();
    collect(root, root, &mut entries);
    entries
}

fn collect(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
    for entry in fs::read_dir(directory).expect("read snapshot directory") {
        let path = entry.expect("read snapshot entry").path();
        let metadata = fs::symlink_metadata(&path).expect("read snapshot metadata");
        if metadata.file_type().is_symlink() {
            entries.insert(
                path.strip_prefix(root)
                    .expect("snapshot link remains under root")
                    .to_path_buf(),
                Entry {
                    bytes: Vec::new(),
                    mode: metadata.permissions().mode(),
                    link: Some(fs::read_link(&path).expect("read snapshot link")),
                },
            );
        } else if metadata.is_dir() {
            entries.insert(
                path.strip_prefix(root)
                    .expect("snapshot directory remains under root")
                    .to_path_buf(),
                Entry {
                    bytes: Vec::new(),
                    mode: metadata.permissions().mode(),
                    link: None,
                },
            );
            collect(root, &path, entries);
        } else {
            entries.insert(
                path.strip_prefix(root)
                    .expect("snapshot entry remains under root")
                    .to_path_buf(),
                Entry {
                    bytes: fs::read(&path).expect("read snapshot bytes"),
                    mode: metadata.permissions().mode(),
                    link: None,
                },
            );
        }
    }
}
