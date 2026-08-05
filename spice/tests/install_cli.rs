#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use spice::install::{InstallPreview, InstallReceipt, InstallStatus};
use wire::Outcome;

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

/// How long a paused install is given to reach its mutation rendezvous.
const RENDEZVOUS: Duration = Duration::from_secs(10);

static PAUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    python: PathBuf,
    python_bytes: Vec<u8>,
    tracked_bytes: Vec<u8>,
    exclude_bytes: Vec<u8>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-install-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(".git/info")).expect("create repository metadata");
        fs::create_dir_all(root.join(".git/.spice/data"))
            .expect("create Python authority sentinel directory");
        fs::create_dir_all(root.join("bin")).expect("create predecessor bin directory");
        let exclude_bytes = b"# python-owned exclusions\n/cache/\n".to_vec();
        fs::write(root.join(".git/info/exclude"), &exclude_bytes).expect("write exclusions");
        fs::write(
            root.join(".git/.spice/data/spicejournal.sqlite3"),
            b"python authority sentinel\n",
        )
        .expect("write Python authority sentinel");
        fs::write(
            root.join(".git/.spice/config.toml"),
            b"[operator]\nmode = 'python'\n",
        )
        .expect("write Python local configuration sentinel");
        let tracked_bytes = b"[tasks]\nmax_parallel = 7\n\n[policy]\nowner = \"python\"\n".to_vec();
        fs::write(root.join("spice.toml"), &tracked_bytes).expect("write tracked intent");
        let python = root.join("bin/spice");
        let python_bytes = b"#!/bin/sh\nprintf 'python-spice\\n'\n".to_vec();
        fs::write(&python, &python_bytes).expect("write predecessor entrypoint");
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755))
            .expect("make predecessor entrypoint executable");
        Self {
            root,
            python,
            python_bytes,
            tracked_bytes,
            exclude_bytes,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Spoken {
        let output = self
            .command(arguments)
            .output()
            .expect("spice install launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(arguments)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX);
        command
    }

    fn spawn_paused(
        &self,
        arguments: &[&str],
        action: &str,
        path: PathBuf,
        release: Release,
    ) -> Paused {
        let relative = path
            .strip_prefix(&self.root)
            .expect("paused install path belongs to the fixture");
        let path = fs::canonicalize(&self.root)
            .expect("canonicalize install fixture")
            .join(relative);
        let control = self.root.join(format!(
            ".install-pause-{}",
            PAUSE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&control).expect("create install race rendezvous");
        fs::create_dir(control.join("ready")).expect("create install readiness rendezvous");
        fs::create_dir(control.join("release")).expect("create install release rendezvous");
        if matches!(release, Release::Landed) {
            fs::write(control.join("release").join("continue"), b"continue")
                .expect("land install release before the wait arms");
        }
        let child = self
            .command(arguments)
            .env(spice::install::TEST_PAUSE_CONTROL, &control)
            .env(spice::install::TEST_PAUSE_ACTION, action)
            .env(spice::install::TEST_PAUSE_PATH, path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn paused install process");
        let mut paused = Paused {
            child: Some(child),
            control,
        };
        paused.wait_until_ready();
        paused
    }

    fn assert_python_authority_unchanged(&self) {
        assert_eq!(fs::read(&self.python).unwrap(), self.python_bytes);
        assert_eq!(
            fs::read(self.root.join("spice.toml")).unwrap(),
            self.tracked_bytes
        );
        assert_eq!(
            fs::read(self.root.join(".git/.spice/data/spicejournal.sqlite3")).unwrap(),
            b"python authority sentinel\n"
        );
        assert_eq!(
            fs::read(self.root.join(".git/.spice/config.toml")).unwrap(),
            b"[operator]\nmode = 'python'\n"
        );
        let mut entries = fs::read_dir(self.root.join(".git/.spice"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["config.toml", "data"]);
    }

    fn installed_binary(&self) -> PathBuf {
        self.root.join(".spice/rust/bin/spice-rust")
    }

    fn record(&self) -> PathBuf {
        self.root.join(".git/.spice-rust/install.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root)
            && !std::thread::panicking()
        {
            panic!("remove {}: {error}", self.root.display());
        }
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

/// Whether the release a paused install waits for is already in place.
///
/// The disposition is stated at the spawn rather than inferred from the ending,
/// because it decides what the wait can possibly observe: a release landed
/// before its directory is watched produces no notification the wait can
/// receive.
enum Release {
    /// Nothing has written `release/continue` yet, so the wait arms on an
    /// absent release and the harness decides the ending.
    Withheld,
    /// `release/continue` is in place before the install is spawned.
    Landed,
}

struct Paused {
    child: Option<Child>,
    control: PathBuf,
}

impl Paused {
    fn wait_until_ready(&mut self) {
        let ready = self.control.join("ready");
        let rendezvous = ready.join("arrived");
        let waited = readiness::wait_until_ready(
            self.child.as_mut().expect("paused child is present"),
            &ready,
            RENDEZVOUS,
            "install mutation rendezvous",
            || rendezvous.is_file(),
        );
        if matches!(waited, Readiness::Exited) {
            let output = self
                .child
                .take()
                .expect("paused child is present")
                .wait_with_output()
                .expect("collect early install exit");
            panic!(
                "install exited before rendezvous: status={:?}, stdout={}, stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    fn resume(self) -> Spoken {
        fs::write(self.control.join("release").join("continue"), b"continue")
            .expect("release install mutation rendezvous");
        self.collect()
    }

    /// Collect a paused install that is never released.
    ///
    /// The only difference from `resume` is the release that does not happen,
    /// so what is collected here is whatever ending the install reaches on its
    /// own -- which is the horizon bounding its wait.
    fn abandon(self) -> Spoken {
        self.collect()
    }

    fn collect(mut self) -> Spoken {
        let output = self
            .child
            .take()
            .expect("paused child is present")
            .wait_with_output()
            .expect("collect paused install process");
        fs::remove_dir_all(&self.control).expect("remove install race rendezvous");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }
}

#[test]
fn preview_is_zero_write_and_apply_requires_its_exact_digest() {
    let fixture = Fixture::new("digest");
    let text = fixture.invoke(&["install"]);
    let json = fixture.invoke(&["--json", "install"]);
    let text_document = successful_document(text);
    let text_digest = emitted_digest(&text_document).to_owned();
    let text: InstallPreview =
        serde_json::from_value(text_document).expect("text preview body decodes");
    let json_document = successful_document(json);
    assert_eq!(json_document["kind"], "reply");
    assert!(json_document.get("direct").is_some());
    let json_body = json_document["body"].clone();
    let json_digest = emitted_digest(&json_body).to_owned();
    let preview: InstallPreview =
        serde_json::from_value(json_body).expect("JSON preview body decodes");
    assert_eq!(text, preview);
    assert_eq!(text_digest, json_digest);
    assert_eq!(preview.digest().as_str(), text_digest);
    assert_eq!(preview.digest(), &preview.plan().digest().unwrap());
    let operation_count = preview.plan().operations().len();
    assert_eq!(operation_count, 8);
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    assert!(!fixture.root.join(".git/.spice-rust").exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_python_authority_unchanged();

    let malformed = fixture.invoke(&["--json", "install", "--apply=not-a-digest"]);
    assert_eq!(malformed.code, Some(2));
    assert!(malformed.out.is_empty());
    assert_refusal(&malformed.err, "install::digest-refused");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());

    let moved = alternate_digest(&text_digest);
    let refused = fixture.invoke(&["--json", "install", &format!("--apply={moved}")]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::digest-moved");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_python_authority_unchanged();

    let applied: InstallReceipt =
        reply_body(fixture.invoke(&["--json", "install", &format!("--apply={text_digest}")]));
    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(applied.completed(), operation_count);
    assert_eq!(
        fs::read(fixture.installed_binary()).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_spice")).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_restores_exact_prior_images_and_keeps_the_namespaced_receipt() {
    let fixture = Fixture::new("unapply");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let operation_count = preview.plan().operations().len();
    let digest = preview.digest();
    let applied = fixture.invoke(&["install", &format!("--apply={digest}")]);
    assert_eq!(applied.code, Some(0));
    assert!(applied.err.is_empty());

    let unapplied: InstallReceipt = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert_eq!(unapplied.completed(), operation_count);
    assert!(!fixture.installed_binary().exists());
    assert!(fixture.record().is_file());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_python_authority_unchanged();

    let python = Command::new(&fixture.python)
        .output()
        .expect("run Python entrypoint");
    assert!(python.status.success());
    assert_eq!(python.stdout, b"python-spice\n");
}

#[test]
fn drift_refuses_reversal_without_overwriting_the_changed_target() {
    let fixture = Fixture::new("drift");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    assert_eq!(
        fixture
            .invoke(&["install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let entry = fixture.root.join(".spice/rust/hooks/entry.sh");
    fs::write(&entry, b"operator drift\n").expect("drift installed entry hook");

    let refused = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::drift");
    assert_eq!(fs::read(entry).unwrap(), b"operator drift\n");
    assert!(fixture.record().is_file());
    fixture.assert_python_authority_unchanged();
}

#[test]
#[ignore = "workstation acceptance: requires the installed Python entrypoint"]
fn ambient_python_default_coexists_with_the_explicit_installed_rust_binary() {
    let python = PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
        .join("Users")
        .join("c")
        .join(".local/bin/spice");
    assert!(
        python.is_file(),
        "{} is the Python entrypoint",
        python.display()
    );
    let python_link = fs::read_link(&python).expect("ambient Python entrypoint is a symlink");
    let python_bytes = fs::read(&python).expect("snapshot ambient Python entrypoint");
    let fixture = Fixture::new("ambient-coexistence");

    let before = Command::new(&python)
        .arg("--help")
        .current_dir(&fixture.root)
        .output()
        .expect("ambient Python entrypoint launches before Rust installation");
    assert!(before.status.success());
    assert!(before.stderr.is_empty());

    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let applied: InstallReceipt = reply_body(fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", preview.digest()),
    ]));
    assert_eq!(applied.status(), InstallStatus::Complete);

    let rust = fixture.installed_binary();
    let explicit = Command::new(&rust)
        .arg("--help")
        .current_dir(&fixture.root)
        .output()
        .expect("explicit installed Rust binary launches");
    assert!(explicit.status.success());
    assert!(explicit.stderr.is_empty());
    assert_ne!(
        fs::canonicalize(&python).unwrap(),
        fs::canonicalize(&rust).unwrap()
    );

    let after = Command::new(&python)
        .arg("--help")
        .current_dir(&fixture.root)
        .output()
        .expect("ambient Python entrypoint launches after Rust installation");
    assert!(after.status.success());
    assert!(after.stderr.is_empty());
    assert_eq!(fs::read_link(&python).unwrap(), python_link);
    assert_eq!(fs::read(&python).unwrap(), python_bytes);
    fixture.assert_python_authority_unchanged();

    let unapplied: InstallReceipt = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert_eq!(fs::read_link(&python).unwrap(), python_link);
    assert_eq!(fs::read(&python).unwrap(), python_bytes);
}

#[test]
fn apply_destination_replacement_at_publication_preserves_both_images() {
    let fixture = Fixture::new("apply-target-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let destination = fixture.root.join(".git/info/exclude");
    let parked = fixture.root.join(".git/info/exclude.parked");
    let racer = b"racer-owned exclusions\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park planned exclusion destination");
    fs::write(&destination, racer).expect("replace planned exclusion destination");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), fixture.exclude_bytes);
    fixture.assert_python_authority_unchanged();
}

/// A paused install whose harness never releases it ends on its own bound.
///
/// Nothing writes `release/continue` here, so the only ending reachable is the
/// horizon the wait carries. That makes the fixture cost the whole ten seconds
/// by construction, and that cost is the point: it is what distinguishes a
/// bound that still terminates the wait from one merely written down beside a
/// wait that now blocks on a filesystem notification.
#[test]
fn apply_pause_no_harness_releases_faults_when_its_bound_expires() {
    let fixture = Fixture::new("apply-abandoned-pause");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let destination = fixture.root.join(".git/info/exclude");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    let expired = paused.abandon();

    assert_eq!(expired.code, Some(1));
    assert!(expired.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&expired.err).expect("typed rendezvous fault decodes");
    let Outcome::Fault(fault) = outcome else {
        panic!("an unreleased install must fault: {outcome:?}");
    };
    assert_eq!(fault.class, wire::FaultClass::Environment);
    assert_eq!(fault.reason, "install::environment-failed");
    assert_eq!(
        fault.detail,
        format!(
            "failed to wait for install mutation rendezvous at {}: \
             test rendezvous timed out",
            fs::canonicalize(&fixture.root)
                .expect("canonicalize install fixture")
                .join(".git/info/exclude")
                .display()
        ),
    );
    assert_eq!(fs::read(&destination).unwrap(), fixture.exclude_bytes);
    fixture.assert_python_authority_unchanged();
}

/// A release already in place when the wait arms is observed, not waited out.
///
/// `release/continue` is written before the install is spawned, so by the time
/// the wait arms its watcher the file it waits for is already there. The install
/// then writes `ready/arrived` in a sibling directory; the release directory
/// remains untouched, so no notification about this release can arrive. The
/// only thing left that can end this wait is the release test it makes before
/// its first receive.
///
/// A green run here is a run that reached that wait: `ready/arrived`, which the
/// harness waits for before collecting, is written immediately before the wait
/// is entered, so it cannot be reported by an install that never paused at all.
///
/// The fixture adds no horizon cost by the same construction that makes it a
/// guard: the release is already there, so the wait ends on its first test
/// rather than on its bound. Move that test below the receive and this install
/// spends the whole ten seconds and faults instead of applying.
#[test]
fn apply_pause_released_before_the_wait_arms_is_observed_without_a_notification() {
    let fixture = Fixture::new("apply-prelanded-pause");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let operation_count = preview.plan().operations().len();
    let digest = preview.digest();
    let destination = fixture.root.join(".git/info/exclude");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Landed,
    );

    let applied: InstallReceipt = reply_body(paused.collect());

    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(applied.completed(), operation_count);
    assert_eq!(
        fs::read(fixture.installed_binary()).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_spice")).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_python_authority_unchanged();
}

#[test]
fn apply_record_parent_replacement_at_publication_preserves_both_trees() {
    let fixture = Fixture::new("apply-record-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let parent = fixture.root.join(".git/.spice-rust");
    let parked = fixture.root.join(".git/.spice-rust.parked");
    let outside = fixture.root.join("outside-record");
    fs::create_dir(&outside).expect("create replacement record tree");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        fixture.record(),
        Release::Withheld,
    );

    fs::rename(&parent, &parked).expect("park admitted record parent");
    symlink(&outside, &parent).expect("replace admitted record parent");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert!(fs::read_dir(&parked).unwrap().next().is_none());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_destination_replacement_at_removal_preserves_both_images() {
    let fixture = Fixture::new("unapply-target-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    assert_eq!(
        fixture
            .invoke(&["--json", "install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let destination = fixture.installed_binary();
    let parked = destination.with_extension("parked");
    let installed = fs::read(&destination).unwrap();
    let racer = b"racer-owned binary\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "remove",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park installed binary");
    fs::write(&destination, racer).expect("replace installed binary");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), installed);
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_destination_replacement_at_restore_preserves_both_images() {
    let fixture = Fixture::new("unapply-restore-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    assert_eq!(
        fixture
            .invoke(&["--json", "install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let destination = fixture.root.join(".git/info/exclude");
    let parked = fixture.root.join(".git/info/exclude.parked");
    let installed = fs::read(&destination).unwrap();
    let racer = b"racer-owned exclusions\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park installed exclusions");
    fs::write(&destination, racer).expect("replace installed exclusions");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), installed);
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_directory_replacement_at_cleanup_preserves_both_trees() {
    let fixture = Fixture::new("unapply-cleanup-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    assert_eq!(
        fixture
            .invoke(&["--json", "install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let destination = fixture.root.join(".spice/rust/bin");
    let parked = fixture.root.join(".spice/rust/bin.parked");
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "cleanup",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park introduced binary directory");
    fs::create_dir(&destination).expect("replace introduced binary directory");
    fs::write(destination.join("racer"), b"outside")
        .expect("populate replacement binary directory");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert!(fs::read_dir(&parked).unwrap().next().is_none());
    assert_eq!(fs::read(destination.join("racer")).unwrap(), b"outside");
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_record_parent_replacement_at_publication_preserves_both_trees() {
    let fixture = Fixture::new("unapply-record-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    assert_eq!(
        fixture
            .invoke(&["--json", "install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let record = fixture.record();
    let record_before = fs::read(&record).unwrap();
    let binary_before = fs::read(fixture.installed_binary()).unwrap();
    let parent = fixture.root.join(".git/.spice-rust");
    let parked = fixture.root.join(".git/.spice-rust.parked");
    let outside = fixture.root.join("outside-record");
    fs::create_dir(&outside).expect("create replacement record tree");
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "publish",
        record.clone(),
        Release::Withheld,
    );

    fs::rename(&parent, &parked).expect("park admitted record parent");
    symlink(&outside, &parent).expect("replace admitted record parent");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(
        fs::read(parked.join("install.json")).unwrap(),
        record_before
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    assert_eq!(fs::read(fixture.installed_binary()).unwrap(), binary_before);
    fixture.assert_python_authority_unchanged();
}

fn successful_document(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    serde_json::from_slice(&spoken.out).expect("successful output is JSON")
}

fn emitted_digest(document: &Value) -> &str {
    document["digest"]
        .as_str()
        .expect("install preview emits a digest token")
}

fn reply_body<T: DeserializeOwned + std::fmt::Debug>(spoken: Spoken) -> T {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<T> = wire::decode(&spoken.out).expect("JSON outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("install must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn assert_refusal(bytes: &[u8], reason: &str) {
    let outcome: Outcome<()> = wire::decode(bytes).expect("typed refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("install must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}

fn alternate_digest(digest: &str) -> String {
    let mut moved = digest.as_bytes().to_vec();
    let last = moved.last_mut().expect("digest is nonempty");
    *last = if *last == b'0' { b'1' } else { b'0' };
    String::from_utf8(moved).expect("digest is ASCII")
}
