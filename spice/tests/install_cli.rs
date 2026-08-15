#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde_json::Value;
use spice::install::{Asset, InstallPreview, InstallReceipt, InstallStatus};
use spice::repository::Repository;
use wire::{DoctorObservation, DoctorReport, DoctorSubsystem, Outcome, Refusal};

/// How long a paused install is given to reach its mutation rendezvous.
///
/// Unapply authenticates the installed image before reaching its first
/// mutation. Keep this harness horizon comfortably above that validation
/// cost; the production rendezvous retains its own ten-second release bound.
const RENDEZVOUS: Duration = Duration::from_secs(120);

static PAUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_PUBLIC_INSTALL_REPLY: usize = 64 * 1024;
const MIN_PRIVATE_INSTALL_RECORD: usize = 1024 * 1024;

mod install_cli {
    mod races;
}

struct Fixture {
    root: PathBuf,
    executable: PathBuf,
    tracked_bytes: Vec<u8>,
    local_bytes: Vec<u8>,
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
        fs::create_dir(&root).expect("create install fixture root");
        let initialized = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&root)
            .status()
            .expect("launch install fixture Git initialization");
        assert!(initialized.success());
        fs::create_dir_all(root.join(".git/info")).expect("create repository metadata");
        fs::create_dir_all(root.join(".git/.spice-rs/data"))
            .expect("create Spice state sentinel directory");
        let exclude_bytes = b"# operator exclusions\n/cache/\n".to_vec();
        fs::write(root.join(".git/info/exclude"), &exclude_bytes).expect("write exclusions");
        fs::write(
            root.join(".git/.spice-rs/data/journal.sqlite3"),
            b"Spice state sentinel\n",
        )
        .expect("write Spice state sentinel");
        let local_bytes =
            b"[lock.worker]\ndirectory = 'fixture-locks/worker'\nshards = 2\n".to_vec();
        fs::write(root.join(".git/.spice-rs/config.toml"), &local_bytes)
            .expect("write Spice local configuration sentinel");
        let tracked_bytes = concat!(
            "[tasks]\nstems = ['fixture']\n\n",
            "[policy]\nenv_names = ['FIXTURE_INSTALL_SENTINEL']\n",
        )
        .as_bytes()
        .to_vec();
        fs::write(root.join("spice-rust.toml"), &tracked_bytes).expect("write tracked intent");
        let executable = root.join("fixture-spice");
        fs::write(&executable, b"fixture install executable\n")
            .expect("write fixture install executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make fixture install executable");
        Self {
            root,
            executable,
            tracked_bytes,
            local_bytes,
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

    fn invoke_at(&self, arguments: &[&str], unix_seconds: u64) -> Spoken {
        let output = self
            .command(arguments)
            .env(
                spice::install::TEST_PAUSE_ACTION,
                format!("maintenance-now:{unix_seconds}"),
            )
            .output()
            .expect("spice install launches with test maintenance time");
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
            .env(spice::install::TEST_EXECUTABLE, &self.executable)
            .env_remove(spice::install::TEST_PAUSE_ACTION)
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
        let ready =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind install readiness rendezvous");
        fs::write(
            control.join("ready").join("port"),
            ready
                .local_addr()
                .expect("inspect install readiness rendezvous")
                .port()
                .to_string(),
        )
        .expect("publish install readiness rendezvous");
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
            ready: Some(ready),
            release_port: None,
        };
        paused.wait_until_ready(action);
        paused
    }

    fn assert_state_unchanged(&self) {
        let authority = if self
            .root
            .join(".git/.spice-rs/data/journal.sqlite3")
            .is_file()
        {
            self.root.join(".git/.spice-rs")
        } else {
            self.root.join(".git/.spice-rs.parked")
        };
        assert_eq!(
            fs::read(self.root.join("spice-rust.toml")).unwrap(),
            self.tracked_bytes
        );
        assert_eq!(
            fs::read(authority.join("data/journal.sqlite3")).unwrap(),
            b"Spice state sentinel\n"
        );
        assert_eq!(
            fs::read(authority.join("config.toml")).unwrap(),
            self.local_bytes
        );
        let mut entries = fs::read_dir(authority)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert!(
            entries == ["config.toml", "data"]
                || entries == ["config.toml", "data", "install.json"],
            "shared authority contains an unexpected entry: {entries:?}",
        );
    }

    fn installed_binary(&self) -> PathBuf {
        self.root.join(".git/.spice-install/bin/spice")
    }

    fn record(&self) -> PathBuf {
        self.root.join(".git/.spice-rs/install.json")
    }

    fn record_temps(&self) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(self.root.join(".git/.spice-rs"))
            .expect("read runtime directory")
            .map(|entry| entry.expect("read runtime entry").path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(".spice-install-") && name.ends_with(".tmp")
                    })
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    fn assert_small_owned_executable(&self) {
        assert!(self.executable.starts_with(&self.root));
        assert!(
            fs::metadata(&self.executable)
                .expect("inspect fixture install executable")
                .len()
                < 1024,
            "process fixture source must not copy the debug Spice binary",
        );
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

impl Spoken {
    fn assert_success(&self) {
        assert_eq!(
            self.code,
            Some(0),
            "expected successful install process: stdout={} stderr={}",
            String::from_utf8_lossy(&self.out),
            String::from_utf8_lossy(&self.err),
        );
        assert!(
            self.err.is_empty(),
            "successful install process wrote stderr: stdout={} stderr={}",
            String::from_utf8_lossy(&self.out),
            String::from_utf8_lossy(&self.err),
        );
    }
}

/// Whether the release a paused install waits for is already in place.
///
/// The disposition is stated at the spawn rather than inferred from the ending,
/// because it decides whether the child observes a prelanded marker or binds an
/// event receiver before announcing readiness.
enum Release {
    /// No release is present, so the child binds and the harness decides the
    /// ending by sending one release token.
    Withheld,
    /// `release/continue` is in place before the install is spawned.
    Landed,
}

struct Paused {
    child: Option<Child>,
    control: PathBuf,
    ready: Option<TcpListener>,
    release_port: Option<u16>,
}

impl Paused {
    fn wait_until_ready(&mut self, expected_action: &str) {
        let listener = self.ready.take().expect("readiness listener is present");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = listener.accept().and_then(|(mut stream, _)| {
                let mut message = String::new();
                stream.read_to_string(&mut message)?;
                let Some((action, port)) = message.split_once(' ') else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "install readiness message has no release port",
                    ));
                };
                let port = port
                    .parse::<u16>()
                    .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))?;
                Ok((action.to_owned(), port))
            });
            let _ = sender.send(result);
        });
        match receiver.recv_timeout(RENDEZVOUS) {
            Ok(Ok((action, _))) if action != expected_action => {
                panic!("install readiness action moved: expected {expected_action}, got {action}")
            }
            Ok(Ok((_action, 0))) => self.release_port = None,
            Ok(Ok((_action, port))) => self.release_port = Some(port),
            Ok(Err(error)) => panic!("install readiness rendezvous failed: {error}"),
            Err(error) => panic!("install readiness rendezvous did not arrive: {error}"),
        }
    }

    fn resume(self) -> Spoken {
        let port = self
            .release_port
            .expect("withheld install rendezvous publishes a release port");
        TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .and_then(|mut stream| stream.write_all(b"continue"))
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

    fn kill(mut self) -> (u32, Spoken) {
        let mut child = self.child.take().expect("paused child is present");
        let pid = child.id();
        child.kill().expect("kill paused installer");
        let output = child
            .wait_with_output()
            .expect("collect killed install process");
        fs::remove_dir_all(&self.control).expect("remove install race rendezvous");
        (
            pid,
            Spoken {
                code: output.status.code(),
                out: output.stdout,
                err: output.stderr,
            },
        )
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

#[derive(Debug, Eq, PartialEq)]
struct FileSnapshot {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn file_snapshot(path: &Path) -> FileSnapshot {
    let metadata = fs::metadata(path).expect("inspect temporary record");
    FileSnapshot {
        bytes: fs::read(path).expect("read fixture temporary record"),
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn installation_observation(spoken: &Spoken) -> DoctorObservation {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<DoctorReport<spice::config::Root>> =
        wire::decode(&spoken.out).expect("Doctor JSON decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("Doctor inspection must reply: {outcome:?}");
    };
    reply
        .body
        .subsystems
        .into_iter()
        .find(|diagnosis| diagnosis.subsystem == DoctorSubsystem::Installation)
        .expect("Doctor reports installation")
        .observation
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
    let operation_count = preview.plan().operation_count();
    assert_eq!(operation_count, 10);
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    assert!(fixture.root.join(".git/.spice-rs").is_dir());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_state_unchanged();

    let malformed = fixture.invoke(&["--json", "install", "--apply=not-a-digest"]);
    assert_eq!(malformed.code, Some(2));
    assert!(malformed.out.is_empty());
    assert_refusal(&malformed.err, "install::digest-refused");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());

    let moved = alternate_digest(&text_digest);
    let refused = fixture.invoke(&["--json", "install", &format!("--apply={moved}")]);
    assert_eq!(
        refused.code,
        Some(2),
        "unexpected digest-mismatch outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::digest-moved");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_state_unchanged();

    let applied: InstallReceipt =
        reply_body(fixture.invoke(&["--json", "install", &format!("--apply={text_digest}")]));
    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(applied.completed(), operation_count);
    assert_eq!(
        fs::read(fixture.installed_binary()).unwrap(),
        fs::read(&fixture.executable).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_state_unchanged();
}

#[test]
fn large_migration_payload_never_crosses_the_cli_reply_boundary() {
    let migration = LargeMigration::new("bounded-migration");
    let fixture = &migration.fixture;
    let text_preview = fixture.invoke(&["install"]);
    let json_preview = fixture.invoke(&["--json", "install"]);
    let text_preview_body: InstallPreview = bounded_install_reply(&text_preview, false);
    let json_preview_body: InstallPreview = bounded_install_reply(&json_preview, true);
    assert_eq!(text_preview_body, json_preview_body);
    let archive = text_preview_body
        .plan()
        .rollback_archive()
        .expect("migration preview reports rollback metadata");
    assert!(archive.uncompressed_bytes() > MIN_PRIVATE_INSTALL_RECORD as u64);
    let digest = text_preview_body.digest().clone();

    let text_apply = fixture.invoke(&["install", &format!("--apply={digest}")]);
    let text_applied: InstallReceipt = bounded_install_reply(&text_apply, false);
    assert_eq!(text_applied.status(), InstallStatus::Complete);
    assert_eq!(text_applied.plan(), text_preview_body.plan());
    let json_apply = fixture.invoke(&["--json", "install", &format!("--apply={digest}")]);
    let json_applied: InstallReceipt = bounded_install_reply(&json_apply, true);
    assert_eq!(json_applied, text_applied);

    let private_record = fs::read(fixture.record()).expect("read private migration receipt");
    assert!(private_record.len() > MIN_PRIVATE_INSTALL_RECORD);
    let private_document: Value =
        serde_json::from_slice(&private_record).expect("private migration receipt is JSON");
    let payload = private_document["plan"]["rollback_archive"]["payload"]
        .as_str()
        .expect("private migration receipt retains rollback payload");
    assert!(payload.len() > MIN_PRIVATE_INSTALL_RECORD);
    let sentinel = &payload[payload.len() / 2..payload.len() / 2 + 64];

    let text_unapply = fixture.invoke(&["install", "--unapply"]);
    let text_removed: InstallReceipt = bounded_install_reply(&text_unapply, false);
    assert_eq!(text_removed.status(), InstallStatus::Uninstalled);
    migration.assert_restored();

    let repeated_preview = fixture.invoke(&["--json", "install"]);
    let repeated_preview_body: InstallPreview = bounded_install_reply(&repeated_preview, true);
    assert_eq!(repeated_preview_body, json_preview_body);
    let repeated_apply = fixture.invoke(&["--json", "install", &format!("--apply={digest}")]);
    let repeated_applied: InstallReceipt = bounded_install_reply(&repeated_apply, true);
    assert_eq!(repeated_applied, json_applied);
    let json_unapply = fixture.invoke(&["--json", "install", "--unapply"]);
    let json_removed: InstallReceipt = bounded_install_reply(&json_unapply, true);
    assert_eq!(json_removed, text_removed);
    migration.assert_restored();

    for spoken in [
        &text_preview,
        &json_preview,
        &text_apply,
        &json_apply,
        &text_unapply,
        &repeated_preview,
        &repeated_apply,
        &json_unapply,
    ] {
        assert!(
            !spoken
                .out
                .windows(sentinel.len())
                .any(|window| window == sentinel.as_bytes()),
            "public install reply exposed rollback payload bytes",
        );
    }
}

#[test]
fn installed_hook_activation_uses_no_cargo_or_xtask_and_reverses_git_config() {
    let fixture = Fixture::new("no-cargo-hook-activation");
    let tools = fixture.root.join("fixture-tools");
    fs::create_dir(&tools).expect("create fixture tool directory");
    symlink("/usr/bin/git", tools.join("git")).expect("link fixture Git");
    let invoked = fixture.root.join("build-tool-invoked");
    for name in ["cargo", "xtask"] {
        let path = tools.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' {name} >> {:?}\nexit 97\n",
                invoked,
            ),
        )
        .expect("write forbidden build-tool trap");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make forbidden build-tool trap executable");
    }
    let invoke = |arguments: &[&str]| {
        let output = fixture
            .command(arguments)
            .env("PATH", &tools)
            .output()
            .expect("launch install with trapped build tools");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    };

    let preview: InstallPreview = reply_body(invoke(&["--json", "install"]));
    assert!(preview.plan().hook_activation().is_some());
    assert!(git_hook_path(&fixture.root).is_none());
    assert!(!invoked.exists());

    let receipt: InstallReceipt = reply_body(invoke(&[
        "--json",
        "install",
        &format!("--apply={}", preview.digest()),
    ]));
    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(
        git_hook_path(&fixture.root).as_deref(),
        preview
            .plan()
            .hook_activation()
            .map(|activation| activation.hooks_path()),
    );
    assert!(!invoked.exists());

    let removed: InstallReceipt = reply_body(invoke(&["--json", "install", "--unapply"]));
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert!(git_hook_path(&fixture.root).is_none());
    assert!(!invoked.exists());
}

fn git_hook_path(root: &std::path::Path) -> Option<String> {
    let output = Command::new("/usr/bin/git")
        .args(["config", "--get", "core.hooksPath"])
        .current_dir(root)
        .output()
        .expect("read fixture hook selection");
    if output.status.code() == Some(1) {
        return None;
    }
    assert!(output.status.success());
    Some(
        String::from_utf8(output.stdout)
            .expect("fixture hook path is UTF-8")
            .trim_ascii_end()
            .to_owned(),
    )
}

#[test]
fn foreign_exclusions_survive_preview_apply_refresh_and_unapply() {
    let mut exclusions = concat!(
        "# operator exclusions\n",
        "/cache/\n",
        "# BEGIN spice-rust coexistence\n",
        "/.spice/rust/\n",
        "/.agents/skills/spice-rust/\n",
        "# END spice-rust coexistence\n",
        "# BEGIN Spice managed assets\n",
        "/.spice/\n",
        "/.agents/skills/spice/\n",
        "# END Spice managed assets\n",
        "# BEGIN Spice managed assets\n",
        "foreign malformed duplicate\n",
    )
    .as_bytes()
    .to_vec();
    exclusions.push(0xff);

    let mut fixture = Fixture::new("foreign-exclusions");
    fixture.exclude_bytes.clone_from(&exclusions);
    let exclude = fixture.root.join(".git/info/exclude");
    fs::write(&exclude, &exclusions).expect("install foreign exclusions");

    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let repeated: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(preview, repeated);
    assert!(
        preview
            .plan()
            .operations()
            .iter()
            .all(|operation| operation.asset() != Asset::Exclusions)
    );
    assert!(preview.plan().operations().iter().all(|operation| {
        !operation.path().starts_with(".spice-rs")
            && !operation.path().starts_with(".agents/skills/spice")
    }));
    assert_eq!(fs::read(&exclude).unwrap(), exclusions);

    let applied: InstallReceipt = reply_body(fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", preview.digest()),
    ]));
    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(fs::read(&exclude).unwrap(), exclusions);

    fs::write(
        &fixture.executable,
        b"refreshed fixture install executable\n",
    )
    .expect("rebuild fixture executable");
    let refresh: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_ne!(refresh.digest(), preview.digest());
    let refreshed: InstallReceipt = reply_body(fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", refresh.digest()),
    ]));
    assert_eq!(refreshed.status(), InstallStatus::Complete);
    assert_eq!(fs::read(&exclude).unwrap(), exclusions);

    let unapplied: InstallReceipt = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert_eq!(fs::read(&exclude).unwrap(), exclusions);
    fixture.assert_state_unchanged();
}

