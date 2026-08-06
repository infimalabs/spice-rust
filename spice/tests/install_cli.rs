#![cfg(unix)]

use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use spice::install::{InstallPreview, InstallReceipt, InstallStatus};
use wire::Outcome;

/// How long a paused install is given to reach its mutation rendezvous.
///
/// Debug binaries are deliberately large, and unapply authenticates the
/// installed image before reaching its first mutation. Keep this harness
/// horizon comfortably above that validation cost; the production rendezvous
/// retains its own ten-second release bound.
const RENDEZVOUS: Duration = Duration::from_secs(30);

static PAUSE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
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
            .expect("create Spice state sentinel directory");
        let exclude_bytes = b"# operator exclusions\n/cache/\n".to_vec();
        fs::write(root.join(".git/info/exclude"), &exclude_bytes).expect("write exclusions");
        fs::write(
            root.join(".git/.spice/data/journal.sqlite3"),
            b"Spice state sentinel\n",
        )
        .expect("write Spice state sentinel");
        fs::write(
            root.join(".git/.spice/config.toml"),
            b"[operator]\nmode = 'native'\n",
        )
        .expect("write Spice local configuration sentinel");
        let tracked_bytes = b"[tasks]\nmax_parallel = 7\n\n[policy]\nowner = \"spice\"\n".to_vec();
        fs::write(root.join("spice.toml"), &tracked_bytes).expect("write tracked intent");
        Self {
            root,
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
        let authority = if self.root.join(".git/.spice/data/journal.sqlite3").is_file() {
            self.root.join(".git/.spice")
        } else {
            self.root.join(".git/.spice.parked")
        };
        assert_eq!(
            fs::read(self.root.join("spice.toml")).unwrap(),
            self.tracked_bytes
        );
        assert_eq!(
            fs::read(authority.join("data/journal.sqlite3")).unwrap(),
            b"Spice state sentinel\n"
        );
        assert_eq!(
            fs::read(authority.join("config.toml")).unwrap(),
            b"[operator]\nmode = 'native'\n"
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
        self.root.join(".git/.spice/install.json")
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
    assert!(fixture.root.join(".git/.spice").is_dir());
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
        fs::read(env!("CARGO_BIN_EXE_spice")).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_state_unchanged();
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
    fixture.assert_state_unchanged();
}

#[test]
fn same_image_atomic_rewrite_still_plans_and_unapplies() {
    let fixture = Fixture::new("same-image-rewrite");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest().clone();
    assert_eq!(
        fixture
            .invoke(&["install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let entry = fixture.root.join(".git/.spice-install/hooks/entry.sh");
    let replacement = fixture.root.join("replacement-entry");
    fs::write(&replacement, fs::read(&entry).unwrap()).expect("write replacement entry hook");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755))
        .expect("set replacement entry hook mode");
    fs::rename(replacement, &entry).expect("atomically rewrite installed entry hook");

    let replanned: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(replanned.digest(), &digest);
    let unapply = fixture.invoke(&["--json", "install", "--unapply"]);
    let unapplied: InstallReceipt = reply_body(unapply);

    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert!(!entry.exists());
    fixture.assert_state_unchanged();
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
    let entry = fixture.root.join(".git/.spice-install/hooks/entry.sh");
    fs::write(&entry, b"operator drift\n").expect("drift installed entry hook");

    let refused = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(
        refused.code,
        Some(2),
        "unexpected drift outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::drift");
    assert_eq!(fs::read(entry).unwrap(), b"operator drift\n");
    assert!(fixture.record().is_file());
    fixture.assert_state_unchanged();
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
    fixture.assert_state_unchanged();
}

/// A paused install whose harness never releases it ends on its own bound.
///
/// Nothing writes `release/continue` here, so the only ending reachable is the
/// horizon the wait carries. That makes the fixture cost the whole ten seconds
/// by construction, and that cost is the point: it is what distinguishes a
/// bound that still terminates the event receiver from one merely written down
/// beside an unbounded receive.
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
    fixture.assert_state_unchanged();
}

/// A release already in place is observed before the receiver is armed.
///
/// `release/continue` is written before the install is spawned. The child sees
/// that authoritative marker, connects to the harness's readiness receiver,
/// and never binds a release receiver. The only thing that can produce this
/// ending is the release test before receiver setup.
///
/// A green run here is a run that reached that wait: the readiness message the
/// harness accepts before collecting is sent only by an install that reached
/// the configured pause.
///
/// The fixture adds no horizon cost by the same construction that makes it a
/// guard: the release is already there, so the wait ends on its first test
/// rather than on its bound. Remove that test and this install cannot announce
/// readiness because no harness can release a receiver that was never needed.
#[test]
fn apply_pause_released_before_receiver_setup_is_observed() {
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
    fixture.assert_state_unchanged();
}

#[test]
fn apply_record_parent_replacement_at_publication_preserves_both_trees() {
    let fixture = Fixture::new("apply-record-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let parent = fixture.root.join(".git/.spice");
    let parked = fixture.root.join(".git/.spice.parked");
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

    assert_eq!(
        refused.code,
        Some(2),
        "unexpected apply record-race outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fixture.assert_state_unchanged();
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
    fixture.assert_state_unchanged();
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
    fixture.assert_state_unchanged();
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
    let destination = fixture.root.join(".git/.spice-install/bin");
    let parked = fixture.root.join(".git/.spice-install/bin.parked");
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
    fixture.assert_state_unchanged();
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
    let parent = fixture.root.join(".git/.spice");
    let parked = fixture.root.join(".git/.spice.parked");
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

    assert_eq!(
        refused.code,
        Some(2),
        "unexpected unapply record-race outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(
        fs::read(parked.join("install.json")).unwrap(),
        record_before
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    assert_eq!(fs::read(fixture.installed_binary()).unwrap(), binary_before);
    fixture.assert_state_unchanged();
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
