#![cfg(unix)]

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use spice::install::{InstallPlanSummary, InstallPreview, InstallReceipt, InstallStatus, Location};
use spice::repository::Repository;
use wire::{Outcome, Refusal};

#[path = "support/git_repository.rs"]
mod git_repository;

/// How long a paused install is given to reach its mutation rendezvous.
///
/// Unapply authenticates the installed image before reaching its first
/// mutation. Keep this harness horizon comfortably above that validation
/// cost; the production rendezvous retains its own ten-second release bound.
const RENDEZVOUS: Duration = Duration::from_secs(120);

static PAUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MIN_PRIVATE_INSTALL_RECORD: usize = 64 * 1024;

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
        git_repository::initialize(&root);
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

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(arguments)
            .current_dir(&self.root)
            .env(spice::install::TEST_EXECUTABLE, &self.executable)
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

    fn rewrite_executable(&self, bytes: &[u8]) {
        fs::write(&self.executable, bytes).expect("rewrite fixture install executable");
        fs::set_permissions(&self.executable, fs::Permissions::from_mode(0o755))
            .expect("restore fixture executable mode");
        self.assert_small_owned_executable();
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
        let listener = self
            .ready
            .as_ref()
            .expect("readiness listener is present")
            .try_clone()
            .expect("clone readiness listener");
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

    fn release(&mut self) {
        let port = self
            .release_port
            .take()
            .expect("withheld install rendezvous publishes a release port");
        fs::write(self.control.join("release").join("continue"), b"continue")
            .expect("land install release for later matching mutations");
        TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .and_then(|mut stream| stream.write_all(b"continue"))
            .expect("release install mutation rendezvous");
    }

    fn resume(mut self) -> Spoken {
        self.release();
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

impl Drop for Paused {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.control.exists() {
            let _ = fs::remove_dir_all(&self.control);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileSnapshot {
    bytes: Vec<u8>,
    mode: u32,
    device: u64,
    inode: u64,
}

impl FileSnapshot {
    fn capture(path: &std::path::Path) -> Option<Self> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => panic!("inspect {}: {error}", path.display()),
        };
        assert!(metadata.is_file(), "{} is not a file", path.display());
        Some(Self {
            bytes: fs::read(path).unwrap_or_else(|error| {
                panic!("read {}: {error}", path.display());
            }),
            mode: metadata.permissions().mode() & 0o777,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstallSnapshot {
    record: Option<FileSnapshot>,
    targets: Vec<(String, Option<FileSnapshot>)>,
    git_config: FileSnapshot,
}

impl InstallSnapshot {
    fn capture(fixture: &Fixture, plan: &InstallPlanSummary) -> Self {
        let repository = Repository::discover(&fixture.root).expect("discover snapshot repository");
        let mut targets = plan
            .operations()
            .iter()
            .map(|operation| {
                let root = match operation.location() {
                    Location::Worktree => repository.worktree_root(),
                    Location::CommonGit => repository.common_dir(),
                };
                let path = root.join(operation.path());
                (operation.path().to_owned(), FileSnapshot::capture(&path))
            })
            .collect::<Vec<_>>();
        targets.extend(plan.retirements().iter().map(|retirement| {
            let path = repository.common_dir().join(retirement.path());
            (retirement.path().to_owned(), FileSnapshot::capture(&path))
        }));
        Self {
            record: FileSnapshot::capture(&fixture.record()),
            targets,
            git_config: FileSnapshot::capture(&fixture.root.join(".git/config"))
                .expect("fixture Git configuration exists"),
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
    let operation_count = preview.plan().operation_count();
    assert_eq!(operation_count, 11);
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
    let bytes = fs::read(root.join(".git/config")).expect("read fixture Git configuration");
    let config = gix_config::File::from_bytes_no_includes(
        &bytes,
        gix_config::file::Metadata::default(),
        gix_config::file::init::Options::default(),
    )
    .expect("parse fixture Git configuration");
    config
        .raw_value("core.hooksPath")
        .ok()
        .map(|value| String::from_utf8(value.into()).expect("fixture hook path is UTF-8"))
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

#[path = "install_cli/mutations.rs"]
mod mutations;
#[path = "install_cli/races.rs"]
mod races;

use mutations::{emitted_digest, successful_document};