#[test]
fn retired_state_refusal_names_ownership_recovery_and_changes_nothing() {
    let fixture = Fixture::new("retired-state-refusal");
    let retired = fixture.root.join(".git/.spice-rust");
    fs::create_dir(&retired).expect("create retired Rust state root");
    let evidence = retired.join("evidence");
    fs::write(&evidence, b"retired Rust evidence\n").expect("write retired Rust evidence");
    let canonical = fs::canonicalize(&retired).expect("canonicalize retired Rust state root");
    let inspection = shell_words::join([
        "ls".to_owned(),
        "-ld".to_owned(),
        "--".to_owned(),
        canonical.to_string_lossy().into_owned(),
    ]);

    let text = fixture.invoke(&["install"]);
    assert_eq!(text.code, Some(2));
    assert!(text.out.is_empty());
    let text = String::from_utf8(text.err).expect("text refusal is UTF-8");
    for signal in [
        "install::state-refused",
        "retired state root belongs to release=release-one",
        canonical.to_string_lossy().as_ref(),
        inspection.as_str(),
        "operator-chosen path",
    ] {
        assert!(
            text.contains(signal),
            "text refusal omitted {signal:?}: {text}"
        );
    }

    let json = fixture.invoke(&["--json", "install"]);
    assert_eq!(json.code, Some(2));
    assert!(json.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&json.err).expect("JSON refusal decodes");
    let Outcome::Refusal(Refusal::Bare { diagnostic }) = outcome else {
        panic!("retired state must be one bare refusal: {outcome:?}");
    };
    assert_eq!(diagnostic.reason(), "install::state-refused");
    assert_eq!(diagnostic.witness(), Some("install-state"));
    assert_eq!(
        diagnostic.expected(),
        Some("unlinked current-release Rust state under .spice-rs")
    );
    let observed = diagnostic.observed().expect("refusal carries observation");
    assert!(observed.contains("release=release-one"), "{observed}");
    assert!(
        observed.contains(canonical.to_string_lossy().as_ref()),
        "{observed}"
    );
    let detail = diagnostic
        .detail()
        .expect("refusal carries recovery detail");
    assert!(detail.starts_with(observed), "{detail}");
    assert!(detail.contains(&inspection), "{detail}");
    assert!(detail.contains("operator-chosen path"), "{detail}");

    assert_eq!(fs::read(&evidence).unwrap(), b"retired Rust evidence\n");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    fixture.assert_state_unchanged();
}

fn successful_document(spoken: Spoken) -> Value {
    spoken.assert_success();
    serde_json::from_slice(&spoken.out).expect("successful output is JSON")
}

struct LargeMigration {
    fixture: Fixture,
    prior_record: Vec<u8>,
    legacy_xtask: PathBuf,
    legacy_guard: PathBuf,
    xtask_bytes: Vec<u8>,
    guard_bytes: Vec<u8>,
}

impl LargeMigration {
    fn new(label: &str) -> Self {
        let fixture = Fixture::new(label);
        let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
        let _: InstallReceipt = reply_body(fixture.invoke(&[
            "--json",
            "install",
            &format!("--apply={}", preview.digest()),
        ]));
        let repository = Repository::discover(&fixture.root).expect("discover migration fixture");
        spice::install::publish_test_version_7_receipt(&repository)
            .expect("publish version-7 process fixture receipt");
        let prior_record = fs::read(fixture.record()).expect("read version-7 process receipt");
        let legacy_xtask = fixture.root.join(".git/.spice-install/bin/xtask");
        let legacy_guard = fixture.root.join(".git/.spice-install/bin/xtask.guard");
        let xtask_bytes = deterministic_large_bytes(2 * MIN_PRIVATE_INSTALL_RECORD);
        let guard_bytes = b"large-migration-guard\n".to_vec();
        fs::write(&legacy_xtask, &xtask_bytes).expect("write large legacy xtask");
        fs::set_permissions(&legacy_xtask, fs::Permissions::from_mode(0o755))
            .expect("set large legacy xtask mode");
        fs::write(&legacy_guard, &guard_bytes).expect("write legacy guard");
        fs::set_permissions(&legacy_guard, fs::Permissions::from_mode(0o644))
            .expect("set legacy guard mode");
        Self {
            fixture,
            prior_record,
            legacy_xtask,
            legacy_guard,
            xtask_bytes,
            guard_bytes,
        }
    }

    fn assert_restored(&self) {
        assert_eq!(
            fs::read(self.fixture.record()).expect("read restored version-7 receipt"),
            self.prior_record,
        );
        assert_eq!(
            fs::read(&self.legacy_xtask).expect("read restored legacy xtask"),
            self.xtask_bytes,
        );
        assert_eq!(
            fs::metadata(&self.legacy_xtask)
                .expect("inspect restored legacy xtask")
                .permissions()
                .mode()
                & 0o777,
            0o755,
        );
        assert_eq!(
            fs::read(&self.legacy_guard).expect("read restored legacy guard"),
            self.guard_bytes,
        );
        assert_eq!(
            fs::metadata(&self.legacy_guard)
                .expect("inspect restored legacy guard")
                .permissions()
                .mode()
                & 0o777,
            0o644,
        );
    }
}

fn deterministic_large_bytes(length: usize) -> Vec<u8> {
    let mut state = 0xd1b5_4a32_d192_ed03_u64;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn bounded_install_reply<T: DeserializeOwned + std::fmt::Debug>(spoken: &Spoken, json: bool) -> T {
    spoken.assert_success();
    assert!(
        spoken.out.len() <= MAX_PUBLIC_INSTALL_REPLY,
        "public install reply grew to {} bytes",
        spoken.out.len(),
    );
    let document: Value =
        serde_json::from_slice(&spoken.out).expect("public install reply is JSON");
    assert_no_private_install_key(&document);
    if json {
        let outcome: Outcome<T> = wire::decode(&spoken.out).expect("JSON install reply decodes");
        let Outcome::Reply(reply) = outcome else {
            panic!("install must reply: {outcome:?}");
        };
        assert_eq!(reply.position, wire::ReplyPosition::Direct {});
        reply.body
    } else {
        wire::decode(&spoken.out).expect("text install body decodes")
    }
}

fn assert_no_private_install_key(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                assert!(
                    !matches!(name.as_str(), "payload" | "bytes" | "repository"),
                    "public install reply exposed private field {name:?}",
                );
                assert_no_private_install_key(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_no_private_install_key(value);
            }
        }
        _ => {}
    }
}

fn emitted_digest(document: &Value) -> &str {
    document["digest"]
        .as_str()
        .expect("install preview emits a digest token")
}

fn reply_body<T: DeserializeOwned + std::fmt::Debug>(spoken: Spoken) -> T {
    spoken.assert_success();
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
    assert_eq!(refusal.reason(), reason);
}

fn alternate_digest(digest: &str) -> String {
    let mut moved = digest.as_bytes().to_vec();
    let last = moved.last_mut().expect("digest is nonempty");
    *last = if *last == b'0' { b'1' } else { b'0' };
    String::from_utf8(moved).expect("digest is ASCII")
}
